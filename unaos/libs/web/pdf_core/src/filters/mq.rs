// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The MQ arithmetic decoder (ITU-T T.88 Annex E; the same coder is ITU-T T.800 Annex C, JPEG 2000) — PDFFAX SR331.
//!
//! CHARTER: Aether — shared-core. pdf_core's ONE MQ coder: JBIG2's generic regions, symbol dictionaries and text
//! regions decode through it, and the owed PDF-JPX arc's tier-1 decoder is to reuse it, never a second copy.
//!
//! A context is one byte, `index << 1 | mps`, starting at 0 (index 0, MPS 0). The register layout follows the
//! software convention of E.3 with C split into a 16-bit high half (compared against Qe) and a 16-bit low half that
//! receives the bytes; past the end of the data the decoder is fed 0xFF00, as E.3.4 BYTEIN prescribes for a marker.

/// Table E.1: (Qe, NMPS, NLPS, SWITCH) for each of the 47 probability states.
pub const QE: [(u16, u8, u8, u8); 47] = [
    (0x5601, 1, 1, 1),
    (0x3401, 2, 6, 0),
    (0x1801, 3, 9, 0),
    (0x0AC1, 4, 12, 0),
    (0x0521, 5, 29, 0),
    (0x0221, 38, 33, 0),
    (0x5601, 7, 6, 1),
    (0x5401, 8, 14, 0),
    (0x4801, 9, 14, 0),
    (0x3801, 10, 14, 0),
    (0x3001, 11, 17, 0),
    (0x2401, 12, 18, 0),
    (0x1C01, 13, 20, 0),
    (0x1601, 29, 21, 0),
    (0x5601, 15, 14, 1),
    (0x5401, 16, 14, 0),
    (0x5101, 17, 15, 0),
    (0x4801, 18, 16, 0),
    (0x3801, 19, 17, 0),
    (0x3401, 20, 18, 0),
    (0x3001, 21, 19, 0),
    (0x2801, 22, 19, 0),
    (0x2401, 23, 20, 0),
    (0x2201, 24, 21, 0),
    (0x1C01, 25, 22, 0),
    (0x1801, 26, 23, 0),
    (0x1601, 27, 24, 0),
    (0x1401, 28, 25, 0),
    (0x1201, 29, 26, 0),
    (0x1101, 30, 27, 0),
    (0x0AC1, 31, 28, 0),
    (0x09C1, 32, 29, 0),
    (0x08A1, 33, 30, 0),
    (0x0521, 34, 31, 0),
    (0x0441, 35, 32, 0),
    (0x02A1, 36, 33, 0),
    (0x0221, 37, 34, 0),
    (0x0141, 38, 35, 0),
    (0x0111, 39, 36, 0),
    (0x0085, 40, 37, 0),
    (0x0049, 41, 38, 0),
    (0x0025, 42, 39, 0),
    (0x0015, 43, 40, 0),
    (0x0009, 44, 41, 0),
    (0x0005, 45, 42, 0),
    (0x0001, 45, 43, 0),
    (0x5601, 46, 46, 0),
];

/// An MQ decoder over one byte slice (E.3: INITDEC, BYTEIN, DECODE, RENORMD).
pub struct MqDecoder<'a> {
    data: &'a [u8],
    /// The index of the last byte consumed.
    bp: usize,
    chigh: u32,
    clow: u32,
    a: u32,
    ct: u32,
}

impl<'a> MqDecoder<'a> {
    /// INITDEC.
    pub fn new(data: &'a [u8]) -> Self {
        let mut d = MqDecoder { data, bp: 0, chigh: data.first().copied().unwrap_or(0xFF) as u32, clow: 0, a: 0, ct: 0 };
        d.byte_in();
        d.chigh = ((d.chigh << 7) & 0xFFFF) | ((d.clow >> 9) & 0x7F);
        d.clow = (d.clow << 7) & 0xFFFF;
        d.ct -= 7;
        d.a = 0x8000;
        d
    }

    /// The bytes consumed so far (the read position of the coder).
    pub fn position(&self) -> usize {
        self.bp
    }

    fn at(&self, i: usize) -> Option<u8> {
        self.data.get(i).copied()
    }

    /// BYTEIN: a 0xFF followed by a byte above 0x8F is a marker (the coder is fed 1 bits); otherwise a 0xFF is
    /// followed by a stuffed byte carrying 7 bits.
    fn byte_in(&mut self) {
        if self.at(self.bp) == Some(0xFF) {
            match self.at(self.bp + 1) {
                Some(b) if b <= 0x8F => {
                    self.bp += 1;
                    self.clow += (b as u32) << 9;
                    self.ct = 7;
                }
                _ => {
                    self.clow += 0xFF00;
                    self.ct = 8;
                }
            }
        } else {
            match self.at(self.bp + 1) {
                Some(b) => {
                    self.bp += 1;
                    self.clow += (b as u32) << 8;
                }
                None => self.clow += 0xFF00,
            }
            self.ct = 8;
        }
        if self.clow > 0xFFFF {
            self.chigh += self.clow >> 16;
            self.clow &= 0xFFFF;
        }
    }

    /// DECODE one decision in context `cx` (updated in place).
    pub fn decode(&mut self, cx: &mut u8) -> u8 {
        let mut index = (*cx >> 1) as usize;
        let mut mps = *cx & 1;
        let (qe, nmps, nlps, switch) = QE[index.min(46)];
        let qe = qe as u32;
        let mut a = self.a - qe;
        let d;
        if self.chigh < qe {
            // LPS_EXCHANGE
            if a < qe {
                a = qe;
                d = mps;
                index = nmps as usize;
            } else {
                a = qe;
                d = 1 ^ mps;
                if switch == 1 {
                    mps = d;
                }
                index = nlps as usize;
            }
        } else {
            self.chigh -= qe;
            if a & 0x8000 != 0 {
                self.a = a;
                return mps;
            }
            // MPS_EXCHANGE
            if a < qe {
                d = 1 ^ mps;
                if switch == 1 {
                    mps = d;
                }
                index = nlps as usize;
            } else {
                d = mps;
                index = nmps as usize;
            }
        }
        // RENORMD
        loop {
            if self.ct == 0 {
                self.byte_in();
            }
            a <<= 1;
            self.chigh = ((self.chigh << 1) & 0xFFFF) | ((self.clow >> 15) & 1);
            self.clow = (self.clow << 1) & 0xFFFF;
            self.ct -= 1;
            if a & 0x8000 != 0 {
                break;
            }
        }
        self.a = a;
        *cx = ((index as u8) << 1) | mps;
        d
    }
}
