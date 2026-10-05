// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The line-diff engine, a Rust port of libxdiff as git ships it (xprepare.c, xdiffi.c,
//! xhistogram.c, xemit.c — LibXDiff by Davide Libenzi, LGPL-2.1-or-later; the histogram
//! algorithm from JGit, EDL-1.0). Ported, not reinvented, because the claim is BYTE-EQUALITY with
//! `git diff`: the same line classes, the same prefix/suffix trim and "discard lines that cannot
//! match" pre-pass, the same Myers divide-and-conquer with its cost heuristics
//! (`XDL_SNAKE_CNT`, `XDL_HEUR_MIN_COST`, `mxcost = max(256, bogosqrt(ndiags))`), the same group
//! sliding with the indent heuristic (Michael Haggerty's weights), the same hunk grouping and
//! function-name rule (`def_ff`, carried across hunks).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

/// The diff algorithm (`diff.algorithm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Algorithm {
    /// Myers with git's heuristics (the default).
    #[default]
    Myers,
    /// Myers without the cost cut-offs (`--minimal`).
    Minimal,
    /// Histogram diff (`--histogram`).
    Histogram,
}

/// Split into records, each including its `\n` (the last may lack one).
pub fn split_lines(d: &[u8]) -> Vec<&[u8]> {
    let mut v = Vec::new();
    let mut s = 0;
    for (i, &b) in d.iter().enumerate() {
        if b == b'\n' {
            v.push(&d[s..=i]);
            s = i + 1;
        }
    }
    if s < d.len() {
        v.push(&d[s..]);
    }
    v
}

/// One side of a diff: its records, their line classes and the change marks.
pub struct Side<'a> {
    /// Records.
    pub recs: Vec<&'a [u8]>,
    /// Class id per record (equal content ⇔ equal class).
    pub ha: Vec<usize>,
    /// Change marks with a zero sentinel at both ends: record `i` is `rchg[i + 1]`.
    pub rchg: Vec<u8>,
}

impl Side<'_> {
    #[inline]
    fn chg(&self, i: i64) -> bool {
        self.rchg[(i + 1) as usize] != 0
    }
    #[inline]
    fn set(&mut self, i: i64, v: u8) {
        self.rchg[(i + 1) as usize] = v;
    }
    fn nrec(&self) -> i64 {
        self.recs.len() as i64
    }
}

/// A computed diff of two buffers.
pub struct Env<'a> {
    /// The pre-image.
    pub a: Side<'a>,
    /// The post-image.
    pub b: Side<'a>,
}

fn classify<'a>(a: &[&'a [u8]], b: &[&'a [u8]]) -> (Vec<usize>, Vec<usize>, Vec<(usize, usize)>) {
    let mut map: BTreeMap<&'a [u8], usize> = BTreeMap::new();
    let mut counts: Vec<(usize, usize)> = Vec::new();
    let mut ha1 = Vec::with_capacity(a.len());
    for &r in a {
        let n = counts.len();
        let c = *map.entry(r).or_insert(n);
        if c == n {
            counts.push((0, 0));
        }
        counts[c].0 += 1;
        ha1.push(c);
    }
    let mut ha2 = Vec::with_capacity(b.len());
    for &r in b {
        let n = counts.len();
        let c = *map.entry(r).or_insert(n);
        if c == n {
            counts.push((0, 0));
        }
        counts[c].1 += 1;
        ha2.push(c);
    }
    (ha1, ha2, counts)
}

fn bogosqrt(mut n: i64) -> i64 {
    let mut i = 1;
    while n > 0 {
        i <<= 1;
        n >>= 2;
    }
    i
}

const MAX_COST_MIN: i64 = 256;
const HEUR_MIN_COST: i64 = 256;
const LINE_MAX: i64 = i64::MAX;
const SNAKE_CNT: i64 = 20;
const K_HEUR: i64 = 4;
const MAX_EQLIMIT: i64 = 1024;
const SIMSCAN_WINDOW: i64 = 100;
const KPDIS_RUN: i64 = 4;

/// Diff `a` against `b`: change marks after compaction (the input to [`build_script`]).
pub fn diff<'a>(a: &'a [u8], b: &'a [u8], alg: Algorithm, indent_heuristic: bool) -> Env<'a> {
    let mut env = prepare_and_diff(split_lines(a), split_lines(b), alg);
    change_compact(&mut env.a, &mut env.b, indent_heuristic);
    change_compact(&mut env.b, &mut env.a, indent_heuristic);
    env
}

fn prepare_and_diff<'a>(r1: Vec<&'a [u8]>, r2: Vec<&'a [u8]>, alg: Algorithm) -> Env<'a> {
    let (ha1, ha2, counts) = classify(&r1, &r2);
    let n1 = r1.len();
    let n2 = r2.len();
    let mut env = Env {
        a: Side { recs: r1, ha: ha1, rchg: vec![0; n1 + 2] },
        b: Side { recs: r2, ha: ha2, rchg: vec![0; n2 + 2] },
    };
    match alg {
        Algorithm::Histogram => {
            histogram(&mut env, 1, n1 as i64, 1, n2 as i64);
        }
        _ => myers(&mut env, &counts, alg == Algorithm::Minimal),
    }
    env
}

// ---------------------------------------------------------------------------------------------
// xprepare: trim ends, discard unmatchable records
// ---------------------------------------------------------------------------------------------

fn clean_mmatch(dis: &[u8], i: i64, mut s: i64, mut e: i64) -> bool {
    if i - s > SIMSCAN_WINDOW {
        s = i - SIMSCAN_WINDOW;
    }
    if e - i > SIMSCAN_WINDOW {
        e = i + SIMSCAN_WINDOW;
    }
    let (mut rdis0, mut rpdis0) = (0i64, 1i64);
    let mut r = 1;
    while i - r >= s {
        match dis[(i - r) as usize] {
            0 => rdis0 += 1,
            2 => rpdis0 += 1,
            _ => break,
        }
        r += 1;
    }
    if rdis0 == 0 {
        return false;
    }
    let (mut rdis1, mut rpdis1) = (0i64, 1i64);
    r = 1;
    while i + r <= e {
        match dis[(i + r) as usize] {
            0 => rdis1 += 1,
            2 => rpdis1 += 1,
            _ => break,
        }
        r += 1;
    }
    if rdis1 == 0 {
        return false;
    }
    rdis1 += rdis0;
    rpdis1 += rpdis0;
    rpdis1 * KPDIS_RUN < rpdis1 + rdis1
}

fn myers(env: &mut Env<'_>, counts: &[(usize, usize)], need_min: bool) {
    let n1 = env.a.nrec();
    let n2 = env.b.nrec();
    // xdl_trim_ends
    let lim = n1.min(n2);
    let mut i = 0;
    while i < lim && env.a.ha[i as usize] == env.b.ha[i as usize] {
        i += 1;
    }
    let dstart = i;
    let lim2 = lim - i;
    let mut k = 0;
    while k < lim2 && env.a.ha[(n1 - 1 - k) as usize] == env.b.ha[(n2 - 1 - k) as usize] {
        k += 1;
    }
    let dend1 = n1 - k - 1;
    let dend2 = n2 - k - 1;
    // xdl_cleanup_records
    let mut dis1 = vec![0u8; n1 as usize + 1];
    let mut dis2 = vec![0u8; n2 as usize + 1];
    let mlim1 = bogosqrt(n1).min(MAX_EQLIMIT);
    for i in dstart..=dend1 {
        let nm = counts[env.a.ha[i as usize]].1 as i64;
        dis1[i as usize] = if nm == 0 { 0 } else if nm >= mlim1 { 2 } else { 1 };
    }
    let mlim2 = bogosqrt(n2).min(MAX_EQLIMIT);
    for i in dstart..=dend2 {
        let nm = counts[env.b.ha[i as usize]].0 as i64;
        dis2[i as usize] = if nm == 0 { 0 } else if nm >= mlim2 { 2 } else { 1 };
    }
    let mut rindex1 = Vec::new();
    let mut rha1 = Vec::new();
    for i in dstart..=dend1 {
        let d = dis1[i as usize];
        if d == 1 || (d == 2 && !clean_mmatch(&dis1, i, dstart, dend1)) {
            rindex1.push(i);
            rha1.push(env.a.ha[i as usize]);
        } else {
            env.a.set(i, 1);
        }
    }
    let mut rindex2 = Vec::new();
    let mut rha2 = Vec::new();
    for i in dstart..=dend2 {
        let d = dis2[i as usize];
        if d == 1 || (d == 2 && !clean_mmatch(&dis2, i, dstart, dend2)) {
            rindex2.push(i);
            rha2.push(env.b.ha[i as usize]);
        } else {
            env.b.set(i, 1);
        }
    }
    let nreff1 = rha1.len() as i64;
    let nreff2 = rha2.len() as i64;
    let ndiags = nreff1 + nreff2 + 3;
    let mut kvdf = vec![0i64; ndiags as usize + 2];
    let mut kvdb = vec![0i64; ndiags as usize + 2];
    let off = nreff2 + 1;
    let xenv = XEnv { mxcost: bogosqrt(ndiags).max(MAX_COST_MIN), snake_cnt: SNAKE_CNT, heur_min: HEUR_MIN_COST };
    // xdl_recs_cmp, iteratively (the halves are independent).
    let mut stack: Vec<(i64, i64, i64, i64, bool)> = vec![(0, nreff1, 0, nreff2, need_min)];
    while let Some((mut off1, mut lim1, mut off2, mut lim2, nm)) = stack.pop() {
        while off1 < lim1 && off2 < lim2 && rha1[off1 as usize] == rha2[off2 as usize] {
            off1 += 1;
            off2 += 1;
        }
        while off1 < lim1 && off2 < lim2 && rha1[(lim1 - 1) as usize] == rha2[(lim2 - 1) as usize] {
            lim1 -= 1;
            lim2 -= 1;
        }
        if off1 == lim1 {
            for i in off2..lim2 {
                env.b.set(rindex2[i as usize], 1);
            }
        } else if off2 == lim2 {
            for i in off1..lim1 {
                env.a.set(rindex1[i as usize], 1);
            }
        } else {
            let spl = split(&rha1, off1, lim1, &rha2, off2, lim2, &mut kvdf, &mut kvdb, off, nm, &xenv);
            // push the second half first so the first is processed first (order is immaterial)
            stack.push((spl.i1, lim1, spl.i2, lim2, spl.min_hi));
            stack.push((off1, spl.i1, off2, spl.i2, spl.min_lo));
        }
    }
}

struct XEnv {
    mxcost: i64,
    snake_cnt: i64,
    heur_min: i64,
}

struct Split {
    i1: i64,
    i2: i64,
    min_lo: bool,
    min_hi: bool,
}

#[allow(clippy::too_many_arguments)]
fn split(
    ha1: &[usize],
    off1: i64,
    lim1: i64,
    ha2: &[usize],
    off2: i64,
    lim2: i64,
    kvdf_v: &mut [i64],
    kvdb_v: &mut [i64],
    base: i64,
    need_min: bool,
    xenv: &XEnv,
) -> Split {
    macro_rules! kf {
        ($d:expr) => {
            kvdf_v[($d + base) as usize]
        };
    }
    macro_rules! kb {
        ($d:expr) => {
            kvdb_v[($d + base) as usize]
        };
    }
    let h1 = |i: i64| ha1[i as usize];
    let h2 = |i: i64| ha2[i as usize];
    let dmin = off1 - lim2;
    let dmax = lim1 - off2;
    let fmid = off1 - off2;
    let bmid = lim1 - lim2;
    let odd = (fmid - bmid) & 1 != 0;
    let (mut fmin, mut fmax) = (fmid, fmid);
    let (mut bmin, mut bmax) = (bmid, bmid);
    kf!(fmid) = off1;
    kb!(bmid) = lim1;
    let mut ec: i64 = 1;
    loop {
        let mut got_snake = false;
        if fmin > dmin {
            fmin -= 1;
            kf!(fmin - 1) = -1;
        } else {
            fmin += 1;
        }
        if fmax < dmax {
            fmax += 1;
            kf!(fmax + 1) = -1;
        } else {
            fmax -= 1;
        }
        let mut d = fmax;
        while d >= fmin {
            let mut i1 = if kf!(d - 1) >= kf!(d + 1) { kf!(d - 1) + 1 } else { kf!(d + 1) };
            let prev1 = i1;
            let mut i2 = i1 - d;
            while i1 < lim1 && i2 < lim2 && h1(i1) == h2(i2) {
                i1 += 1;
                i2 += 1;
            }
            if i1 - prev1 > xenv.snake_cnt {
                got_snake = true;
            }
            kf!(d) = i1;
            if odd && bmin <= d && d <= bmax && kb!(d) <= i1 {
                return Split { i1, i2, min_lo: true, min_hi: true };
            }
            d -= 2;
        }
        if bmin > dmin {
            bmin -= 1;
            kb!(bmin - 1) = LINE_MAX;
        } else {
            bmin += 1;
        }
        if bmax < dmax {
            bmax += 1;
            kb!(bmax + 1) = LINE_MAX;
        } else {
            bmax -= 1;
        }
        let mut d = bmax;
        while d >= bmin {
            let mut i1 = if kb!(d - 1) < kb!(d + 1) { kb!(d - 1) } else { kb!(d + 1) - 1 };
            let prev1 = i1;
            let mut i2 = i1 - d;
            while i1 > off1 && i2 > off2 && h1(i1 - 1) == h2(i2 - 1) {
                i1 -= 1;
                i2 -= 1;
            }
            if prev1 - i1 > xenv.snake_cnt {
                got_snake = true;
            }
            kb!(d) = i1;
            if !odd && fmin <= d && d <= fmax && i1 <= kf!(d) {
                return Split { i1, i2, min_lo: true, min_hi: true };
            }
            d -= 2;
        }
        if need_min {
            ec += 1;
            continue;
        }
        if got_snake && ec > xenv.heur_min {
            let mut best = 0;
            let mut res = (0, 0);
            let mut d = fmax;
            while d >= fmin {
                let dd = if d > fmid { d - fmid } else { fmid - d };
                let i1 = kf!(d);
                let i2 = i1 - d;
                let v = (i1 - off1) + (i2 - off2) - dd;
                if v > K_HEUR * ec
                    && v > best
                    && off1 + xenv.snake_cnt <= i1
                    && i1 < lim1
                    && off2 + xenv.snake_cnt <= i2
                    && i2 < lim2
                {
                    let mut k = 1;
                    while h1(i1 - k) == h2(i2 - k) {
                        if k == xenv.snake_cnt {
                            best = v;
                            res = (i1, i2);
                            break;
                        }
                        k += 1;
                    }
                }
                d -= 2;
            }
            if best > 0 {
                return Split { i1: res.0, i2: res.1, min_lo: true, min_hi: false };
            }
            best = 0;
            let mut d = bmax;
            while d >= bmin {
                let dd = if d > bmid { d - bmid } else { bmid - d };
                let i1 = kb!(d);
                let i2 = i1 - d;
                let v = (lim1 - i1) + (lim2 - i2) - dd;
                if v > K_HEUR * ec
                    && v > best
                    && off1 < i1
                    && i1 <= lim1 - xenv.snake_cnt
                    && off2 < i2
                    && i2 <= lim2 - xenv.snake_cnt
                {
                    let mut k = 0;
                    while h1(i1 + k) == h2(i2 + k) {
                        if k == xenv.snake_cnt - 1 {
                            best = v;
                            res = (i1, i2);
                            break;
                        }
                        k += 1;
                    }
                }
                d -= 2;
            }
            if best > 0 {
                return Split { i1: res.0, i2: res.1, min_lo: false, min_hi: true };
            }
        }
        if ec >= xenv.mxcost {
            let (mut fbest, mut fbest1) = (-1i64, -1i64);
            let mut d = fmax;
            while d >= fmin {
                let mut i1 = kf!(d).min(lim1);
                let mut i2 = i1 - d;
                if lim2 < i2 {
                    i1 = lim2 + d;
                    i2 = lim2;
                }
                if fbest < i1 + i2 {
                    fbest = i1 + i2;
                    fbest1 = i1;
                }
                d -= 2;
            }
            let (mut bbest, mut bbest1) = (LINE_MAX, LINE_MAX);
            let mut d = bmax;
            while d >= bmin {
                let mut i1 = off1.max(kb!(d));
                let mut i2 = i1 - d;
                if i2 < off2 {
                    i1 = off2 + d;
                    i2 = off2;
                }
                if i1 + i2 < bbest {
                    bbest = i1 + i2;
                    bbest1 = i1;
                }
                d -= 2;
            }
            if (lim1 + lim2) - bbest < fbest - (off1 + off2) {
                return Split { i1: fbest1, i2: fbest - fbest1, min_lo: true, min_hi: false };
            }
            return Split { i1: bbest1, i2: bbest - bbest1, min_lo: false, min_hi: true };
        }
        ec += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// Histogram (xhistogram.c)
// ---------------------------------------------------------------------------------------------

/// Lines are 1-based here, as in xhistogram.c.
fn histogram(env: &mut Env<'_>, mut line1: i64, mut count1: i64, mut line2: i64, mut count2: i64) {
    loop {
        if count1 <= 0 && count2 <= 0 {
            return;
        }
        if count1 == 0 {
            for l in line2..line2 + count2 {
                env.b.set(l - 1, 1);
            }
            return;
        } else if count2 == 0 {
            for l in line1..line1 + count1 {
                env.a.set(l - 1, 1);
            }
            return;
        }
        let (found, lcs) = find_lcs(env, line1, count1, line2, count2);
        if found {
            fall_back(env, line1, count1, line2, count2);
            return;
        }
        if lcs.0 == 0 && lcs.2 == 0 {
            for l in line1..line1 + count1 {
                env.a.set(l - 1, 1);
            }
            for l in line2..line2 + count2 {
                env.b.set(l - 1, 1);
            }
            return;
        }
        let (b1, e1, b2, e2) = lcs;
        histogram(env, line1, b1 - line1, line2, b2 - line2);
        let end1 = line1 + count1 - 1;
        let end2 = line2 + count2 - 1;
        count1 = end1 - e1;
        line1 = e1 + 1;
        count2 = end2 - e2;
        line2 = e2 + 1;
    }
}

const MAX_CHAIN: u64 = 64;

/// Returns (fall back to Myers?, (begin1, end1, begin2, end2)).
fn find_lcs(env: &Env<'_>, line1: i64, count1: i64, line2: i64, count2: i64) -> (bool, (i64, i64, i64, i64)) {
    let end1 = line1 + count1 - 1;
    let end2 = line2 + count2 - 1;
    let c1 = |l: i64| env.a.ha[(l - 1) as usize];
    let c2 = |l: i64| env.b.ha[(l - 1) as usize];
    // scanA: per class, the first occurrence (chain head, ascending via next) and a count.
    // rec_of[line] -> record index; records hold (head ptr, cnt).
    let mut recs: Vec<(i64, u64)> = Vec::new();
    let mut by_class: BTreeMap<usize, usize> = BTreeMap::new();
    let mut line_rec = vec![0usize; count1 as usize];
    let mut next = vec![0i64; count1 as usize];
    let mut p = end1;
    while line1 <= p {
        let c = c1(p);
        match by_class.get(&c) {
            Some(&r) => {
                next[(p - line1) as usize] = recs[r].0;
                recs[r].0 = p;
                recs[r].1 += 1;
                line_rec[(p - line1) as usize] = r;
            }
            None => {
                let r = recs.len();
                recs.push((p, 1));
                by_class.insert(c, r);
                line_rec[(p - line1) as usize] = r;
            }
        }
        p -= 1;
    }
    let cnt_at = |l: i64| recs[line_rec[(l - line1) as usize]].1;
    let next_of = |l: i64| next[(l - line1) as usize];
    let mut index_cnt: u64 = MAX_CHAIN + 1;
    let mut has_common = false;
    let mut lcs = (0i64, 0i64, 0i64, 0i64);
    let mut b_ptr = line2;
    while b_ptr <= end2 {
        let mut b_next = b_ptr + 1;
        if let Some(&r) = by_class.get(&c2(b_ptr)) {
            let rec = recs[r];
            if rec.1 > index_cnt {
                if !has_common {
                    has_common = true; // same class by construction
                }
            } else {
                let mut a_s = rec.0;
                has_common = true;
                loop {
                    let mut should_break = false;
                    let mut np = next_of(a_s);
                    let mut bs = b_ptr;
                    let mut ae = a_s;
                    let mut be = bs;
                    let mut rc = rec.1;
                    while line1 < a_s && line2 < bs && c1(a_s - 1) == c2(bs - 1) {
                        a_s -= 1;
                        bs -= 1;
                        if 1 < rc {
                            rc = rc.min(cnt_at(a_s));
                        }
                    }
                    while ae < end1 && be < end2 && c1(ae + 1) == c2(be + 1) {
                        ae += 1;
                        be += 1;
                        if 1 < rc {
                            rc = rc.min(cnt_at(ae));
                        }
                    }
                    if b_next <= be {
                        b_next = be + 1;
                    }
                    if lcs.1 - lcs.0 < ae - a_s || rc < index_cnt {
                        lcs = (a_s, ae, bs, be);
                        index_cnt = rc;
                    }
                    if np == 0 {
                        break;
                    }
                    while np <= ae {
                        np = next_of(np);
                        if np == 0 {
                            should_break = true;
                            break;
                        }
                    }
                    if should_break {
                        break;
                    }
                    a_s = np;
                }
            }
        }
        b_ptr = b_next;
    }
    (has_common && MAX_CHAIN < index_cnt, lcs)
}

fn fall_back(env: &mut Env<'_>, line1: i64, count1: i64, line2: i64, count2: i64) {
    let r1: Vec<&[u8]> = env.a.recs[(line1 - 1) as usize..(line1 - 1 + count1) as usize].to_vec();
    let r2: Vec<&[u8]> = env.b.recs[(line2 - 1) as usize..(line2 - 1 + count2) as usize].to_vec();
    let sub = prepare_and_diff(r1, r2, Algorithm::Myers);
    for i in 0..count1 {
        env.a.set(line1 - 1 + i, sub.a.rchg[(i + 1) as usize]);
    }
    for i in 0..count2 {
        env.b.set(line2 - 1 + i, sub.b.rchg[(i + 1) as usize]);
    }
}

// ---------------------------------------------------------------------------------------------
// Change compaction with the indent heuristic (xdiffi.c)
// ---------------------------------------------------------------------------------------------

const MAX_INDENT: i32 = 200;
const MAX_BLANKS: i32 = 20;
const START_OF_FILE_PENALTY: i32 = 1;
const END_OF_FILE_PENALTY: i32 = 21;
const TOTAL_BLANK_WEIGHT: i32 = -30;
const POST_BLANK_WEIGHT: i32 = 6;
const RELATIVE_INDENT_PENALTY: i32 = -4;
const RELATIVE_INDENT_WITH_BLANK_PENALTY: i32 = 10;
const RELATIVE_OUTDENT_PENALTY: i32 = 24;
const RELATIVE_OUTDENT_WITH_BLANK_PENALTY: i32 = 17;
const RELATIVE_DEDENT_PENALTY: i32 = 23;
const RELATIVE_DEDENT_WITH_BLANK_PENALTY: i32 = 17;
const INDENT_WEIGHT: i32 = 60;
const INDENT_HEURISTIC_MAX_SLIDING: i64 = 100;

fn isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn get_indent(rec: &[u8]) -> i32 {
    let mut ret = 0;
    for &c in rec {
        if !isspace(c) {
            return ret;
        } else if c == b' ' {
            ret += 1;
        } else if c == b'\t' {
            ret += 8 - ret % 8;
        }
        if ret >= MAX_INDENT {
            return MAX_INDENT;
        }
    }
    -1
}

#[derive(Default)]
struct Measure {
    end_of_file: bool,
    indent: i32,
    pre_blank: i32,
    pre_indent: i32,
    post_blank: i32,
    post_indent: i32,
}

fn measure_split(s: &Side<'_>, split: i64) -> Measure {
    let mut m = Measure::default();
    if split >= s.nrec() {
        m.end_of_file = true;
        m.indent = -1;
    } else {
        m.indent = get_indent(s.recs[split as usize]);
    }
    m.pre_indent = -1;
    let mut i = split - 1;
    while i >= 0 {
        m.pre_indent = get_indent(s.recs[i as usize]);
        if m.pre_indent != -1 {
            break;
        }
        m.pre_blank += 1;
        if m.pre_blank == MAX_BLANKS {
            m.pre_indent = 0;
            break;
        }
        i -= 1;
    }
    m.post_indent = -1;
    let mut i = split + 1;
    while i < s.nrec() {
        m.post_indent = get_indent(s.recs[i as usize]);
        if m.post_indent != -1 {
            break;
        }
        m.post_blank += 1;
        if m.post_blank == MAX_BLANKS {
            m.post_indent = 0;
            break;
        }
        i += 1;
    }
    m
}

#[derive(Clone, Copy, Default)]
struct Score {
    effective_indent: i32,
    penalty: i32,
}

fn score_add_split(m: &Measure, s: &mut Score) {
    if m.pre_indent == -1 && m.pre_blank == 0 {
        s.penalty += START_OF_FILE_PENALTY;
    }
    if m.end_of_file {
        s.penalty += END_OF_FILE_PENALTY;
    }
    let post_blank = if m.indent == -1 { 1 + m.post_blank } else { 0 };
    let total_blank = m.pre_blank + post_blank;
    s.penalty += TOTAL_BLANK_WEIGHT * total_blank;
    s.penalty += POST_BLANK_WEIGHT * post_blank;
    let indent = if m.indent != -1 { m.indent } else { m.post_indent };
    let any_blanks = total_blank != 0;
    s.effective_indent += indent;
    if indent == -1 || m.pre_indent == -1 {
    } else if indent > m.pre_indent {
        s.penalty += if any_blanks { RELATIVE_INDENT_WITH_BLANK_PENALTY } else { RELATIVE_INDENT_PENALTY };
    } else if indent == m.pre_indent {
    } else if m.post_indent != -1 && m.post_indent > indent {
        s.penalty += if any_blanks { RELATIVE_OUTDENT_WITH_BLANK_PENALTY } else { RELATIVE_OUTDENT_PENALTY };
    } else {
        s.penalty += if any_blanks { RELATIVE_DEDENT_WITH_BLANK_PENALTY } else { RELATIVE_DEDENT_PENALTY };
    }
}

fn score_cmp(a: &Score, b: &Score) -> i32 {
    let ci = (a.effective_indent > b.effective_indent) as i32 - (a.effective_indent < b.effective_indent) as i32;
    INDENT_WEIGHT * ci + (a.penalty - b.penalty)
}

#[derive(Clone, Copy)]
struct Group {
    start: i64,
    end: i64,
}

fn group_init(s: &Side<'_>) -> Group {
    let mut g = Group { start: 0, end: 0 };
    while s.chg(g.end) {
        g.end += 1;
    }
    g
}

fn group_next(s: &Side<'_>, g: &mut Group) -> bool {
    if g.end == s.nrec() {
        return false;
    }
    g.start = g.end + 1;
    g.end = g.start;
    while s.chg(g.end) {
        g.end += 1;
    }
    true
}

fn group_previous(s: &Side<'_>, g: &mut Group) -> bool {
    if g.start == 0 {
        return false;
    }
    g.end = g.start - 1;
    g.start = g.end;
    while s.chg(g.start - 1) {
        g.start -= 1;
    }
    true
}

fn group_slide_down(s: &mut Side<'_>, g: &mut Group) -> bool {
    if g.end < s.nrec() && s.ha[g.start as usize] == s.ha[g.end as usize] {
        s.set(g.start, 0);
        g.start += 1;
        s.set(g.end, 1);
        g.end += 1;
        while s.chg(g.end) {
            g.end += 1;
        }
        true
    } else {
        false
    }
}

fn group_slide_up(s: &mut Side<'_>, g: &mut Group) -> bool {
    if g.start > 0 && s.ha[(g.start - 1) as usize] == s.ha[(g.end - 1) as usize] {
        g.start -= 1;
        s.set(g.start, 1);
        g.end -= 1;
        s.set(g.end, 0);
        while s.chg(g.start - 1) {
            g.start -= 1;
        }
        true
    } else {
        false
    }
}

fn change_compact(x: &mut Side<'_>, o: &mut Side<'_>, indent_heuristic: bool) {
    let mut g = group_init(x);
    let mut go = group_init(o);
    loop {
        if g.end != g.start {
            let mut groupsize;
            let mut earliest_end;
            let mut end_matching_other;
            loop {
                groupsize = g.end - g.start;
                end_matching_other = -1;
                while group_slide_up(x, &mut g) {
                    let ok = group_previous(o, &mut go);
                    debug_assert!(ok, "group sync broken sliding up");
                }
                earliest_end = g.end;
                if go.end > go.start {
                    end_matching_other = g.end;
                }
                loop {
                    if !group_slide_down(x, &mut g) {
                        break;
                    }
                    let ok = group_next(o, &mut go);
                    debug_assert!(ok, "group sync broken sliding down");
                    if go.end > go.start {
                        end_matching_other = g.end;
                    }
                }
                if groupsize == g.end - g.start {
                    break;
                }
            }
            if g.end == earliest_end {
            } else if end_matching_other != -1 {
                while go.end == go.start {
                    group_slide_up(x, &mut g);
                    group_previous(o, &mut go);
                }
            } else if indent_heuristic {
                let mut shift = earliest_end;
                if g.end - groupsize - 1 > shift {
                    shift = g.end - groupsize - 1;
                }
                if g.end - INDENT_HEURISTIC_MAX_SLIDING > shift {
                    shift = g.end - INDENT_HEURISTIC_MAX_SLIDING;
                }
                let mut best_shift = -1;
                let mut best = Score::default();
                while shift <= g.end {
                    let mut score = Score::default();
                    score_add_split(&measure_split(x, shift), &mut score);
                    score_add_split(&measure_split(x, shift - groupsize), &mut score);
                    if best_shift == -1 || score_cmp(&score, &best) <= 0 {
                        best = score;
                        best_shift = shift;
                    }
                    shift += 1;
                }
                while g.end > best_shift {
                    group_slide_up(x, &mut g);
                    group_previous(o, &mut go);
                }
            }
        }
        if !group_next(x, &mut g) {
            break;
        }
        group_next(o, &mut go);
    }
}

// ---------------------------------------------------------------------------------------------
// Script and unified output (xdiffi.c xdl_build_script, xemit.c)
// ---------------------------------------------------------------------------------------------

/// One change: `chg1` records at `i1` replaced by `chg2` records at `i2` (0-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    /// First removed record in the pre-image.
    pub i1: i64,
    /// First added record in the post-image.
    pub i2: i64,
    /// Removed count.
    pub chg1: i64,
    /// Added count.
    pub chg2: i64,
}

/// The edit script, in ascending order.
pub fn build_script(env: &Env<'_>) -> Vec<Change> {
    let mut v = Vec::new();
    let (mut i1, mut i2) = (env.a.nrec(), env.b.nrec());
    while i1 >= 0 || i2 >= 0 {
        if env.a.chg(i1 - 1) || env.b.chg(i2 - 1) {
            let l1 = i1;
            while env.a.chg(i1 - 1) {
                i1 -= 1;
            }
            let l2 = i2;
            while env.b.chg(i2 - 1) {
                i2 -= 1;
            }
            v.push(Change { i1, i2, chg1: l1 - i1, chg2: l2 - i2 });
        }
        i1 -= 1;
        i2 -= 1;
    }
    v.reverse();
    v
}

/// Output options.
#[derive(Debug, Clone, Copy)]
pub struct EmitOptions {
    /// Context lines (`-U`, default 3).
    pub context: i64,
    /// `--inter-hunk-context` (default 0).
    pub interhunk: i64,
    /// Show the function line in hunk headers (git always does).
    pub funcnames: bool,
}

impl Default for EmitOptions {
    fn default() -> Self {
        EmitOptions { context: 3, interhunk: 0, funcnames: true }
    }
}

fn def_ff(rec: &[u8], buf: &mut Vec<u8>) -> bool {
    if let Some(&c) = rec.first() {
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let mut len = rec.len().min(80);
            while len > 0 && isspace(rec[len - 1]) {
                len -= 1;
            }
            buf.clear();
            buf.extend_from_slice(&rec[..len]);
            return true;
        }
    }
    false
}

fn push_num(out: &mut Vec<u8>, v: i64) {
    crate::object::push_decimal(out, v.max(0) as u64);
}

fn emit_rec(out: &mut Vec<u8>, pre: u8, rec: &[u8]) {
    out.push(pre);
    out.extend_from_slice(rec);
    if !rec.is_empty() && *rec.last().unwrap() != b'\n' {
        out.extend_from_slice(b"\n\\ No newline at end of file\n");
    }
}

/// The unified hunks (from the first `@@` on) for a computed diff.
pub fn emit(env: &Env<'_>, script: &[Change], o: &EmitOptions, out: &mut Vec<u8>) {
    let max_common = 2 * o.context + o.interhunk;
    let n1 = env.a.nrec();
    let n2 = env.b.nrec();
    let mut func: Vec<u8> = Vec::new();
    let mut funclineprev: i64 = -1;
    let mut k = 0;
    while k < script.len() {
        // xdl_get_hunk without ignorable changes
        let mut e = k;
        while e + 1 < script.len() {
            let prev = script[e];
            let next = script[e + 1];
            if next.i1 - (prev.i1 + prev.chg1) > max_common {
                break;
            }
            e += 1;
        }
        let first = script[k];
        let last = script[e];
        let s1 = (first.i1 - o.context).max(0);
        let mut s2 = (first.i2 - o.context).max(0);
        let mut lctx = o.context;
        lctx = lctx.min(n1 - (last.i1 + last.chg1));
        lctx = lctx.min(n2 - (last.i2 + last.chg2));
        let e1 = last.i1 + last.chg1 + lctx;
        let e2 = last.i2 + last.chg2 + lctx;
        if o.funcnames {
            let start = s1 - 1;
            let limit = funclineprev;
            let step = if start > limit { -1 } else { 1 };
            let mut l = start;
            while l != limit && 0 <= l && l < n1 {
                if def_ff(env.a.recs[l as usize], &mut func) {
                    break;
                }
                l += step;
            }
            funclineprev = s1 - 1;
        }
        // header
        let (c1, c2) = (e1 - s1, e2 - s2);
        let mut h = Vec::with_capacity(64);
        h.extend_from_slice(b"@@ -");
        push_num(&mut h, if c1 != 0 { s1 + 1 } else { s1 });
        if c1 != 1 {
            h.push(b',');
            push_num(&mut h, c1);
        }
        h.extend_from_slice(b" +");
        push_num(&mut h, if c2 != 0 { s2 + 1 } else { s2 });
        if c2 != 1 {
            h.push(b',');
            push_num(&mut h, c2);
        }
        h.extend_from_slice(b" @@");
        if !func.is_empty() {
            h.push(b' ');
            let room = 128 - h.len() - 1;
            h.extend_from_slice(&func[..func.len().min(room)]);
        }
        h.push(b'\n');
        // git's diff.c `sane_truncate_line`: the header is cut at the first byte that does not
        // start a valid UTF-8 character (a function name truncated mid-character), newline kept.
        let cut = utf8_valid_prefix(&h);
        if cut < h.len() {
            h.truncate(cut);
            h.push(b'\n');
        }
        out.extend_from_slice(&h);
        // pre-context
        while s2 < first.i2 {
            emit_rec(out, b' ', env.b.recs[s2 as usize]);
            s2 += 1;
        }
        let (mut p1, mut p2) = (first.i1, first.i2);
        for c in &script[k..=e] {
            while p1 < c.i1 && p2 < c.i2 {
                emit_rec(out, b' ', env.b.recs[p2 as usize]);
                p1 += 1;
                p2 += 1;
            }
            for i in c.i1..c.i1 + c.chg1 {
                emit_rec(out, b'-', env.a.recs[i as usize]);
            }
            for i in c.i2..c.i2 + c.chg2 {
                emit_rec(out, b'+', env.b.recs[i as usize]);
            }
            p1 = c.i1 + c.chg1;
            p2 = c.i2 + c.chg2;
        }
        let mut s = last.i2 + last.chg2;
        while s < e2 {
            emit_rec(out, b' ', env.b.recs[s as usize]);
            s += 1;
        }
        k = e + 1;
    }
}

/// Length of the longest prefix of whole valid UTF-8 characters (git's `utf8_width` rules:
/// no overlongs, surrogates, code points above U+10FFFF, or U+FFFE/U+FFFF).
pub fn utf8_valid_prefix(s: &[u8]) -> usize {
    let n = match core::str::from_utf8(s) {
        Ok(_) => s.len(),
        Err(e) => e.valid_up_to(),
    };
    let st = core::str::from_utf8(&s[..n]).unwrap();
    for (i, c) in st.char_indices() {
        if c == '\u{fffe}' || c == '\u{ffff}' {
            return i;
        }
    }
    n
}

/// (added, deleted) line counts of a computed diff.
pub fn counts(env: &Env<'_>) -> (u64, u64) {
    let del = env.a.rchg.iter().filter(|&&c| c != 0).count() as u64;
    let add = env.b.rchg.iter().filter(|&&c| c != 0).count() as u64;
    (add, del)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simple_hunk() {
        let a = b"a\nb\nc\nd\ne\nf\ng\n";
        let b = b"a\nb\nc\nX\ne\nf\ng\n";
        let env = diff(a, b, Algorithm::Myers, true);
        let s = build_script(&env);
        let mut o = Vec::new();
        emit(&env, &s, &EmitOptions::default(), &mut o);
        assert_eq!(core::str::from_utf8(&o).unwrap(), "@@ -1,7 +1,7 @@\n a\n b\n c\n-d\n+X\n e\n f\n g\n");
    }
}
