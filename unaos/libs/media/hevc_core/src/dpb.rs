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
use crate::ps::Sps;
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
}

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
    /// not in the DPB is an error (nothing is marked then); missing Foll pictures are allowed.
    pub fn apply_rps(&mut self, sh: &SliceHeader, sps: &Sps, poc: i32, irap_no_rasl: bool) -> Result<Rps> {
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
            let i = find_lt(&self.pics, e).ok_or(Error::Invalid("reference picture set names a missing picture (8.3.2)"))?;
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
            keep_st.push(find_st(&self.pics, p).ok_or(Error::Invalid("reference picture set names a missing picture (8.3.2)"))?);
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
        Ok(Rps { st_curr_before: before, st_curr_after: after, lt_curr: lt_curr_poc })
    }

    fn ref_index(&self, poc: i32) -> Option<usize> {
        self.pics.iter().position(|d| d.is_ref() && d.pic.poc == poc)
    }

    /// 8.3.4: RefPicList0 (and RefPicList1 for a B slice) of slice `sh` from the picture's set `rps`.
    pub fn ref_lists(&self, rps: &Rps, sh: &SliceHeader) -> Result<[RefList; 2]> {
        let total = rps.st_curr_before.len() + rps.st_curr_after.len() + rps.lt_curr.len();
        if total == 0 {
            return Err(Error::Invalid("NumPicTotalCurr is 0 in a P or B slice"));
        }
        let build = |first: &[i32], second: &[i32], active: u32, entries: &Option<Vec<u32>>| -> Result<RefList> {
            let n = (active as usize).max(total);
            let mut temp: Vec<(i32, bool)> = Vec::with_capacity(n);
            while temp.len() < n {
                for &p in first.iter().chain(second.iter()) {
                    if temp.len() < n {
                        temp.push((p, false));
                    }
                }
                for &p in rps.lt_curr.iter() {
                    if temp.len() < n {
                        temp.push((p, true));
                    }
                }
            }
            let mut l = RefList::default();
            for r in 0..active as usize {
                let e = match entries {
                    Some(v) => *v.get(r).ok_or(Error::Invalid("list_entry"))? as usize,
                    None => r,
                };
                let &(p, lt) = temp.get(e).ok_or(Error::Invalid("list_entry beyond the reference picture set"))?;
                l.dpb.push(self.ref_index(p).ok_or(Error::Invalid("reference picture set names a missing picture (8.3.2)"))?);
                l.poc.push(p);
                l.long_term.push(lt);
            }
            Ok(l)
        };
        let l0 = build(&rps.st_curr_before, &rps.st_curr_after, sh.num_ref_idx_l0_active, &sh.list_entry_l0)?;
        let l1 = if sh.slice_type == SliceType::B {
            build(&rps.st_curr_after, &rps.st_curr_before, sh.num_ref_idx_l1_active, &sh.list_entry_l1)?
        } else {
            RefList::default()
        };
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
