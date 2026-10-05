// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The LAYOUT — what an operator (or the installer ensemble, over the bus) asks for, as text. A
//! layout is disk-size free: [`LayoutSpec::plan`] lays it on a medium of a given size through
//! `amber_core::Plan::layout`, so one layout file plans a 200 MiB card and a 2 TB disk alike, and
//! the signature `PlanLayout` shows covers the plan for THAT medium (see [`crate::signer`]).
//!
//! ```text
//! amber-layout v1
//! disk-seed UNAOS-X86-CARD
//! part esp 262144s UNAOS-ESP seed=UNAOS-X86-ESP format=fat32 label=UNAOS
//! part unafs rest UNAOS-UNAFS format=unafs
//! ```
//!
//! Sizes: `<n>s` sectors, `<n>K`/`<n>M`/`<n>G` (binary units, a whole number of sectors), or
//! `rest`. A part's GUID seed defaults to `<disk-seed>/<name>`; a FAT32 volume serial is the CRC-32
//! of the part's seed, so a layout is fully deterministic. The compact CLI form of a part is
//! `kind:size:name[:format]` (`esp:128M:UNAOS-ESP:fat32`).

use std::fmt::{self, Write as _};

use amber_core::plan::{PartKind, PartReq, Plan, PlanError, Size};

/// What to put inside a partition after the table is laid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fs {
    None,
    Fat32,
    Unafs,
}

impl Fs {
    pub fn tag(self) -> &'static str {
        match self {
            Fs::None => "none",
            Fs::Fat32 => "fat32",
            Fs::Unafs => "unafs",
        }
    }
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "none" => Ok(Fs::None),
            "fat32" | "vfat" => Ok(Fs::Fat32),
            "unafs" => Ok(Fs::Unafs),
            _ => Err(format!("unknown format `{s}` (fat32, unafs, none)")),
        }
    }
}

/// One requested partition.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PartSpec {
    pub kind: PartKind,
    /// `None` = rest of the disk.
    pub sectors: Option<u64>,
    pub name: String,
    pub seed: Option<String>,
    pub format: Fs,
    /// FAT32 label (defaults to the part name).
    pub label: Option<String>,
    /// FAT32 sectors per cluster (defaults to the fatgen103 table).
    pub spc: Option<u8>,
}

/// A whole layout.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LayoutSpec {
    pub disk_seed: String,
    pub parts: Vec<PartSpec>,
}

fn kind(s: &str) -> Result<PartKind, String> {
    match s {
        "esp" => Ok(PartKind::Esp),
        "unafs" => Ok(PartKind::UnaFS),
        "data" => Ok(PartKind::Data),
        _ => Err(format!("unknown partition kind `{s}` (esp, unafs, data)")),
    }
}

/// `<n>s`, `<n>K|M|G`, or `rest` → sectors (`None` = rest).
pub fn parse_size(s: &str) -> Result<Option<u64>, String> {
    if s == "rest" {
        return Ok(None);
    }
    let (num, mul) = match s.as_bytes().last() {
        Some(b's') => (&s[..s.len() - 1], 1u64),
        Some(b'K') => (&s[..s.len() - 1], 2),
        Some(b'M') => (&s[..s.len() - 1], 2048),
        Some(b'G') => (&s[..s.len() - 1], 2048 * 1024),
        _ => return Err(format!("size `{s}` needs a unit: s, K, M, G, or `rest`")),
    };
    let n: u64 = num.parse().map_err(|_| format!("size `{s}` is not a number"))?;
    match n.checked_mul(mul) {
        Some(v) if v > 0 => Ok(Some(v)),
        _ => Err(format!("size `{s}` is zero or too large")),
    }
}

fn token_ok(s: &str) -> Result<(), String> {
    if s.is_empty() || s.len() > 36 || s.bytes().any(|b| !(0x21..0x7f).contains(&b)) {
        return Err(format!("`{s}`: names and seeds are 1..=36 printable ASCII characters, no spaces"));
    }
    Ok(())
}

impl PartSpec {
    /// The compact CLI form `kind:size:name[:format]`.
    pub fn parse_compact(s: &str) -> Result<Self, String> {
        let f: Vec<&str> = s.split(':').collect();
        if !(3..=4).contains(&f.len()) {
            return Err(format!("`{s}`: expected kind:size:name[:format]"));
        }
        token_ok(f[2])?;
        Ok(Self {
            kind: kind(f[0])?,
            sectors: parse_size(f[1])?,
            name: f[2].to_string(),
            seed: None,
            format: f.get(3).map(|x| Fs::parse(x)).transpose()?.unwrap_or(Fs::None),
            label: None,
            spc: None,
        })
    }
    /// The seed the part's unique GUID derives from.
    pub fn seed_for(&self, disk_seed: &str) -> String {
        self.seed.clone().unwrap_or_else(|| format!("{disk_seed}/{}", self.name))
    }
}

impl LayoutSpec {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
        if lines.next() != Some("amber-layout v1") {
            return Err("not an amber layout (first line must be `amber-layout v1`)".into());
        }
        let mut disk_seed = None;
        let mut parts = Vec::new();
        for l in lines {
            let w: Vec<&str> = l.split_whitespace().collect();
            match w.first().copied() {
                Some("disk-seed") if w.len() == 2 => {
                    token_ok(w[1])?;
                    disk_seed = Some(w[1].to_string());
                }
                Some("part") if w.len() >= 4 => {
                    token_ok(w[3])?;
                    let mut p = PartSpec {
                        kind: kind(w[1])?,
                        sectors: parse_size(w[2])?,
                        name: w[3].to_string(),
                        seed: None,
                        format: Fs::None,
                        label: None,
                        spc: None,
                    };
                    for kv in &w[4..] {
                        let (k, v) = kv.split_once('=').ok_or_else(|| format!("`{kv}`: expected key=value"))?;
                        match k {
                            "seed" => {
                                token_ok(v)?;
                                p.seed = Some(v.to_string())
                            }
                            "format" => p.format = Fs::parse(v)?,
                            "label" => {
                                token_ok(v)?;
                                p.label = Some(v.to_string())
                            }
                            "spc" => {
                                let n: u8 = v.parse().map_err(|_| format!("spc `{v}`"))?;
                                if !n.is_power_of_two() || n > 128 {
                                    return Err(format!("spc `{v}` is not a power of two in 1..=128"));
                                }
                                p.spc = Some(n)
                            }
                            _ => return Err(format!("unknown part option `{k}`")),
                        }
                    }
                    parts.push(p);
                }
                _ => return Err(format!("unrecognised layout line `{l}`")),
            }
        }
        let disk_seed = disk_seed.ok_or("the layout has no `disk-seed` line")?;
        if parts.is_empty() {
            return Err("the layout has no partitions".into());
        }
        if parts.len() > 128 {
            return Err("more than 128 partitions".into());
        }
        Ok(Self { disk_seed, parts })
    }

    /// The canonical text (what `parse` reads back to an equal spec).
    pub fn emit(&self) -> String {
        let mut s = String::from("amber-layout v1\n");
        let _ = writeln!(s, "disk-seed {}", self.disk_seed);
        for p in &self.parts {
            let size = p.sectors.map_or("rest".to_string(), |n| format!("{n}s"));
            let _ = write!(s, "part {} {} {} seed={} format={}", p.kind.tag(), size, p.name, p.seed_for(&self.disk_seed), p.format.tag());
            if let Some(l) = &p.label {
                let _ = write!(s, " label={l}");
            }
            if let Some(n) = p.spc {
                let _ = write!(s, " spc={n}");
            }
            s.push('\n');
        }
        s
    }

    fn with_reqs<T>(&self, f: impl FnOnce(&[PartReq<'_>]) -> T) -> T {
        let seeds: Vec<String> = self.parts.iter().map(|p| p.seed_for(&self.disk_seed)).collect();
        let reqs: Vec<PartReq<'_>> = self
            .parts
            .iter()
            .zip(&seeds)
            .map(|(p, seed)| PartReq {
                kind: p.kind,
                size: p.sectors.map_or(Size::Rest, Size::Sectors),
                name: &p.name,
                seed: seed.as_bytes(),
            })
            .collect();
        f(&reqs)
    }

    /// Lay the layout on a medium of `disk_sectors` sectors. A `rest` part that finds no room is
    /// an error here (the core drops it silently; an operator must hear about it).
    pub fn plan(&self, disk_sectors: u64) -> Result<Plan, String> {
        let plan = self.with_reqs(|r| Plan::layout(disk_sectors, self.disk_seed.as_bytes(), r)).map_err(plan_err)?;
        if plan.parts.len() != self.parts.len() {
            return Err(format!("no room left for `{}`", self.parts[plan.parts.len()].name));
        }
        Ok(plan)
    }

    /// The smallest image (whole MiB) that holds every part — every size must be fixed.
    pub fn image_sectors(&self) -> Result<u64, String> {
        if self.parts.iter().any(|p| p.sectors.is_none()) {
            return Err("an image is sized from its parts: `rest` needs an existing medium".into());
        }
        Ok(self.with_reqs(|r| Plan::for_image(self.disk_seed.as_bytes(), r)).map_err(plan_err)?.disk_sectors)
    }
}

fn plan_err(e: PlanError) -> String {
    e.to_string()
}

impl fmt::Display for LayoutSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.emit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card layout `tools/una-card` lays, written as a layout file, plans to the golden card.
    pub(crate) const CARD: &str = "amber-layout v1\n# the x86 card\ndisk-seed UNAOS-X86-CARD\n\
        part esp 262144s UNAOS-ESP seed=UNAOS-X86-ESP format=fat32 label=UNAOS\n\
        part unafs 131072s UNAOS-UNAFS seed=UNAOS-X86-UFS format=unafs\n";

    #[test]
    fn card_layout_is_the_golden_plan() {
        let l = LayoutSpec::parse(CARD).unwrap();
        assert_eq!(l.image_sectors().unwrap(), amber_core::kat::CARD_TOTAL);
        assert_eq!(l.plan(amber_core::kat::CARD_TOTAL).unwrap(), amber_core::kat::golden_card_plan());
        assert_eq!(LayoutSpec::parse(&l.emit()).unwrap(), l, "emit/parse round trip");
    }

    #[test]
    fn sizes_and_refusals() {
        assert_eq!(parse_size("1M"), Ok(Some(2048)));
        assert_eq!(parse_size("3s"), Ok(Some(3)));
        assert_eq!(parse_size("rest"), Ok(None));
        assert!(parse_size("0M").is_err() && parse_size("12").is_err() && parse_size("xM").is_err());
        assert!(LayoutSpec::parse("amber-layout v1\npart esp 1M A\n").is_err(), "no disk-seed");
        assert!(LayoutSpec::parse("amber-layout v1\ndisk-seed X\n").is_err(), "no parts");
        assert!(LayoutSpec::parse("amber-layout v1\ndisk-seed X\npart esp 1M A bogus=1\n").is_err());
        assert!(LayoutSpec::parse("amber-layout v1\ndisk-seed X\npart esp 1M A spc=3\n").is_err());
        assert!(PartSpec::parse_compact("esp:128M:UNAOS-ESP:fat32").is_ok());
        assert!(PartSpec::parse_compact("swap:1M:X").is_err());
        // A `rest` with nothing left is reported, not dropped.
        let l = LayoutSpec::parse("amber-layout v1\ndisk-seed X\npart data 4M A\npart data rest B\n").unwrap();
        assert!(l.plan(2048 + 8192 + 33).unwrap_err().contains("`B`"));
    }
}
