//! RTCCLOCK (R75) — the battery-backed CMOS real-time clock (ports 0x70/0x71). Every x86 machine has one.
//! `read()` waits out the update-in-progress bit, takes two consecutive reads that must agree, and decodes
//! BCD / 12h per status B. The century comes from the FADT `Century` byte (offset 108) when the ACPI tables
//! are parsed and name one, else 20xx is assumed. `write()` is the inverse for `date -s`.
use x86_64::instructions::port::Port;

const FADT_CENTURY: usize = 108;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rtc {
    pub y: u32,
    pub mo: u32,
    pub d: u32,
    pub h: u32,
    pub mi: u32,
    pub s: u32,
    pub bcd: bool,
    pub h24: bool,
    /// `Some(reg)` = the FADT named a century register; `None` = 20xx assumed.
    pub century_reg: Option<u8>,
}

unsafe fn cmos_read(reg: u8) -> u8 {
    let mut idx: Port<u8> = Port::new(0x70);
    let mut dat: Port<u8> = Port::new(0x71);
    idx.write(reg & 0x7F); // bit 7 clear: NMI stays enabled
    dat.read()
}

unsafe fn cmos_write(reg: u8, v: u8) {
    let mut idx: Port<u8> = Port::new(0x70);
    let mut dat: Port<u8> = Port::new(0x71);
    idx.write(reg & 0x7F);
    dat.write(v);
}

/// The FADT century register index, if the tables are parsed and the byte is non-zero.
fn fadt_century_reg() -> Option<u8> {
    use crate::arch::acpi;
    let rsdp = acpi::rsdp_addr();
    if rsdp == 0 {
        return None;
    }
    let (sdt, esz) = acpi::root_sdt(rsdp)?;
    // SAFETY: firmware tables are identity-mapped; read bounded by the table's own length.
    unsafe {
        let fadt = acpi::find_table(sdt, esz, b"FACP")?;
        if acpi::table_len(fadt) <= FADT_CENTURY {
            return None;
        }
        let c = *((fadt as usize + FADT_CENTURY) as *const u8);
        if c == 0 || c > 0x7F { None } else { Some(c) }
    }
}

fn bcd2bin(v: u8) -> u32 {
    ((v >> 4) as u32) * 10 + (v & 0x0F) as u32
}
fn bin2bcd(v: u32) -> u8 {
    (((v / 10) << 4) | (v % 10)) as u8
}

/// (sec, min, hour, day, month, year, century-raw, status B) raw registers.
unsafe fn raw_snapshot(cent: Option<u8>) -> [u8; 8] {
    // Bounded UIP wait (~ 10 ms is the spec window; the loop bound is generous, never unbounded).
    let mut n = 0u32;
    while cmos_read(0x0A) & 0x80 != 0 && n < 2_000_000 {
        core::hint::spin_loop();
        n += 1;
    }
    [
        cmos_read(0x00),
        cmos_read(0x02),
        cmos_read(0x04),
        cmos_read(0x07),
        cmos_read(0x08),
        cmos_read(0x09),
        match cent { Some(r) => cmos_read(r), None => 0 },
        cmos_read(0x0B),
    ]
}

/// Raw two-consistent-reads decode. `None` if the two reads never agree (bounded retries).
pub fn read_full() -> Option<Rtc> {
    let cent = fadt_century_reg();
    // SAFETY: CMOS index/data ports; read-only access to the clock registers.
    let raw = unsafe {
        let mut prev = raw_snapshot(cent);
        let mut agreed = None;
        for _ in 0..8 {
            let cur = raw_snapshot(cent);
            if cur == prev {
                agreed = Some(cur);
                break;
            }
            prev = cur;
        }
        agreed?
    };
    let sb = raw[7];
    let bcd = sb & 0x04 == 0;
    let h24 = sb & 0x02 != 0;
    let dec = |v: u8| if bcd { bcd2bin(v) } else { v as u32 };
    let pm = !h24 && raw[2] & 0x80 != 0;
    let mut h = dec(raw[2] & 0x7F);
    if !h24 {
        h %= 12;
        if pm { h += 12; }
    }
    let yy = dec(raw[5]);
    let century = if cent.is_some() { dec(raw[6]) } else { 20 };
    Some(Rtc {
        y: century * 100 + yy,
        mo: dec(raw[4]),
        d: dec(raw[3]),
        h,
        mi: dec(raw[1]),
        s: dec(raw[0]),
        bcd,
        h24,
        century_reg: cent,
    })
}

/// `(year, month, day, hour, min, sec)` of the CMOS clock.
pub fn read() -> Option<(u32, u32, u32, u32, u32, u32)> {
    read_full().map(|r| (r.y, r.mo, r.d, r.h, r.mi, r.s))
}

fn plausible(r: &Rtc) -> bool {
    (1980..=2107).contains(&r.y) && (1..=12).contains(&r.mo) && (1..=31).contains(&r.d)
        && r.h < 24 && r.mi < 60 && r.s < 60
}

/// Write `(y,mo,d,h,mi,s)` back, in the encoding status B says the chip uses. Returns false on a bad date.
pub fn write(y: u32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> bool {
    if !(1980..=2107).contains(&y) || !(1..=12).contains(&mo) || !(1..=31).contains(&d)
        || h > 23 || mi > 59 || s > 59 {
        return false;
    }
    let cent = fadt_century_reg();
    // SAFETY: CMOS ports; SET bit (status B bit 7) halts updates while the registers are rewritten.
    unsafe {
        let sb = cmos_read(0x0B);
        let bcd = sb & 0x04 == 0;
        let h24 = sb & 0x02 != 0;
        let enc = |v: u32| if bcd { bin2bcd(v) } else { v as u8 };
        let hh = if h24 { enc(h) } else {
            let pm = h >= 12;
            let h12 = if h % 12 == 0 { 12 } else { h % 12 };
            enc(h12) | if pm { 0x80 } else { 0 }
        };
        cmos_write(0x0B, sb | 0x80);
        cmos_write(0x00, enc(s));
        cmos_write(0x02, enc(mi));
        cmos_write(0x04, hh);
        cmos_write(0x07, enc(d));
        cmos_write(0x08, enc(mo));
        cmos_write(0x09, enc(y % 100));
        if let Some(c) = cent { cmos_write(c, enc(y / 100)); }
        cmos_write(0x0B, sb & !0x80);
    }
    true
}

/// Boot step: read the RTC, print the `:: RTC:` witness, and (when plausible) anchor the civil clock from it
/// with source `Rtc`. `UNAOS_TZ_MIN` (minutes, signed, default 0) is added for display: the RTC is read as UTC.
pub fn boot_anchor() {
    match read_full() {
        None => serial_println!(":: RTC: y=0 mo=0 d=0 h=0 mi=0 s=0 bcd=0 h24=0 century=assumed reads=inconsistent -> FAIL ::"),
        Some(r) => {
            let ok = plausible(&r);
            crate::census_println!(
                ":: RTC: y={} mo={} d={} h={} mi={} s={} bcd={} h24={} century={} tz_min={} -> {} ::",
                r.y, r.mo, r.d, r.h, r.mi, r.s, r.bcd as u8, r.h24 as u8,
                if r.century_reg.is_some() { "fadt" } else { "assumed" },
                crate::clock::TZ_MIN, if ok { "PASS" } else { "FAIL" });
            if ok {
                let unix = crate::clock::unix_from_civil(r.y as i64, r.mo, r.d, r.h, r.mi, r.s);
                crate::clock::anchor_from_rtc(unix);
            }
        }
    }
}
