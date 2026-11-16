// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria (A/V) — seam `shared-core` (hevc_core's decoded picture buffer, beside h264_core's).
//!
//! HEVCINTER (LEDGER SR216): the decoded picture buffer — ONE structure holding both the reference marking and the
//! output order (the shape of h264_core's `dpb.rs`): the reference picture set (8.3.2: the five lists, long-term
//! pictures by full PicOrderCntVal or by its LSBs with the DeltaPocMsbCycleLt accumulation of 7-52, marking),
//! reference picture lists (8.3.4: RefPicListTemp0 / 1 and ref_pic_lists_modification), and the output and removal of
//! pictures (C.5.2.2 removal, C.5.2.3 storage and "bumping", C.5.2.4). A picture is output as h264_core's `Frame` by
//! [`Picture::to_frame`] when it is bumped.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::picture::Picture;
use crate::ps::{Geometry, Sps};
use crate::slice::{SliceHeader, SliceType};
use crate::{Error, Frame, Result};

/// The reference marking of a stored picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marking {
    Unused,
    Short,
    Long,
}

/// One picture in the DPB.
pub struct DpbPic {
    pub pic: Picture,
    pub output_needed: bool,
    /// PicLatencyCount (C.5.2.3).
    pub latency: u32,
    pub marking: Marking,
}

impl DpbPic {
    pub fn is_ref(&self) -> bool {
        self.marking != Marking::Unused
    }
}

/// The current picture's RefPicSetStCurrBefore / StCurrAfter / LtCurr (8.3.2), as the PicOrderCntVal of the pictures
/// found in the DPB (the lists of every slice of the picture are built from them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rps {
    pub st_curr_before: Vec<i32>,
    pub st_curr_after: Vec<i32>,
    pub lt_curr: Vec<i32>,
    /// HEVCREXTSCC (SR357): the current picture's PicOrderCntVal when pps_curr_pic_ref_enabled_flag (it is a long-term
    /// reference picture of its own slices, 8.1.3).
    pub curr: Option<i32>,
}

/// HEVCREXTSCC (SR357): the `RefList::dpb` index of the current picture (intra block copy); not a DPB index.
pub const CUR_PIC: usize = usize::MAX;

/// One reference picture list of a slice (8.3.4): per reference index the DPB index of the picture, its
/// PicOrderCntVal and whether it is a long-term reference picture.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefList {
    pub dpb: Vec<usize>,
    pub poc: Vec<i32>,
    pub long_term: Vec<bool>,
}

impl RefList {
    pub fn len(&self) -> usize {
        self.dpb.len()
    }
    pub fn is_empty(&self) -> bool {
        self.dpb.is_empty()
    }
}

#[derive(Default)]
pub struct Dpb {
    pub pics: Vec<DpbPic>,
    /// HEVCINTERCIP (SR688): the PicOrderCntVal of every picture [`Dpb::generate_missing`] made, until taken.
    pub generated: Vec<i32>,
    // ---- HEVCERRORCONCEAL (LEDGER SR700): the concealment note (named struct-tail block) ----
    /// HEVCERRORCONCEAL (SR700): the [`Concealed`] note (generated / source PicOrderCntVal, projected: SR710) of every generated picture whose
    /// samples [`Dpb::conceal_from`] copied from the nearest held picture, until taken.
    pub concealed: Vec<Concealed>,
    /// HEVCERRORCONCEAL (SR700): generated pictures left grey because the nearest held picture's planes or bit depths
    /// differed from theirs.
    pub conceal_misfit: u32,
}

impl Dpb {
    pub fn len(&self) -> usize {
        self.pics.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pics.is_empty()
    }

    pub fn clear(&mut self) {
        self.pics.clear();
    }

    /// Mark every picture "unused for reference".
    pub fn unmark_all(&mut self) {
        for p in self.pics.iter_mut() {
            p.marking = Marking::Unused;
        }
    }

    /// 8.3.2 for the current picture (PicOrderCntVal `poc`) whose first slice header is `sh`. An IRAP picture with
    /// NoRaslOutputFlag 1 first marks every picture unused. A picture of StCurrBefore / StCurrAfter / LtCurr that is
    /// not in the DPB is generated (8.3.3.2, [`Dpb::generate_missing`] at the picture geometry `geo`; HEVCINTERCIP
    /// SR688); missing Foll pictures are allowed.
    pub fn apply_rps(&mut self, sh: &SliceHeader, sps: &Sps, geo: &Geometry, poc: i32, irap_no_rasl: bool) -> Result<Rps> {
        if irap_no_rasl {
            self.unmark_all();
        }
        let max_lsb = 1i32 << sps.log2_max_poc_lsb;
        let (mut before, mut after, mut st_foll) = (Vec::new(), Vec::new(), Vec::new());
        if let Some(r) = sh.rps(sps) {
            for (d, &u) in r.delta_poc_s0.iter().zip(r.used_s0.iter()) {
                if u { before.push(poc + d) } else { st_foll.push(poc + d) }
            }
            for (d, &u) in r.delta_poc_s1.iter().zip(r.used_s1.iter()) {
                if u { after.push(poc + d) } else { st_foll.push(poc + d) }
            }
        }
        // PocLtCurr / PocLtFoll with CurrDeltaPocMsbPresentFlag / FollDeltaPocMsbPresentFlag (7-52, 8-5).
        let (mut lt_curr, mut lt_foll) = (Vec::new(), Vec::new());
        let mut cycle = 0i32;
        for (i, lt) in sh.long_term.iter().enumerate() {
            let d = lt.delta_poc_msb_cycle.unwrap_or(0).min(1 << 20) as i32;
            cycle = if i == 0 || i == sh.num_long_term_sps as usize { d } else { d + cycle };
            let mut p = lt.poc_lsb as i32;
            let msb = lt.delta_poc_msb_cycle.is_some();
            if msb {
                p += poc - cycle * max_lsb - (poc & (max_lsb - 1));
            }
            if lt.used { lt_curr.push((p, msb)) } else { lt_foll.push((p, msb)) }
        }
        let find_lt = |pics: &[DpbPic], (p, msb): (i32, bool)| {
            pics.iter().position(|d| d.is_ref() && if msb { d.pic.poc == p } else { d.pic.poc & (max_lsb - 1) == p })
        };
        let mut keep_lt = Vec::new();
        let mut lt_curr_poc = Vec::new();
        for &e in lt_curr.iter() {
            let i = find_lt(&self.pics, e).unwrap_or_else(|| self.generate_missing(e.0, true, sps, geo));
            keep_lt.push(i);
            lt_curr_poc.push(self.pics[i].pic.poc);
        }
        for &e in lt_foll.iter() {
            if let Some(i) = find_lt(&self.pics, e) {
                keep_lt.push(i);
            }
        }
        let find_st = |pics: &[DpbPic], p: i32| {
            pics.iter().enumerate().position(|(i, d)| d.marking == Marking::Short && d.pic.poc == p && !keep_lt.contains(&i))
        };
        let mut keep_st = Vec::new();
        for &p in before.iter().chain(after.iter()) {
            keep_st.push(find_st(&self.pics, p).unwrap_or_else(|| self.generate_missing(p, false, sps, geo)));
        }
        for &p in st_foll.iter() {
            if let Some(i) = find_st(&self.pics, p) {
                keep_st.push(i);
            }
        }
        for (i, d) in self.pics.iter_mut().enumerate() {
            if keep_lt.contains(&i) {
                d.marking = Marking::Long;
            } else if !keep_st.contains(&i) {
                d.marking = Marking::Unused;
            }
        }
        Ok(Rps { st_curr_before: before, st_curr_after: after, lt_curr: lt_curr_poc, curr: None })
    }

    fn ref_index(&self, poc: i32) -> Option<usize> {
        self.pics.iter().position(|d| d.is_ref() && d.pic.poc == poc)
    }

    /// 8.3.4: RefPicList0 (and RefPicList1 for a B slice) of slice `sh` from the picture's set `rps`: the entries of
    /// [`list_entries`] mapped to their DPB pictures (the current picture to [`CUR_PIC`]).
    pub fn ref_lists(&self, rps: &Rps, sh: &SliceHeader) -> Result<[RefList; 2]> {
        let map = |entries: Vec<(i32, bool, bool)>| -> Result<RefList> {
            let mut l = RefList::default();
            for (p, lt, cur) in entries {
                l.dpb.push(if cur { CUR_PIC } else { self.ref_index(p).ok_or(Error::Invalid("reference picture set names a missing picture (8.3.2)"))? });
                l.poc.push(p);
                l.long_term.push(lt);
            }
            Ok(l)
        };
        let l0 = map(list_entries(rps, sh, true)?)?;
        let l1 = if sh.slice_type == SliceType::B { map(list_entries(rps, sh, false)?)? } else { RefList::default() };
        Ok([l0, l1])
    }

    /// Drop the pictures neither needed for output nor used for reference.
    pub fn prune(&mut self) {
        self.pics.retain(|p| p.output_needed || p.is_ref());
    }

    fn waiting(&self) -> usize {
        self.pics.iter().filter(|p| p.output_needed).count()
    }

    fn over(&self, reorder: u32, latency: u32) -> bool {
        self.waiting() as u32 > reorder || (latency != 0 && self.pics.iter().any(|p| p.output_needed && p.latency >= latency))
    }

    /// C.5.2.4 "bumping": output the picture with the smallest PicOrderCntVal that waits for output (and drop it when
    /// it is no reference). False when nothing waits.
    pub fn bump(&mut self, out: &mut VecDeque<Frame>) -> bool {
        let Some(i) = (0..self.pics.len()).filter(|&i| self.pics[i].output_needed).min_by_key(|&i| self.pics[i].pic.poc) else {
            return false;
        };
        out.push_back(self.pics[i].pic.to_frame());
        self.pics[i].output_needed = false;
        if !self.pics[i].is_ref() {
            self.pics.remove(i);
        }
        true
    }

    pub fn bump_all(&mut self, out: &mut VecDeque<Frame>) {
        while self.bump(out) {}
        self.prune();
    }

    /// C.5.2.2 before a picture that is not an IRAP with NoRaslOutputFlag 1 (after its RPS is applied): remove what is
    /// neither waiting nor referenced, then bump while too many pictures wait, one waited too long, or the DPB is full.
    /// HEVCSCCTAIL (SR365, HEVCREXT-SCC-DPB): a picture with pps_curr_pic_ref_enabled_flag holds a slot of its own while it
    /// is decoded (A.4.2's maxDpbPicBuf 7), so the decoder passes `max_dec` less that slot.
    pub fn make_room(&mut self, reorder: u32, latency: u32, max_dec: u32, out: &mut VecDeque<Frame>) {
        self.prune();
        while self.over(reorder, latency) || (!self.pics.is_empty() && self.pics.len() as u32 >= max_dec) {
            if !self.bump(out) {
                break;
            }
        }
    }

    /// C.5.2.3: store the decoded current picture (marked "used for short-term reference", waiting for output when
    /// its PicOutputFlag is 1), then bump while too many pictures wait or one waited too long.
    pub fn store(&mut self, pic: Picture, reorder: u32, latency: u32, out: &mut VecDeque<Frame>) {
        let output = pic.output;
        if output {
            for p in self.pics.iter_mut().filter(|p| p.output_needed) {
                p.latency += 1;
            }
        }
        self.pics.push(DpbPic { pic, output_needed: output, latency: 0, marking: Marking::Short });
        while self.over(reorder, latency) {
            if !self.bump(out) {
                break;
            }
        }
    }
}

impl Rps {
    /// HEVCREXTSCC (SR357): the set with the current picture (PicOrderCntVal `poc`) as a reference when `on`
    /// (pps_curr_pic_ref_enabled_flag).
    pub fn with_curr(mut self, on: bool, poc: i32) -> Rps {
        self.curr = on.then_some(poc);
        self
    }
}

/// 8.3.4: the entries of RefPicList0 (`list0`) or RefPicList1 of slice `sh` from the set `rps`, as (PicOrderCntVal,
/// long-term, the current picture) per reference index: RefPicListTemp0 / 1 (8-8, 8-10) and list_entry. HEVCREXTSCC
/// (SR357): with `rps.curr` the current picture closes every round of the temp list as a long-term picture, and 8-9
/// puts it in the last entry of an unmodified RefPicList0 when the list is shorter than NumRpsCurrTempList0.
/// HEVCSCCTAIL (SR365): the ONE list builder — [`Dpb::ref_lists`] maps its POCs to pictures, and the slice header's
/// pred_weight_table reads only its current-picture flags (7.3.6.3 skips their weights).
pub fn list_entries(rps: &Rps, sh: &SliceHeader, list0: bool) -> Result<Vec<(i32, bool, bool)>> {
    let total = rps.st_curr_before.len() + rps.st_curr_after.len() + rps.lt_curr.len() + rps.curr.is_some() as usize;
    if total == 0 {
        return Err(Error::Invalid("NumPicTotalCurr is 0 in a P or B slice"));
    }
    let (first, second, active, entries) = if list0 {
        (&rps.st_curr_before, &rps.st_curr_after, sh.num_ref_idx_l0_active, &sh.list_entry_l0)
    } else {
        (&rps.st_curr_after, &rps.st_curr_before, sh.num_ref_idx_l1_active, &sh.list_entry_l1)
    };
    let n = (active as usize).max(total);
    // (PicOrderCntVal, long-term, the current picture)
    let mut temp: Vec<(i32, bool, bool)> = Vec::with_capacity(n + 1);
    while temp.len() < n {
        for &p in first.iter().chain(second.iter()) {
            if temp.len() < n {
                temp.push((p, false, false));
            }
        }
        for &p in rps.lt_curr.iter() {
            if temp.len() < n {
                temp.push((p, true, false));
            }
        }
        if let Some(c) = rps.curr {
            temp.push((c, true, true));
        }
    }
    let mut l = Vec::with_capacity(active as usize);
    for r in 0..active as usize {
        let e = match entries {
            Some(v) => *v.get(r).ok_or(Error::Invalid("list_entry"))? as usize,
            None => r,
        };
        l.push(*temp.get(e).ok_or(Error::Invalid("list_entry beyond the reference picture set"))?);
    }
    // 8-9 (HEVCREXTSCC): NumRpsCurrTempList0 > num_ref_idx_l0_active_minus1 + 1 without list_entry_l0.
    if let (true, Some(c), None, true, Some(last)) = (list0, rps.curr, entries, total > active as usize, (active as usize).checked_sub(1)) {
        l[last] = (c, true, true);
    }
    Ok(l)
}

// ---- HEVCINTERCIP (LEDGER SR688): 8.3.3.2 generated reference pictures (named tail block) ----
impl Dpb {
    /// 8.3.3.2: generate an unavailable reference picture — every sample 1 << (BitDepth - 1), CuPredMode MODE_INTRA
    /// everywhere (the default motion: no temporal vector), PicOrderCntVal `poc`, PicOutputFlag 0 (never output) —
    /// marked "used for long-term reference" when `long_term`, else short-term; its POC is noted in `generated`.
    /// Returns its DPB index.
    pub fn generate_missing(&mut self, poc: i32, long_term: bool, sps: &Sps, geo: &Geometry) -> usize {
        let chroma = sps.chroma_format().unwrap_or(h264_core::ChromaFormat::Yuv420);
        let mut pic = Picture::with_chroma(sps.width as usize, sps.height as usize, geo, chroma);
        pic.planes[0].data.fill(1u16 << (sps.bit_depth_luma - 1));
        let grey_c = 1u16 << (sps.bit_depth_chroma - 1);
        pic.planes[1].data.fill(grey_c);
        pic.planes[2].data.fill(grey_c);
        pic.poc = poc;
        pic.bit_depth = sps.bit_depth_luma;
        pic.bit_depth_c = sps.bit_depth_chroma;
        pic.output = false; if let Some(s) = self.nearest_held(poc) { self.conceal_from(&mut pic, s); }
        let marking = if long_term { Marking::Long } else { Marking::Short };
        self.pics.push(DpbPic { pic, output_needed: false, latency: 0, marking });
        self.generated.push(poc);
        self.pics.len() - 1
    }
}

// ---- HEVCERRORCONCEAL (LEDGER SR700): a generated reference concealed from the nearest held picture (named tail block) ----
impl Dpb {
    /// HEVCERRORCONCEAL (SR700): the DPB index of the held picture nearest PicOrderCntVal `poc` (the smallest
    /// |PicOrderCntVal - poc|, the lower PicOrderCntVal on a tie), among the decoded pictures when any is held, else
    /// among the generated ones (a picture no slice was decoded into: only 8.3.3.2 stores one); `None` when the DPB is
    /// empty.
    pub fn nearest_held(&self, poc: i32) -> Option<usize> {
        let decoded = self.pics.iter().any(|d| !d.pic.slices.is_empty());
        (0..self.pics.len())
            .filter(|&i| !decoded || !self.pics[i].pic.slices.is_empty())
            .min_by_key(|&i| {
                let p = self.pics[i].pic.poc;
                ((p as i64 - poc as i64).abs(), p)
            })
    }

    /// HEVCERRORCONCEAL (SR700): conceal the generated picture `pic` from the held picture `src`: its three sample
    /// planes are copied when their geometry and both bit depths equal `pic`'s, and (`pic.poc`, the source's
    /// PicOrderCntVal) is noted in `concealed`; else the grey fill stays and `conceal_misfit` counts it. The motion
    /// (MODE_INTRA, no vector), the marking and PicOutputFlag 0 are untouched.
    fn conceal_from(&mut self, pic: &mut Picture, src: usize) {
        let s = &self.pics[src].pic;
        let fits = s.bit_depth == pic.bit_depth
            && s.bit_depth_c == pic.bit_depth_c
            && s.planes.iter().zip(pic.planes.iter()).all(|(a, b)| a.w == b.w && a.h == b.h && a.data.len() == b.data.len());
        if !fits {
            self.conceal_misfit += 1;
            return;
        }
        for (dst, from) in pic.planes.iter_mut().zip(s.planes.iter()) {
            dst.data.copy_from_slice(&from.data);
        }
        let src_poc = s.poc;
        let projected = self.conceal_motion(pic, src); let bidir_blocks = bidir_blocks(pic); self.concealed.push(Concealed { poc: pic.poc, src_poc, projected, bidir_blocks });
    }
}

// ---- HEVCERRORCONCEALMOTION (LEDGER SR710): a concealed picture projected along its source's motion (named tail block) ----
/// HEVCERRORCONCEALMOTION (SR710): one concealment note — the generated picture's PicOrderCntVal, its source's, and
/// whether any of its 8x8 blocks was projected along the source's motion by [`Dpb::conceal_motion`] (else the still
/// copy of [`Dpb::conceal_from`] stands everywhere).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Concealed {
    pub poc: i32,
    pub src_poc: i32,
    pub projected: bool,
    /// HEVCCONCEALBIDIR (SR721): the 8x8 blocks projected along BOTH lists of a bi-predicted source block (averaged).
    pub bidir_blocks: u32,
}

impl Dpb {
    /// HEVCERRORCONCEALMOTION (SR710): project the source picture `src` (POC s) along its own motion into the generated
    /// picture `pic` (POC c), over the still copy `conceal_from` already made. Source S predicted block x from reference
    /// R (POC r, the block's stored `ref_poc`) by mv: S(x) = R(x + mv), so the lost picture is C(x) ~ S(x + mv'), mv' =
    /// `scale_mv(mv, s - r, c - s)` (the TMVP POC-distance rescale), kept at quarter samples (HEVCCONCEALSUBPEL SR733) and
    /// fetched from S's own planes through `luma_mc` / `chroma_mc`'s fractional filters (nearest-edge clamped), per 8x8 block read at its
    /// top-left 4x4 motion. A block keeps the still copy when it is intra (pred 0), its list is long-term, or it is
    /// intra block copy (ref_poc == s); HEVCCONCEALBIDIR (SR721): each usable list projects (`project_lists`), two are averaged by
    /// 8-262 (`conceal_fetch`), one unusable list falls back to the other. A projected block's motion becomes mv' to S, so a
    /// later picture's TMVP reads the projected vector. True when any block was projected.
    fn conceal_motion(&self, pic: &mut Picture, src: usize) -> bool {
        let s = &self.pics[src].pic;
        if s.w4 != pic.w4 || s.h4 != pic.h4 || s.motion.len() != pic.motion.len() || s.width != pic.width || s.height != pic.height {
            return false;
        }
        let (sub_w, sub_h) = {
            let (a, b) = pic.chroma.sub();
            (a as i32, b as i32)
        };
        let (w, h) = (pic.width as i32, pic.height as i32);
        let mut projected = false;
        let mut out: Vec<i32> = Vec::new();
        for y in (0..h).step_by(8) {
            for x in (0..w).step_by(8) {
                let m = s.motion[s.idx4(x as usize, y as usize)];
                let cand = project_lists(&m, s.poc, pic.poc);
                if cand == [None, None] {
                    continue;
                }
                let (bw, bh) = ((w - x).min(8), (h - y).min(8));
                for c in 0..3 {
                    let (cx, cy, cw, ch, bd) = if c == 0 {
                        (x, y, bw, bh, pic.bit_depth)
                    } else {
                        (x / sub_w, y / sub_h, bw / sub_w, bh / sub_h, pic.bit_depth_c)
                    };
                    if pic.planes[c].w == 0 || pic.planes[c].h == 0 || cw <= 0 || ch <= 0 {
                        continue;
                    }
                    out.clear();
                    out.resize((cw * ch) as usize, 0);
                    conceal_fetch(&s.planes[c], c, cx, cy, cw, ch, &cand, (sub_w, sub_h), bd, &mut out);
                    let plane = &mut pic.planes[c];
                    for j in 0..ch {
                        for i in 0..cw {
                            let (px, py) = ((cx + i) as usize, (cy + j) as usize);
                            if px < plane.w && py < plane.h {
                                plane.set(px, py, out[(j * cw + i) as usize] as u16);
                            }
                        }
                    }
                }
                let pm = conceal_pu(s.poc, &cand);
                pic.fill4(x as usize, y as usize, bw as usize, bh as usize, |p, i| p.motion[i] = pm);
                projected = true;
            }
        }
        projected
    }
}

// ---- HEVCCONCEALBIDIR (LEDGER SR721): a bi-predicted source block projected along both lists (named tail block) ----
/// HEVCCONCEALBIDIR (SR721): the projected vector of each list of source block motion `m` (source POC `s_poc`, lost POC
/// `c_poc`): a list that is used, not long-term and not intra block copy (its ref_poc is not the source's) gives
/// `scale_mv(mv, s - r, c - s)` kept at quarter samples (HEVCCONCEALSUBPEL SR733; SR710's rule, per list); an unusable list is `None`, so a block
/// with one unusable list projects along the other alone.
fn project_lists(m: &crate::picture::PuMotion, s_poc: i32, c_poc: i32) -> [Option<[i32; 2]>; 2] {
    let mut cand = [None, None];
    for (l, slot) in cand.iter_mut().enumerate() {
        if m.uses(l) && !m.ref_lt[l] && m.ref_poc[l] != s_poc {
            *slot = Some(crate::inter::scale_mv(m.mv[l], s_poc - m.ref_poc[l], c_poc - s_poc));
        }
    }
    cand
}

/// HEVCCONCEALBIDIR (SR721): the cw x ch samples of component `c` at (cx, cy) fetched from the source plane `r` along each
/// candidate of [`project_lists`] (quarter-sample luma vectors; chroma by (SubWidthC, SubHeightC) `sub` as `predict_pu`) through
/// `luma_mc` / `chroma_mc`'s fractional filters (HEVCCONCEALSUBPEL SR733), written to `out` at sample depth, clipped: two candidates averaged by inter.rs
/// `bi_default` (8-262), one rounded by inter.rs `uni_default` (8-261). A block is at most 8x8 in every plane (the stack buffer).
fn conceal_fetch(r: &crate::picture::Plane, c: usize, cx: i32, cy: i32, cw: i32, ch: i32, cand: &[Option<[i32; 2]>; 2], sub: (i32, i32), bd: u32, out: &mut [i32]) {
    let n = (cw * ch) as usize;
    let max = (1i32 << bd) - 1;
    let mut second = [0i32; 64];
    let mut got = 0;
    for mv in cand.iter().flatten() {
        let buf: &mut [i32] = if got == 0 { &mut out[..n] } else { &mut second[..n] };
        if c == 0 {
            crate::inter::luma_mc(r, cx, cy, *mv, cw, ch, bd, buf);
        } else {
            let mvc = [mv[0] * 2 / sub.0, mv[1] * 2 / sub.1];
            crate::inter::chroma_mc(r, cx, cy, mvc, cw, ch, bd, buf);
        }
        got += 1;
    }
    if got == 2 {
        for (a, &b) in out[..n].iter_mut().zip(second[..n].iter()) {
            *a = crate::inter::bi_default(*a, b, bd).clamp(0, max);
        }
    } else {
        for a in out[..n].iter_mut() {
            *a = crate::inter::uni_default(*a, bd).clamp(0, max);
        }
    }
}

/// HEVCCONCEALBIDIR (SR721): the motion a projected block stores: both candidates — pred 3, both vectors, both
/// ref_poc the source — so a later TMVP reads either; one — SR710's L0-slot motion to the source.
fn conceal_pu(s_poc: i32, cand: &[Option<[i32; 2]>; 2]) -> crate::picture::PuMotion {
    use crate::picture::PuMotion;
    match *cand {
        [Some(a), Some(b)] => PuMotion { pred: 3, ref_idx: [0, 0], mv: [a, b], ref_poc: [s_poc, s_poc], ref_lt: [false, false] },
        [Some(a), None] | [None, Some(a)] => PuMotion { pred: 1, ref_idx: [0, -1], mv: [a, [0, 0]], ref_poc: [s_poc, 0], ref_lt: [false, false] },
        [None, None] => PuMotion::default(),
    }
}

/// HEVCCONCEALBIDIR (SR721): the 8x8 blocks of the concealed picture `pic` whose top-left 4x4 motion is bi-predicted
/// (pred 3: written only by [`conceal_pu`]; a generated picture starts intra everywhere).
fn bidir_blocks(pic: &Picture) -> u32 {
    let mut n = 0;
    for y in (0..pic.height).step_by(8) {
        for x in (0..pic.width).step_by(8) {
            if pic.motion[pic.idx4(x, y)].pred == 3 {
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod hevcconcealbidir_tests {
    use super::project_lists;
    use crate::picture::PuMotion;

    fn bi(lt0: bool, ref1: i32) -> PuMotion {
        PuMotion { pred: 3, ref_idx: [0, 0], mv: [[32, 16], [-64, 0]], ref_poc: [0, ref1], ref_lt: [lt0, false] }
    }

    #[test]
    fn both_lists_project_by_their_own_poc_distance() {
        // s 2, c 3: L0 to POC 0 (td 2, tb 1) halves (32, 16) to (16, 8); L1 to POC 4 (td -2, tb 1) negates half of (-64, 0).
        assert_eq!(project_lists(&bi(false, 4), 2, 3), [Some([16, 8]), Some([32, 0])]);
    }

    #[test]
    fn a_long_term_list_falls_back_to_the_other() {
        assert_eq!(project_lists(&bi(true, 4), 2, 3), [None, Some([32, 0])]);
    }

    #[test]
    fn an_intra_block_copy_list_falls_back_to_the_other() {
        assert_eq!(project_lists(&bi(false, 2), 2, 3), [Some([16, 8]), None]);
    }
}
