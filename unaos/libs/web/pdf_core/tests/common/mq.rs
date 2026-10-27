// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The test-side MQ ENCODER (T.88 Annex E.2 = T.800 Annex C.2), shared by the pdf_core test binaries: COPIED from
//! tests/fax.rs by PDFJPX (SR335, GRANT: tests/common) for tests/jpx.rs; fax.rs keeps its own copy until it retires
//! onto this one (FAX-MQENC-ONE, owed). Written from the encoder side of the spec, never the decoder's code. `flush`
//! ends the code with JBIG2's 0xFF 0xAC marker; a JPEG 2000 codeword segment drops those two bytes (T.800 C.2.9).

use pdf_core::filters::mq::QE;

pub struct MqEnc {
    a: u32,
    c: u32,
    ct: u32,
    b: u8,
    have: bool,
    out: Vec<u8>,
}

impl MqEnc {
    pub fn new() -> Self {
        MqEnc { a: 0x8000, c: 0, ct: 12, b: 0, have: false, out: Vec::new() }
    }
    /// BP = BP + 1; B = v (the byte at BPST - 1 is never written out).
    pub fn emit(&mut self, v: u8) {
        if self.have {
            self.out.push(self.b);
        }
        self.b = v;
        self.have = true;
    }
    pub fn byte_out(&mut self) {
        if self.have && self.b == 0xFF {
            self.emit((self.c >> 20) as u8);
            self.c &= 0xFFFFF;
            self.ct = 7;
        } else if self.c < 0x800_0000 {
            self.emit((self.c >> 19) as u8);
            self.c &= 0x7FFFF;
            self.ct = 8;
        } else {
            self.b = self.b.wrapping_add(1);
            if self.b == 0xFF {
                self.c &= 0x7FF_FFFF;
                self.emit((self.c >> 20) as u8);
                self.c &= 0xFFFFF;
                self.ct = 7;
            } else {
                self.emit((self.c >> 19) as u8);
                self.c &= 0x7FFFF;
                self.ct = 8;
            }
        }
    }
    pub fn renorm(&mut self) {
        loop {
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.ct == 0 {
                self.byte_out();
            }
            if self.a & 0x8000 != 0 {
                break;
            }
        }
    }
    pub fn encode(&mut self, cx: &mut u8, d: u8) {
        let mut idx = (*cx >> 1) as usize;
        let mut mps = *cx & 1;
        let (qe, nmps, nlps, sw) = QE[idx];
        let qe = qe as u32;
        self.a -= qe;
        if d == mps {
            if self.a & 0x8000 == 0 {
                if self.a < qe {
                    self.a = qe;
                } else {
                    self.c += qe;
                }
                idx = nmps as usize;
                self.renorm();
            } else {
                self.c += qe;
            }
        } else {
            if self.a < qe {
                self.c += qe;
            } else {
                self.a = qe;
            }
            if sw == 1 {
                mps ^= 1;
            }
            idx = nlps as usize;
            self.renorm();
        }
        *cx = (idx as u8) << 1 | mps;
    }
    pub fn flush(mut self) -> Vec<u8> {
        let temp = self.c + self.a;
        self.c |= 0xFFFF;
        if self.c >= temp {
            self.c -= 0x8000;
        }
        self.c <<= self.ct;
        self.byte_out();
        self.c <<= self.ct;
        self.byte_out();
        if self.b != 0xFF {
            self.emit(0xFF);
        }
        self.emit(0xAC);
        self.out.push(self.b);
        self.out
    }
}
