// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The decoded picture buffer: picture order counts (8.2.1, types 0 / 1 / 2), reference picture
//! list initialisation (8.2.4.2.1 P, 8.2.4.2.3 B) and modification (8.2.4.3), reference picture
//! marking (8.2.5: sliding window 8.2.5.3, MMCO 1..6 8.2.5.4, IDR 8.2.5.1), frame_num gaps
//! (8.2.5.2), and picture output by the bumping process of C.4.5.3.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::picture::{Picture, RefState};
use crate::ps::{Mmco, SliceHeader, SliceType, Sps};
use crate::{Error, Frame, Result};

/// "no reference picture" in a reference list.
pub const NO_REF: usize = usize::MAX;

/// The state 8.2.1 carries from picture to picture.
#[derive(Clone, Debug, Default)]
pub struct PocState {
    prev_poc_msb: i32,
    prev_poc_lsb: i32,
    prev_frame_num_offset: i32,
    prev_frame_num: u32,
    /// The previous picture in decoding order had memory_management_control_operation 5.
    prev_mmco5: bool,
}

/// The values 8.2.1 derives for one picture, kept until the picture is finished.
#[derive(Clone, Copy, Debug, Default)]
pub struct PocResult {
    pub top: i32,
    pub bottom: i32,
    msb: i32,
    lsb: i32,
    frame_num_offset: i32,
}

impl PocState {
    /// 8.2.1.1 .. 8.2.1.3 for a frame.
    pub fn compute(&self, sps: &Sps, sh: &SliceHeader) -> PocResult {
        let max_frame_num = 1i32 << sps.log2_max_frame_num;
        let fnum = sh.frame_num as i32;
        let mut res = PocResult::default();
        match sps.poc_type {
            0 => {
                let (prev_msb, prev_lsb) = if sh.idr { (0, 0) } else { (self.prev_poc_msb, self.prev_poc_lsb) };
                let max_lsb = 1i32 << sps.log2_max_poc_lsb;
                let lsb = sh.poc_lsb as i32;
                let msb = if lsb < prev_lsb && prev_lsb - lsb >= max_lsb / 2 {
                    prev_msb + max_lsb
                } else if lsb > prev_lsb && lsb - prev_lsb > max_lsb / 2 {
                    prev_msb - max_lsb
                } else {
                    prev_msb
                };
                res.msb = msb;
                res.lsb = lsb;
                res.top = msb + lsb;
                res.bottom = res.top + sh.delta_poc_bottom;
            }
            _ => {
                let prev_off = if self.prev_mmco5 { 0 } else { self.prev_frame_num_offset };
                let off = if sh.idr {
                    0
                } else if self.prev_frame_num as i32 > fnum {
                    prev_off + max_frame_num
                } else {
                    prev_off
                };
                res.frame_num_offset = off;
                if sps.poc_type == 1 {
                    let n = sps.offset_for_ref_frame.len() as i32;
                    let mut abs = if n != 0 { off + fnum } else { 0 };
                    if sh.nal_ref_idc == 0 && abs > 0 {
                        abs -= 1;
                    }
                    let mut expected = 0;
                    if abs > 0 {
                        let delta_cycle: i32 = sps.offset_for_ref_frame.iter().sum();
                        let cycle = (abs - 1) / n;
                        let in_cycle = (abs - 1) % n;
                        expected = cycle.wrapping_mul(delta_cycle);
                        for i in 0..=in_cycle as usize {
                            expected = expected.wrapping_add(sps.offset_for_ref_frame[i]);
                        }
                    }
                    if sh.nal_ref_idc == 0 {
                        expected += sps.offset_for_non_ref_pic;
                    }
                    res.top = expected + sh.delta_poc[0];
                    res.bottom = res.top + sps.offset_for_top_to_bottom_field + sh.delta_poc[1];
                } else {
                    let t = if sh.idr {
                        0
                    } else if sh.nal_ref_idc == 0 {
                        2 * (off + fnum) - 1
                    } else {
                        2 * (off + fnum)
                    };
                    res.top = t;
                    res.bottom = t;
                }
            }
        }
        res
    }

    /// Carry a finished picture's values forward (8.2.1: prevPicOrderCntMsb / Lsb come from the
    /// previous reference picture, prevFrameNumOffset / prevFrameNum from the previous picture;
    /// after an MMCO 5 the reference picture counts as POC msb 0, lsb = its TopFieldOrderCnt after
    /// the reset, and frame_num 0).
    pub fn finish(&mut self, sps: &Sps, sh: &SliceHeader, r: &PocResult, mmco5: bool, top_after_reset: i32) {
        if sh.nal_ref_idc != 0 {
            if mmco5 {
                self.prev_poc_msb = 0;
                self.prev_poc_lsb = top_after_reset;
            } else {
                self.prev_poc_msb = r.msb;
                self.prev_poc_lsb = r.lsb;
            }
        }
        let _ = sps;
        self.prev_frame_num_offset = r.frame_num_offset;
        self.prev_frame_num = if mmco5 { 0 } else { sh.frame_num };
        self.prev_mmco5 = mmco5;
    }

    pub fn reset(&mut self) {
        *self = PocState::default();
    }
}

pub struct Dpb {
    pub pics: Vec<Picture>,
    pub size: usize,
    pub max_reorder: Option<usize>,
    /// MaxLongTermFrameIdx; None = "no long-term frame indices".
    pub max_long_term_frame_idx: Option<u32>,
}

impl Default for Dpb {
    fn default() -> Self {
        Dpb::new()
    }
}

impl Dpb {
    pub fn new() -> Dpb {
        Dpb { pics: Vec::new(), size: 16, max_reorder: None, max_long_term_frame_idx: None }
    }

    /// FrameNumWrap (8.2.4.1) of short-term frame `p` relative to the current frame_num.
    fn wrap(p: &Picture, cur_frame_num: u32, max_frame_num: u32) -> i32 {
        if p.frame_num > cur_frame_num { p.frame_num as i32 - max_frame_num as i32 } else { p.frame_num as i32 }
    }

    /// Reference picture list initialisation (8.2.4.2) followed by modification (8.2.4.3), for a
    /// P or B frame; entries are indices into `pics`, `NO_REF` past the initial list's end.
    pub fn ref_lists(&mut self, sh: &SliceHeader, cur_poc: i32, max_frame_num: u32) -> Result<[Vec<usize>; 2]> {
        let cur = sh.frame_num;
        for p in self.pics.iter_mut() {
            if p.reference == RefState::Short {
                p.frame_num_wrap = Self::wrap(p, cur, max_frame_num);
            }
        }
        let short: Vec<usize> = (0..self.pics.len()).filter(|&i| self.pics[i].reference == RefState::Short).collect();
        let mut long: Vec<usize> = (0..self.pics.len()).filter(|&i| self.pics[i].reference == RefState::Long).collect();
        long.sort_by_key(|&i| self.pics[i].long_term_frame_idx);
        let mut lists = [Vec::new(), Vec::new()];
        match sh.slice_type {
            SliceType::P => {
                let mut s = short.clone();
                s.sort_by_key(|&i| -self.pics[i].frame_num_wrap);
                lists[0] = s;
                lists[0].extend_from_slice(&long);
            }
            SliceType::B => {
                let mut before: Vec<usize> = short.iter().copied().filter(|&i| self.pics[i].poc < cur_poc).collect();
                let mut after: Vec<usize> = short.iter().copied().filter(|&i| self.pics[i].poc > cur_poc).collect();
                before.sort_by_key(|&i| -(self.pics[i].poc as i64));
                after.sort_by_key(|&i| self.pics[i].poc);
                lists[0] = before.clone();
                lists[0].extend_from_slice(&after);
                lists[0].extend_from_slice(&long);
                lists[1] = after;
                lists[1].extend_from_slice(&before);
                lists[1].extend_from_slice(&long);
                if lists[1].len() > 1 && lists[0] == lists[1] {
                    lists[1].swap(0, 1);
                }
            }
            SliceType::I => {}
        }
        for l in 0..2 {
            let n = sh.num_ref_idx_active[l] as usize;
            lists[l].resize(n, NO_REF);
            if !sh.rplm[l].is_empty() {
                self.modify(&mut lists[l], &sh.rplm[l], n, cur, max_frame_num)?;
            }
        }
        Ok(lists)
    }

    /// 8.2.4.3: modification_of_pic_nums_idc 0 / 1 (short-term by abs_diff_pic_num) and 2
    /// (long-term by long_term_pic_num), for a frame (MaxPicNum = MaxFrameNum, CurrPicNum =
    /// frame_num).
    fn modify(&self, list: &mut Vec<usize>, mods: &[(u32, u32)], n: usize, cur: u32, max_frame_num: u32) -> Result<()> {
        let max = max_frame_num as i64;
        let curn = cur as i64;
        let mut pred = curn;
        let mut ref_idx = 0usize;
        list.resize(n + 1, NO_REF);
        for &(idc, v) in mods {
            if ref_idx > n {
                return Err(Error::Invalid("too many ref_pic_list_modification operations"));
            }
            let (pic, is_long, num) = match idc {
                0 | 1 => {
                    let abs = v as i64 + 1;
                    let nowrap = if idc == 0 {
                        if pred - abs < 0 { pred - abs + max } else { pred - abs }
                    } else if pred + abs >= max {
                        pred + abs - max
                    } else {
                        pred + abs
                    };
                    pred = nowrap;
                    let num = if nowrap > curn { nowrap - max } else { nowrap };
                    let pic = (0..self.pics.len())
                        .find(|&i| self.pics[i].reference == RefState::Short && self.pics[i].frame_num_wrap as i64 == num)
                        .ok_or(Error::Invalid("ref_pic_list_modification names a missing short-term picture"))?;
                    (pic, false, num)
                }
                2 => {
                    let pic = (0..self.pics.len())
                        .find(|&i| self.pics[i].reference == RefState::Long && self.pics[i].long_term_frame_idx == v)
                        .ok_or(Error::Invalid("ref_pic_list_modification names a missing long-term picture"))?;
                    (pic, true, v as i64)
                }
                _ => return Err(Error::Invalid("modification_of_pic_nums_idc")),
            };
            for c in (ref_idx + 1..=n).rev() {
                list[c] = list[c - 1];
            }
            list[ref_idx] = pic;
            ref_idx += 1;
            let mut k = ref_idx;
            for c in ref_idx..=n {
                let e = list[c];
                let same = e != NO_REF && {
                    let p = &self.pics[e];
                    if is_long { p.reference == RefState::Long && p.long_term_frame_idx as i64 == num } else { p.reference == RefState::Short && p.frame_num_wrap as i64 == num }
                };
                if !same {
                    list[k] = e;
                    k += 1;
                }
            }
        }
        list.truncate(n);
        Ok(())
    }

    fn count_refs(&self) -> usize {
        self.pics.iter().filter(|p| p.is_ref()).count()
    }

    /// 8.2.5.3: when the reference frames fill max_num_ref_frames, the short-term frame with the
    /// smallest FrameNumWrap is marked unused.
    pub fn sliding_window(&mut self, sps: &Sps, cur_frame_num: u32) {
        let max = 1u32 << sps.log2_max_frame_num;
        while self.count_refs() >= (sps.max_num_ref_frames as usize).max(1) {
            let victim = (0..self.pics.len()).filter(|&i| self.pics[i].reference == RefState::Short).min_by_key(|&i| Self::wrap(&self.pics[i], cur_frame_num, max));
            match victim {
                Some(i) => self.pics[i].reference = RefState::Unused,
                None => break,
            }
        }
    }

    /// Reference picture marking (8.2.5) of the finished picture `cur`.
    pub fn mark(&mut self, cur: &mut Picture, sh: &SliceHeader, sps: &Sps) {
        let max = 1u32 << sps.log2_max_frame_num;
        if sh.nal_ref_idc == 0 {
            cur.reference = RefState::Unused;
            return;
        }
        if sh.idr {
            for p in self.pics.iter_mut() {
                p.reference = RefState::Unused;
            }
            if sh.long_term_reference {
                cur.reference = RefState::Long;
                cur.long_term_frame_idx = 0;
                self.max_long_term_frame_idx = Some(0);
            } else {
                cur.reference = RefState::Short;
                self.max_long_term_frame_idx = None;
            }
            return;
        }
        let mut cur_long = false;
        if sh.adaptive_marking {
            for m in &sh.mmcos {
                match *m {
                    Mmco::ForgetShort(d) => {
                        let num = sh.frame_num as i64 - (d as i64 + 1);
                        if let Some(p) = self.pics.iter_mut().find(|p| p.reference == RefState::Short && Self::wrap(p, sh.frame_num, max) as i64 == num) {
                            p.reference = RefState::Unused;
                        }
                    }
                    Mmco::ForgetLong(n) => {
                        if let Some(p) = self.pics.iter_mut().find(|p| p.reference == RefState::Long && p.long_term_frame_idx == n) {
                            p.reference = RefState::Unused;
                        }
                    }
                    Mmco::ShortToLong(d, idx) => {
                        let num = sh.frame_num as i64 - (d as i64 + 1);
                        let target = self.pics.iter().position(|p| p.reference == RefState::Short && Self::wrap(p, sh.frame_num, max) as i64 == num);
                        if let Some(t) = target {
                            for (i, p) in self.pics.iter_mut().enumerate() {
                                if i != t && p.reference == RefState::Long && p.long_term_frame_idx == idx {
                                    p.reference = RefState::Unused;
                                }
                            }
                            self.pics[t].reference = RefState::Long;
                            self.pics[t].long_term_frame_idx = idx;
                        }
                    }
                    Mmco::MaxLongIdx(v) => {
                        self.max_long_term_frame_idx = if v == 0 { None } else { Some(v - 1) };
                        for p in self.pics.iter_mut() {
                            if p.reference == RefState::Long && self.max_long_term_frame_idx.is_none_or(|m| p.long_term_frame_idx > m) {
                                p.reference = RefState::Unused;
                            }
                        }
                    }
                    Mmco::Reset => {
                        for p in self.pics.iter_mut() {
                            p.reference = RefState::Unused;
                        }
                        self.max_long_term_frame_idx = None;
                        cur.mmco5 = true;
                    }
                    Mmco::CurrentToLong(idx) => {
                        for p in self.pics.iter_mut() {
                            if p.reference == RefState::Long && p.long_term_frame_idx == idx {
                                p.reference = RefState::Unused;
                            }
                        }
                        cur.reference = RefState::Long;
                        cur.long_term_frame_idx = idx;
                        cur_long = true;
                    }
                }
            }
        } else {
            self.sliding_window(sps, sh.frame_num);
        }
        if !cur_long {
            cur.reference = RefState::Short;
        }
        // A non-conforming stream may exceed max_num_ref_frames: drop the oldest short-term frames.
        let limit = (sps.max_num_ref_frames as usize).max(1);
        while self.count_refs() + 1 > limit {
            let victim = (0..self.pics.len()).filter(|&i| self.pics[i].reference == RefState::Short).min_by_key(|&i| Self::wrap(&self.pics[i], sh.frame_num, max));
            match victim {
                Some(i) => self.pics[i].reference = RefState::Unused,
                None => break,
            }
        }
    }

    /// Drop frames neither waiting for output nor used for reference.
    pub fn prune(&mut self) {
        self.pics.retain(|p| p.output_needed || p.is_ref());
    }

    /// C.4.5.3 "bumping": output the frame with the smallest PicOrderCnt that waits for output.
    /// Returns false when nothing waits.
    pub fn bump(&mut self, out: &mut VecDeque<Frame>) -> bool {
        let Some(i) = (0..self.pics.len()).filter(|&i| self.pics[i].output_needed).min_by_key(|&i| self.pics[i].poc) else { return false };
        out.push_back(Frame::from_picture(&self.pics[i]));
        self.pics[i].output_needed = false;
        if !self.pics[i].is_ref() {
            self.pics.remove(i);
        }
        true
    }

    /// Output everything (end of stream, or before an IDR / MMCO 5 picture, C.4.4).
    pub fn flush(&mut self, out: &mut VecDeque<Frame>) {
        while self.bump(out) {}
        self.prune();
    }

    /// Store a finished, marked picture (C.4.5.1 / C.4.5.2), bumping as the buffer requires.
    pub fn store(&mut self, cur: Picture, out: &mut VecDeque<Frame>) {
        self.prune();
        if !cur.is_ref() && self.pics.len() >= self.size {
            let min_waiting = self.pics.iter().filter(|p| p.output_needed).map(|p| p.poc).min();
            if cur.output_needed && min_waiting.is_none_or(|m| cur.poc < m) {
                out.push_back(Frame::from_picture(&cur));
                return;
            }
        }
        while self.pics.len() >= self.size {
            if !self.bump(out) {
                // Every frame is a reference frame already output: the stream overfills its DPB.
                break;
            }
            self.prune();
        }
        if !cur.is_ref() && !cur.output_needed {
            return;
        }
        self.pics.push(cur);
        if let Some(r) = self.max_reorder {
            while self.pics.iter().filter(|p| p.output_needed).count() > r {
                if !self.bump(out) {
                    break;
                }
            }
        }
    }
}
