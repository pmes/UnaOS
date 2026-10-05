//! CELT band energies, RFC 6716 §4.3.2 (`celt/quant_bands.c`, `celt/laplace.c`): the Laplace-coded coarse
//! energy with inter/intra prediction, fine energy, and the final priority bits.
use super::super::range::RangeDecoder;
use super::fixed::*;
use super::rate::MAX_FINE_BITS;
use super::tables::NB_EBANDS;

pub static E_MEANS: [i8; 25] = [103, 100, 92, 85, 81, 77, 72, 70, 78, 75, 73, 71, 78, 74, 69, 72, 70, 74, 76, 71, 60, 60, 60, 60, 60];
static PRED_COEF: [i32; 4] = [29440, 26112, 21248, 16384];
static BETA_COEF: [i32; 4] = [30147, 22282, 12124, 6554];
const BETA_INTRA: i32 = 4915;
static SMALL_ENERGY_ICDF: [u8; 3] = [2, 1, 0];
static E_PROB_MODEL: [[[u8; 42]; 2]; 4] = [
    [
        [72, 127, 65, 129, 66, 128, 65, 128, 64, 128, 62, 128, 64, 128, 64, 128, 92, 78, 92, 79, 92, 78, 90, 79, 116, 41, 115, 40, 114, 40, 132, 26, 132, 26, 145, 17, 161, 12, 176, 10, 177, 11],
        [24, 179, 48, 138, 54, 135, 54, 132, 53, 134, 56, 133, 55, 132, 55, 132, 61, 114, 70, 96, 74, 88, 75, 88, 87, 74, 89, 66, 91, 67, 100, 59, 108, 50, 120, 40, 122, 37, 97, 43, 78, 50],
    ],
    [
        [83, 78, 84, 81, 88, 75, 86, 74, 87, 71, 90, 73, 93, 74, 93, 74, 109, 40, 114, 36, 117, 34, 117, 34, 143, 17, 145, 18, 146, 19, 162, 12, 165, 10, 178, 7, 189, 6, 190, 8, 177, 9],
        [23, 178, 54, 115, 63, 102, 66, 98, 69, 99, 74, 89, 71, 91, 73, 91, 78, 89, 86, 80, 92, 66, 93, 64, 102, 59, 103, 60, 104, 60, 117, 52, 123, 44, 138, 35, 133, 31, 97, 38, 77, 45],
    ],
    [
        [61, 90, 93, 60, 105, 42, 107, 41, 110, 45, 116, 38, 113, 38, 112, 38, 124, 26, 132, 27, 136, 19, 140, 20, 155, 14, 159, 16, 158, 18, 170, 13, 177, 10, 187, 8, 192, 6, 175, 9, 159, 10],
        [21, 178, 59, 110, 71, 86, 75, 85, 84, 83, 91, 66, 88, 73, 87, 72, 92, 75, 98, 72, 105, 58, 107, 54, 115, 52, 114, 55, 112, 56, 129, 51, 132, 40, 150, 33, 140, 29, 98, 35, 77, 42],
    ],
    [
        [42, 121, 96, 66, 108, 43, 111, 40, 117, 44, 123, 32, 120, 36, 119, 33, 127, 33, 134, 34, 139, 21, 147, 23, 152, 20, 158, 25, 154, 26, 166, 21, 173, 16, 184, 13, 184, 10, 150, 13, 139, 15],
        [22, 178, 63, 114, 74, 82, 84, 83, 92, 82, 103, 62, 96, 72, 96, 67, 101, 73, 107, 72, 113, 55, 118, 52, 125, 52, 118, 52, 117, 55, 135, 49, 137, 39, 157, 32, 145, 29, 97, 33, 77, 40],
    ],
];

fn laplace_get_freq1(fs0: u32, decay: i32) -> u32 {
    let ft = 32768 - 2 * 16 - fs0;
    ((ft as i32 * (16384 - decay)) >> 15) as u32
}

pub fn laplace_decode(dec: &mut RangeDecoder, fs: u32, decay: i32) -> i32 {
    let mut val = 0i32;
    let mut fs = fs;
    let fm = dec.decode_bin(15);
    let mut fl = 0u32;
    if fm >= fs {
        val += 1;
        fl = fs;
        fs = laplace_get_freq1(fs, decay) + 1;
        while fs > 1 && fm >= fl + 2 * fs {
            fs *= 2;
            fl += fs;
            fs = (((fs - 2) as i32 * decay) >> 15) as u32;
            fs += 1;
            val += 1;
        }
        if fs <= 1 {
            let di = (fm - fl) >> 1;
            val += di as i32;
            fl += 2 * di;
        }
        if fm < fl + fs { val = -val; } else { fl += fs; }
    }
    dec.update(fl, (fl + fs).min(32768), 32768);
    val
}

pub fn unquant_coarse(start: usize, end: usize, old: &mut [i16], intra: bool, dec: &mut RangeDecoder, c: usize, lm: i32) {
    let prob = &E_PROB_MODEL[lm as usize][intra as usize];
    let mut prev = [0i32; 2];
    let (coef, beta) = if intra { (0, BETA_INTRA) } else { (PRED_COEF[lm as usize], BETA_COEF[lm as usize]) };
    let budget = dec.storage as i32 * 8;
    for i in start..end {
        for ch in 0..c {
            let tell = dec.tell();
            let qi: i32 = if budget - tell >= 15 {
                let pi = 2 * i.min(20);
                laplace_decode(dec, (prob[pi] as u32) << 7, (prob[pi + 1] as i32) << 6)
            } else if budget - tell >= 2 {
                let q = dec.icdf(&SMALL_ENERGY_ICDF, 2) as i32;
                (q >> 1) ^ -(q & 1)
            } else if budget - tell >= 1 {
                -(dec.bit_logp(1) as i32)
            } else {
                -1
            };
            let q = shl32(qi, DB_SHIFT);
            let k = i + ch * NB_EBANDS;
            old[k] = (old[k] as i32).max(-(9 << DB_SHIFT)) as i16;
            let mut tmp = pshr32(mult16_16(coef, old[k] as i32), 8).wrapping_add(prev[ch]).wrapping_add(shl32(q, 7));
            tmp = tmp.max(-(28 << (DB_SHIFT + 7)));
            old[k] = pshr32(tmp, 7) as i16;
            prev[ch] = prev[ch].wrapping_add(shl32(q, 7)).wrapping_sub(mult16_16(beta, pshr32(q, 8)));
        }
    }
}

pub fn unquant_fine(start: usize, end: usize, old: &mut [i16], fine_quant: &[i32], dec: &mut RangeDecoder, c: usize) {
    for i in start..end {
        if fine_quant[i] <= 0 { continue; }
        for ch in 0..c {
            let q2 = dec.bits(fine_quant[i] as u32) as i32;
            let offset = sub16((shl32(q2, DB_SHIFT) + 512) >> fine_quant[i], 512);
            let k = i + ch * NB_EBANDS;
            old[k] = (old[k] as i32 + offset) as i16;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn unquant_finalise(start: usize, end: usize, old: &mut [i16], fine_quant: &[i32], fine_priority: &[i32], mut bits_left: i32, dec: &mut RangeDecoder, c: usize) {
    for prio in 0..2 {
        let mut i = start;
        while i < end && bits_left >= c as i32 {
            if fine_quant[i] >= MAX_FINE_BITS || fine_priority[i] != prio { i += 1; continue; }
            for ch in 0..c {
                let q2 = dec.bits(1) as i32;
                let offset = (shl16(q2, DB_SHIFT) - 512) >> (fine_quant[i] + 1);
                let k = i + ch * NB_EBANDS;
                old[k] = (old[k] as i32 + offset) as i16;
                bits_left -= 1;
            }
            i += 1;
        }
    }
}
