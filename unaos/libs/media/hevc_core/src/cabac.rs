// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CABAC (9.3) over h264_core's arithmetic decoding engine ([`h264_core::cabac::Cabac`]; H.265 9.3.4.3.2 DecodeDecision,
//! 9.3.4.3.4 DecodeBypass, 9.3.4.3.5 DecodeTerminate and 9.3.2.5 initialisation are that engine bit for bit). This
//! module holds what is H.265's own: the context initialisation of 9.3.2.2 over hevc_core's ctxIdx layout (the
//! crate doc), the storage and synchronisation of 9.3.2.4 (wavefront rows, dependent slice segments), and the
//! binarizations of 9.3.3 (TR, EGk, FL, cu_qp_delta_abs, coeff_abs_level_remaining).
//!
//! After a DecodeTerminate that returns 1 (end_of_slice_segment_flag, end_of_subset_one_bit, pcm_flag) the engine's
//! reader sits right after the last bit of the arithmetic code word (h264_core's position contract, cabac.rs:7-11);
//! that last bit is the 1 the encoder's flush ends with, which is the alignment_bit_equal_to_one of byte_alignment()
//! after end_of_subset_one_bit, and is followed by the pcm_alignment_zero_bits before pcm_sample(). So the next
//! syntax starts at the next byte boundary: [`Engine::aligned_pos`].

use h264_core::cabac::{Cabac, NUM_CTX};

use crate::slice::SliceType;
use crate::tables::{init_state, CNU, CTX_TABLES};

/// A saved set of context states (9.3.2.4 TableStateIdxWpp / TableStateIdxDs).
pub type Contexts = [u8; NUM_CTX];

/// initType (9.3.2.2, Table 9-4 note): 0 for I slices; P: cabac_init_flag ? 2 : 1; B: cabac_init_flag ? 1 : 2.
pub fn init_type(slice_type: SliceType, cabac_init_flag: bool) -> usize {
    match slice_type {
        SliceType::I => 0,
        SliceType::P => {
            if cabac_init_flag {
                2
            } else {
                1
            }
        }
        SliceType::B => {
            if cabac_init_flag {
                1
            } else {
                2
            }
        }
    }
}

/// The initial context states of every slot for (initType, SliceQpY), 9.3.2.2.
pub fn initial_contexts(init_type: usize, qp: i32) -> Contexts {
    let mut s = [0u8; NUM_CTX];
    for t in CTX_TABLES {
        let row = t.init[init_type];
        for i in 0..t.len() {
            s[t.slot + i] = init_state(row.get(i).copied().unwrap_or(CNU), qp);
        }
    }
    s
}

pub struct Engine<'a> {
    c: Cabac<'a>,
}

impl<'a> Engine<'a> {
    /// An engine whose arithmetic code starts at byte `byte` of `data` (9.3.2.5), with every context zeroed.
    pub fn new(data: &'a [u8], byte: usize) -> Engine<'a> {
        Engine { c: Cabac::new(data, byte * 8) }
    }

    /// 9.3.2.2 for (initType, SliceQpY).
    pub fn init_contexts(&mut self, init_type: usize, qp: i32) {
        *self.c.contexts_mut() = initial_contexts(init_type, qp);
    }

    /// 9.3.2.4 storage.
    pub fn save(&self) -> Contexts {
        *self.c.contexts()
    }

    /// 9.3.2.4 synchronisation.
    pub fn load(&mut self, s: &Contexts) {
        *self.c.contexts_mut() = *s;
    }

    /// The state byte of one slot (for KATs).
    pub fn state(&self, slot: usize) -> u8 {
        self.c.contexts()[slot]
    }

    /// 9.3.2.5: initialise the arithmetic decoding engine at byte `byte`.
    pub fn restart_at_byte(&mut self, byte: usize) {
        self.c.restart_at(byte * 8);
    }

    /// The byte where the syntax after a DecodeTerminate of 1 begins (see the module doc).
    pub fn aligned_pos(&self) -> usize {
        self.c.pos().div_ceil(8)
    }

    pub fn data(&self) -> &'a [u8] {
        self.c.data()
    }

    pub fn overran(&self) -> bool {
        self.c.overran()
    }

    #[inline]
    pub fn decision(&mut self, slot: usize) -> u32 {
        self.c.decision(slot)
    }

    #[inline]
    pub fn bypass(&mut self) -> u32 {
        self.c.bypass()
    }

    pub fn terminate(&mut self) -> u32 {
        self.c.terminate()
    }

    /// FL binarization in bypass bins (9.3.3.5): `n` bins, most significant first.
    pub fn bypass_bits(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bypass();
        }
        v
    }

    /// TR binarization with cRiceParam 0 in bypass bins (9.3.3.2): ones up to `cmax`.
    pub fn bypass_tu(&mut self, cmax: u32) -> u32 {
        let mut v = 0;
        while v < cmax && self.bypass() == 1 {
            v += 1;
        }
        v
    }

    /// k-th order Exp-Golomb in bypass bins (9.3.3.3).
    pub fn bypass_egk(&mut self, k: u32) -> u32 {
        self.c.exp_golomb_bypass(k)
    }

    /// coeff_abs_level_remaining (9.3.3.11): the TR prefix (cMax 4 << cRiceParam) and, past it, the EG(cRiceParam + 1)
    /// suffix; written as the count of leading ones `p` (p < 4: (p << rice) + rice bits; else
    /// (((1 << (p - 3)) + 2) << rice) + (p - 3 + rice) bits).
    pub fn coeff_abs_level_remaining(&mut self, rice: u32) -> u32 {
        let mut p = 0u32;
        while p < 32 && self.bypass() == 1 {
            p += 1;
        }
        if p < 4 {
            (p << rice) + self.bypass_bits(rice)
        } else {
            let n = p - 3 + rice;
            if n > 31 {
                return u32::MAX >> 1;
            }
            ((((1u64 << (p - 3)) + 2) << rice) as u32).wrapping_add(self.bypass_bits(n))
        }
    }
}
