// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The EXIF facts a photograph carries (EXIF 2.32, CIPA DC-008): who made it, with what, how, and when. These
//! become `media:*` attributes beside `una:type` (ATTRCOLUMNS), so Quarry's columns show a shoot.

use alloc::format;
use alloc::string::String;

use crate::tiff::Reader;

pub mod tag {
    pub const EXPOSURE_TIME: u16 = 33434;
    pub const F_NUMBER: u16 = 33437;
    pub const ISO: u16 = 34855;
    pub const DATE_ORIGINAL: u16 = 36867;
    pub const FOCAL_LENGTH: u16 = 37386;
    pub const LENS_MODEL: u16 = 42036;
}

/// The facts of one photograph. `None` = the file does not say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    /// ExposureTime, seconds as a ratio.
    pub exposure: Option<(u32, u32)>,
    pub f_number: Option<(u32, u32)>,
    pub iso: Option<u32>,
    /// FocalLength, millimetres as a ratio.
    pub focal: Option<(u32, u32)>,
    /// DateTimeOriginal as written (`YYYY:MM:DD HH:MM:SS`, the camera's local time).
    pub taken_text: Option<String>,
    /// Sensor size (the raw strip's), else IFD0's image size.
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Read the EXIF IFD at `off` into `f`. Returns 1 when an IFD was read, else 0.
pub fn read(r: &Reader, off: usize, f: &mut Facts) -> u8 {
    let Ok((ents, _)) = r.ifd(off) else { return 0 };
    for e in &ents {
        match e.tag {
            tag::EXPOSURE_TIME => f.exposure = r.rational(e).filter(|&(_, d)| d != 0),
            tag::F_NUMBER => f.f_number = r.rational(e).filter(|&(_, d)| d != 0),
            tag::ISO => f.iso = r.uint(e, 0).filter(|&v| v > 0),
            tag::DATE_ORIGINAL => f.taken_text = r.ascii(e),
            tag::FOCAL_LENGTH => f.focal = r.rational(e).filter(|&(_, d)| d != 0),
            tag::LENS_MODEL => f.lens = r.ascii(e),
            _ => {}
        }
    }
    1
}

impl Facts {
    /// `Make Model`, the make dropped when the model already starts with it.
    pub fn camera(&self) -> Option<String> {
        match (&self.make, &self.model) {
            (Some(m), Some(md)) if md.to_ascii_uppercase().starts_with(&m.to_ascii_uppercase()) => Some(md.clone()),
            (Some(m), Some(md)) => Some(format!("{m} {md}")),
            (None, Some(md)) => Some(md.clone()),
            (Some(m), None) => Some(m.clone()),
            (None, None) => None,
        }
    }

    /// `1/250`, `0.5`, `2` or `30` (seconds) — the way a camera's display says it.
    pub fn exposure_text(&self) -> Option<String> {
        let (n, d) = self.exposure?;
        if n == 0 {
            return None;
        }
        if n < d {
            // 10/2500 -> 1/250; a ratio that does not reduce to 1/x is said in hundredths.
            if d % n == 0 {
                return Some(format!("1/{}", d / n));
            }
            let h = (n as u64 * 100 + d as u64 / 2) / d as u64;
            return Some(format!("0.{:02}", h));
        }
        if n % d == 0 {
            Some(format!("{}", n / d))
        } else {
            let t = (n as u64 * 10 + d as u64 / 2) / d as u64;
            Some(format!("{}.{}", t / 10, t % 10))
        }
    }

    /// Focal length rounded to whole millimetres.
    pub fn focal_mm(&self) -> Option<u32> {
        let (n, d) = self.focal?;
        Some(((n as u64 + d as u64 / 2) / d as u64) as u32)
    }

    /// DateTimeOriginal as seconds since 1970-01-01 00:00 — the camera's wall clock read as UTC (EXIF 2.32's
    /// OffsetTimeOriginal is owed: an ARW names no zone the core reads yet).
    pub fn taken(&self) -> Option<i64> {
        parse_datetime(self.taken_text.as_deref()?)
    }

    /// How many facts are present (the witness's `facts=`).
    pub fn count(&self) -> usize {
        [
            self.camera().is_some(),
            self.lens.is_some(),
            self.exposure_text().is_some(),
            self.iso.is_some(),
            self.focal_mm().is_some(),
            self.taken().is_some(),
            self.width.is_some(),
            self.height.is_some(),
        ]
        .iter()
        .filter(|&&b| b)
        .count()
    }
}

/// `YYYY:MM:DD HH:MM:SS` -> unix seconds (proleptic Gregorian, H. Hinnant's days-from-civil). Pure.
pub fn parse_datetime(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: core::ops::Range<usize>| -> Option<i64> { core::str::from_utf8(&b[r]).ok()?.parse::<i64>().ok() };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}
