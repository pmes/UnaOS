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
    // ---- HEVCCONCEALFORWARD (LEDGER SR754): the concealed pictures awaiting their first referencer (named struct-tail block) ----
    /// HEVCCONCEALFORWARD (SR754): the PicOrderCntVal of every picture [`Dpb::conceal_from`] concealed, until
    /// [`Dpb::conceal_forward`] refines it at each later picture end from that picture's motion; HEVCCONCEALFORWARDDEPTH (SR1073): with one refine count per 8x8 block, kept while it is in the DPB and some block's count < [`CONCEAL_FORWARD_DEPTH`].
    pub forward: Vec<(i32, Vec<u8>)>,
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
        let projected = self.conceal_motion(pic, src, None, &mut []); let bidir_blocks = bidir_blocks(pic); self.concealed.push(Concealed { poc: pic.poc, src_poc, projected, bidir_blocks, forward_blocks: 0 }); self.forward.push((pic.poc, depth_counts(pic)));
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
    /// HEVCCONCEALFORWARD (SR754): the 8x8 blocks re-projected from the first referencer's motion by [`Dpb::conceal_forward`]; HEVCCONCEALFORWARDDEPTH (SR1073): refined by at least one referencer.
    pub forward_blocks: u32,
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
    /// later picture's TMVP reads the projected vector. HEVCCONCEALWEIGHTED (SR741): a source coded with explicit weights (its
    /// slice's kept table, `Picture::slice_weights`, SR1067) weights each fetch by the source block's (w, o, log2WD) for that list and ref_idx. True when any block was projected.
    /// HEVCCONCEALFORWARDDEPTH (SR1073): `depth` (empty: no count) holds one refine count per block: a block at [`CONCEAL_FORWARD_DEPTH`] is skipped, a block at 1 is averaged with its current samples ([`avg_with_current`]), each projected block counts one more.
    fn conceal_motion(&self, pic: &mut Picture, src: usize, only: Option<i32>, depth: &mut [u8]) -> bool {
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
        for y in (0..h).step_by(CONCEAL_BLOCK) {
            for x in (0..w).step_by(CONCEAL_BLOCK) {
                let m = s.motion[s.idx4(x as usize, y as usize)];
                let cand = forward_only(project_lists(&m, s.poc, pic.poc), &m, only); let slot = depth_slot(x, y, w); let seen = depth.get(slot).copied().unwrap_or(0);
                if cand == [None, None] || seen >= CONCEAL_FORWARD_DEPTH {
                    continue;
                }
                let (bw, bh) = ((w - x).min(CONCEAL_BLOCK as i32), (h - y).min(CONCEAL_BLOCK as i32));
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
                    conceal_fetch(&s.planes[c], c, cx, cy, cw, ch, &cand, (sub_w, sub_h), bd, s.slice_weights(x as usize, y as usize).map(|(pw, high)| [0, 1].map(|l| scale_weight(crate::inter::explicit_weight_of(Some(pw), c, l, m.ref_idx[l], bd, *high), if only.is_some() { (1, 1) } else { conceal_poc_pair(s.poc, m.ref_poc[l], pic.poc) }, bd, *high))), only.is_some(), &mut out);
                    let plane = &mut pic.planes[c];
                    for j in 0..ch {
                        for i in 0..cw {
                            let (px, py) = ((cx + i) as usize, (cy + j) as usize);
                            if px < plane.w && py < plane.h {
                                let v = avg_with_current(plane.at(px, py), out[(j * cw + i) as usize], seen); plane.set(px, py, v);
                            }
                        }
                    }
                }
                let pm = conceal_pu(s.poc, &cand);
                pic.fill4(x as usize, y as usize, bw as usize, bh as usize, |p, i| p.motion[i] = pm);
                projected = true; if let Some(n) = depth.get_mut(slot) { *n += 1; }
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
            *slot = Some({ let (tb, td) = conceal_poc_pair(s_poc, m.ref_poc[l], c_poc); crate::inter::scale_mv(m.mv[l], td, tb) });
        }
    }
    cand
}

/// HEVCCONCEALBIDIR (SR721): the cw x ch samples of component `c` at (cx, cy) fetched from the source plane `r` along each
/// candidate of [`project_lists`] (quarter-sample luma vectors; chroma by (SubWidthC, SubHeightC) `sub` as `predict_pu`) through
/// `luma_mc` / `chroma_mc`'s fractional filters (HEVCCONCEALSUBPEL SR733), written to `out` at sample depth, clipped: two candidates averaged by inter.rs
/// `bi_default` (8-262), one rounded by inter.rs `uni_default` (8-261); HEVCCONCEALWEIGHTED (SR741): with `wp` (the source's explicit
/// (w, o, log2WD) per list) through inter.rs `bi_explicit` / `uni_explicit` (8.5.3.3.4.3) instead; HEVCCONCEALBIDIRECTFORWARD (SR778): with `inv` (the forward road: `wp` is the referencer's) one candidate through inter.rs `uni_explicit_inv`, two (both lists on L) through inter.rs `bi_explicit_inv` over the averaged fetch (HEVCCONCEALFORWARDBIINVERSE SR784: the equal-vector joint inverse; HEVC-CONCEAL-FORWARD-BI-INVERSE-SPLIT owed), unweighted by `bi_default`. A block is at most 8x8 in every plane (the stack buffer).
fn conceal_fetch(r: &crate::picture::Plane, c: usize, cx: i32, cy: i32, cw: i32, ch: i32, cand: &[Option<[i32; 2]>; 2], sub: (i32, i32), bd: u32, wp: Option<[(i32, i32, u32); 2]>, inv: bool, out: &mut [i32]) {
    let n = (cw * ch) as usize;
    let max = (1i32 << bd) - 1;
    let mut second = [0i32; CONCEAL_BLOCK_SAMPLES];
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
            *a = match wp { Some(w) if inv => crate::inter::bi_explicit_inv((*a + b + 1) >> 1, w[0], w[1], bd), Some(w) => crate::inter::bi_explicit(*a, b, w[0], w[1]), None => crate::inter::bi_default(*a, b, bd) }.clamp(0, max);
        }
    } else {
        for a in out[..n].iter_mut() {
            *a = match wp { Some(w) if inv => crate::inter::uni_explicit_inv(*a, w[cand[0].is_none() as usize], bd), Some(w) => crate::inter::uni_explicit(*a, w[cand[0].is_none() as usize]), None => crate::inter::uni_default(*a, bd) }.clamp(0, max);
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
    for y in (0..pic.height).step_by(CONCEAL_BLOCK) {
        for x in (0..pic.width).step_by(CONCEAL_BLOCK) {
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

// ---- HEVCCONCEALWEIGHTED (LEDGER SR741): the source picture's explicit weights kept for concealment (named tail block) ----
/// HEVCCONCEALWEIGHTED (SR741): keep the pred_weight_table of each independent slice segment of picture `pic` (slice.rs
/// parses it only under weighted_pred_flag for P / weighted_bipred_flag for B, the gate `predict_pu` reads; None without
/// one), with the SPS's high_precision_offsets_enabled_flag, so `conceal_motion` weights a projection from `pic` as its own
/// PUs were weighted. HEVCCONCEALWEIGHTEDSLICES (SR1067): one entry per independent segment, pushed beside its `slices`
/// entry (decoder.rs `decode_segment`), read per block by `Picture::slice_weights`; a dependent segment inherits its slice's.
pub(crate) fn keep_weights(pic: &mut Picture, sh: &SliceHeader, sps: &Sps) {
    pic.weights.push(sh.pred_weights.clone().map(|w| (w, sps.range_ext.high_precision_offsets_enabled_flag)));
}

// ---- HEVCCONCEALFORWARD (LEDGER SR754): a concealed picture refined from its first referencer's motion (named tail block) ----
impl Dpb {
    /// HEVCCONCEALFORWARD (SR754): at a picture end, after `store` put the finished picture C last, re-project every
    /// picture L waiting in `forward` (concealed since the previous picture end) from C's own motion: per 8x8 block of C
    /// whose list references L, [`Dpb::conceal_motion`] with `only` L fetches C's reconstructed planes at
    /// `scale_mv(mv, c - l, l - c)` = -mv (`project_lists`; distScaleFactor -256 for every td) and stores -mv to C as the
    /// block's motion; every other block of L keeps its projection from the source S. C is only read: its samples (which
    /// read L's S-projection) never change, so only pictures decoded after C read the refined L. HEVCCONCEALFORWARDDEPTH (SR1073): `forward`
    /// keeps L (one count per block) while L is in the DPB and a block is under [`CONCEAL_FORWARD_DEPTH`]: each later referencer refines the blocks it covers (the second averaged per block), a picture not referencing L leaves it waiting. C's
    /// explicit weights are inverted (they map L to C: HEVCCONCEALBIDIRECTFORWARD SR778, `conceal_fetch`'s `inv`). The untaken [`Concealed`]
    /// note of L gets `forward_blocks` (the blocks refined at least once; a note taken before C's end keeps 0). Returns the blocks refined.
    pub fn conceal_forward(&mut self) -> u32 {
        let Some(c) = self.pics.len().checked_sub(1) else {
            return 0;
        };
        let c_poc = self.pics[c].pic.poc;
        let mut total = 0;
        for (poc, mut counts) in core::mem::take(&mut self.forward) {
            let Some(l) = (0..c).find(|&i| self.pics[i].pic.poc == poc && self.pics[i].pic.slices.is_empty()) else {
                continue;
            };
            let mut d = self.pics.remove(l);
            let n = if self.conceal_motion(&mut d.pic, c - 1, Some(poc), &mut counts) { forward_blocks(&d.pic, c_poc) } else { 0 };
            self.pics.insert(l, d);
            if let Some(r) = self.concealed.iter_mut().rev().find(|r| r.poc == poc) {
                r.forward_blocks = refined(&counts);
            }
            total += n;
            if counts.iter().any(|&k| k < CONCEAL_FORWARD_DEPTH) { self.forward.push((poc, counts)); }
        }
        total
    }
}

/// HEVCCONCEALFORWARD (SR754): with `only` (the lost picture's PicOrderCntVal) keep the candidates of [`project_lists`]
/// whose list references that picture (the source's other lists keep the earlier projection); `None` keeps all (SR710).
fn forward_only(mut cand: [Option<[i32; 2]>; 2], m: &crate::picture::PuMotion, only: Option<i32>) -> [Option<[i32; 2]>; 2] {
    if let Some(l) = only {
        for (k, slot) in cand.iter_mut().enumerate() {
            if m.ref_poc[k] != l {
                *slot = None;
            }
        }
    }
    cand
}

/// HEVCCONCEALFORWARD (SR754): the 8x8 blocks of the refined picture `pic` whose top-left 4x4 motion is inter with
/// ref_poc[0] the referencer's `c_poc` (written only by [`conceal_pu`] from [`Dpb::conceal_forward`]).
fn forward_blocks(pic: &Picture, c_poc: i32) -> u32 {
    let mut n = 0;
    for y in (0..pic.height).step_by(CONCEAL_BLOCK) {
        for x in (0..pic.width).step_by(CONCEAL_BLOCK) {
            let m = pic.motion[pic.idx4(x, y)];
            if m.pred != 0 && m.ref_poc[0] == c_poc {
                n += 1;
            }
        }
    }
    n
}

// ---- HEVCCONCEALBI444 (LEDGER SR845): the concealment block's sample bound (named tail block) ----
/// HEVCCONCEALBI444 (SR845): the most samples [`conceal_fetch`] fetches per plane per candidate, the size of its second
/// candidate's stack buffer. [`Dpb::conceal_motion`] walks the picture in 8x8 blocks (`step_by(CONCEAL_BLOCK)`, `min(CONCEAL_BLOCK)`) on every
/// chroma_format_idc, so a block is 8x8 = 64 luma samples and (8 / SubWidthC) x (8 / SubHeightC) chroma samples: 16 at
/// 4:2:0, 32 at 4:2:2, 64 at 4:4:4 (idc 0 has no chroma plane). The bound is the decoder's block step, never a
/// bitstream value (a CU's size does not enter), so a 4:4:4 bi-predicted block fills the whole buffer and no stream can
/// exceed it. The step and this bound move together (HEVCCONCEALBLOCKSTEPCONST SR1003: both read [`CONCEAL_BLOCK`]).
const CONCEAL_BLOCK_SAMPLES: usize = CONCEAL_BLOCK * CONCEAL_BLOCK;

// ---- HEVCCONCEALBLOCKSTEPCONST (LEDGER SR1003): the concealment block's side (named tail block) ----
/// HEVCCONCEALBLOCKSTEPCONST (SR1003): the side of the square block [`Dpb::conceal_motion`] walks and projects, and
/// [`bidir_blocks`] / [`forward_blocks`] count, on every chroma_format_idc; [`CONCEAL_BLOCK_SAMPLES`] is its square, so
/// the step and the stack buffer cannot drift (a 16x16 block is this one edit: HEVC-CONCEAL-BLOCK-16 owed).
const CONCEAL_BLOCK: usize = 8;

// ---- HEVCCONCEALWEIGHTEDDISTANCE (LEDGER SR1056): source weights scaled by the POC distance ratio (named tail block) ----
/// HEVCCONCEALWEIGHTEDDISTANCE (SR1056): the ONE POC pair a concealment candidate is scaled by — (c - s, s - r) for lost
/// picture `c_poc`, source `s_poc` and the source block's reference `ref_poc`: [`project_lists`] hands it to `scale_mv`
/// (tb, td) and [`Dpb::conceal_motion`] to [`scale_weight`] (num, den), so the vector and the weights read one ratio.
fn conceal_poc_pair(s_poc: i32, ref_poc: i32, c_poc: i32) -> (i32, i32) {
    (c_poc - s_poc, s_poc - ref_poc)
}

/// HEVCCONCEALWEIGHTEDDISTANCE (SR1056): the source's explicit (w, o, log2WD) (inter.rs `explicit_weight_of`, coded for s
/// predicted from r) rescaled to the lost picture's distance by the ratio num / den = (c - s) / (s - r) of
/// [`conceal_poc_pair`]: a fade of constant per-picture gain f has g = w / 2^denom = f^(s - r) (denom = log2WD - shift1,
/// shift1 = Max(2, 14 - bitDepth)), so the GAIN scales in the log domain, g' = 2^(log2(g) num / den) through
/// [`log2_q16`] / [`exp2_q16`], w' = g' 2^denom rounded and clamped to 7.4.7.3's weight range [2^denom - 128,
/// 2^denom + 127] (a non-positive weight is kept: it has no log); the OFFSET (already shifted by WpOffsetBdShift, so the
/// shift stays exact) scales linearly, o' = o num / den rounded half away from zero, clamped to 7.4.7.3's half range in
/// that shifted domain [-(half << sh), (half - 1) << sh] (half 1 << (bitDepth - 1) under high_precision_offsets_enabled_flag
/// `high`, else 128; sh 0 or bitDepth - 8: HEVCWPOFFSETRANGE SR1043's bound; HEVC-WP-HALF-RANGE-ONE owed). log2WD is kept.
/// num == den and den == 0 return the triple unchanged; the forward road passes (1, 1) (its ratio is always -1 and
/// `conceal_fetch`'s `inv` already undoes the coded weights at that distance). Chroma rides the same call (c = 1 / 2).
/// Round trip of the Q16 gain against the true g^r (tests/inter.rs `conceal_weighted_distance_log_error_table`):
///
/// | g (Q16)       | r 1/2              | r 2                 | r 3                 |
/// |---------------|--------------------|---------------------|---------------------|
/// | 0.5 (32768)   | 46431 (+0.194%)    | 16384 (0)           | 8192 (0)            |
/// | 0.8 (52429)   | 58530 (-0.149%)    | 41973 (+0.071%)     | 33571 (+0.048%)     |
/// | 1.25 (81920)  | 73356 (+0.115%)    | 102234 (-0.162%)    | 127932 (-0.053%)    |
/// | 2 (131072)    | 92862 (+0.194%)    | 262144 (0)          | 524288 (0)          |
pub fn scale_weight((w, o, lwd): (i32, i32, u32), (num, den): (i32, i32), bd: u32, high: bool) -> (i32, i32, u32) {
    if num == den || den == 0 {
        return (w, o, lwd);
    }
    let round = |n: i64, d: i64| {
        let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
        if n >= 0 { (n + d / 2) / d } else { -((-n + d / 2) / d) }
    };
    let shift1 = (14 - bd as i32).max(2) as u32;
    let base = 1i64 << lwd.saturating_sub(shift1);
    let w2 = if w > 0 {
        let x = (((w as i64) << 16) / base) as u32;
        let g = exp2_q16(round(log2_q16(x) * num as i64, den as i64)) as i64;
        ((g * base + 32768) >> 16).clamp(base - 128, base + 127) as i32
    } else {
        w
    };
    let (half, sh) = if high { (1i64 << (bd - 1), 0) } else { (128i64, bd - 8) };
    let o2 = round(o as i64 * num as i64, den as i64).clamp(-(half << sh), (half - 1) << sh) as i32;
    (w2, o2, lwd)
}

/// HEVCCONCEALWEIGHTEDDISTANCE (SR1056): log2 of the Q16 value `x` (65536 = 1.0; 0 read as 1) in Q16, table-free: x =
/// 2^n m with the mantissa m normalised to [sqrt(1/2), sqrt(2)), then log2 m = (2 / ln 2) atanh(s), s = (m - 1) / (m + 1)
/// (|s| <= 0.172), to second order in s^2: (2 / ln 2)(s + s^3 / 3); 189098 is 2 / ln 2 in Q16. Error under 2e-4.
pub fn log2_q16(x: u32) -> i64 {
    let x = x.max(1) as i64;
    let mut n = 63 - (x as u64).leading_zeros() as i64;
    let mut m = if n >= 16 { x >> (n - 16) } else { x << (16 - n) };
    if m >= 92682 {
        m >>= 1;
        n += 1;
    }
    let s = ((m - 65536) << 16) / (m + 65536);
    ((n - 16) << 16) + 189098 * (s + s * s * s / (3i64 << 32)) / 65536
}

/// HEVCCONCEALWEIGHTEDDISTANCE (SR1056): 2^(y / 65536) in Q16, table-free: y = 65536 i + f with f in [-0.5, 0.5) (Q16),
/// 2^f by the second-order minimax fit 1 + 0.70297 f + 0.23987 f^2 (46070, 15720 in Q16; relative error under 0.2%),
/// shifted by i; saturates at u32::MAX, 0 below 2^-31.
pub fn exp2_q16(y: i64) -> u32 {
    let i = (y + 32768) >> 16;
    let f = y - (i << 16);
    let m = 65536 + (46070 * f + 15720 * f * f / 65536) / 65536;
    if i >= 16 {
        u32::MAX
    } else if i >= 0 {
        (m << i).min(u32::MAX as i64) as u32
    } else if i > -32 {
        (m >> -i) as u32
    } else {
        0
    }
}

// ---- HEVCCONCEALFORWARDDEPTH (LEDGER SR1073): a concealed picture refined by up to two referencers (named tail block) ----
/// HEVCCONCEALFORWARDDEPTH (SR1073): how many referencers refine one 8x8 block of a concealed picture ([`Dpb::conceal_forward`]):
/// the first takes its projection outright, the second is averaged with it, a third does nothing.
const CONCEAL_FORWARD_DEPTH: u8 = 2;

/// HEVCCONCEALFORWARDDEPTH (SR1073): one zero refine count per [`CONCEAL_BLOCK`] block of `pic` (ceil(w / 8) x ceil(h / 8)).
fn depth_counts(pic: &Picture) -> Vec<u8> {
    alloc::vec![0; pic.width.div_ceil(CONCEAL_BLOCK) * pic.height.div_ceil(CONCEAL_BLOCK)]
}

/// HEVCCONCEALFORWARDDEPTH (SR1073): the count slot of the block at luma (x, y) in a picture `w` samples wide.
fn depth_slot(x: i32, y: i32, w: i32) -> usize {
    (y as usize / CONCEAL_BLOCK) * (w as usize).div_ceil(CONCEAL_BLOCK) + x as usize / CONCEAL_BLOCK
}

/// HEVCCONCEALFORWARDDEPTH (SR1073): the sample a projection `new` writes over the current sample `cur` of a block refined
/// `seen` times: `new` outright when unrefined, else the equal-weight average (cur + new + 1) >> 1 (both already clipped).
fn avg_with_current(cur: u16, new: i32, seen: u8) -> u16 {
    if seen == 0 { new as u16 } else { ((cur as i32 + new + 1) >> 1) as u16 }
}

/// HEVCCONCEALFORWARDDEPTH (SR1073): the blocks refined at least once.
fn refined(counts: &[u8]) -> u32 {
    counts.iter().filter(|&&k| k > 0).count() as u32
}
