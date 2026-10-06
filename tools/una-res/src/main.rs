// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! una-res — APPRES (rmbp-ledger B398, MACPARITY §16 B4 / row 38): a program's RESOURCES.
//!
//! A program crate carries a `res/` directory: `app.res` (`key = value` lines: `name`, `signature`,
//! `version`, `kind`, `doctypes` — a comma list of MIME types — and `droptypes`, the same shape, B477) and `icon.svg`. This tool turns it into the
//! resource block `midden_core::res_build` defines — the four text keys, the SVG source, the icon rendered
//! by svg_core at 32, 64 and 128 px and written as RGBA PNGs by pixel_core, and the doc types — and either
//!
//!   * `una-res stamp <res-dir> <elf>`: appends it to a built (stripped) ELF as the NON-ALLOC `SHT_NOTE`
//!     section `.note.unaos.res` (new data, a new `.shstrtab`, a new section-header table at the end of the
//!     file; `e_shoff`/`e_shnum`/`e_shstrndx` patched). No program header names it, so the loader never maps
//!     it. An ELF already stamped is refused; arroyo stamps the fresh objcopy output; or
//!   * `una-res pack <res-dir> <out>`: writes the bare block (the kernel's own apps compile it in);
//!   * `una-res show <elf|block>`: prints the records.
//!
//! The version gets `+<build>` appended when it has none: `--build <id>`, else `$UNAOS_GIT_SHA` (arroyo
//! exports it), else nothing.
use std::path::Path;

use midden_core as mc;

fn die(msg: &str) -> ! {
    eprintln!("una-res: {msg}");
    std::process::exit(1);
}

/// Parse `app.res`: `key = value` lines, `#` comments.
fn parse_app_res(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (k, v) = l.split_once('=').ok_or_else(|| format!("app.res line {}: no '='", n + 1))?;
        let (k, v) = (k.trim(), v.trim());
        match k {
            "name" | "signature" | "version" | "kind" | "doctypes" | "droptypes" => out.push((k.to_string(), v.to_string())),
            _ => return Err(format!("app.res line {}: unknown key {k}", n + 1)),
        }
    }
    for need in ["name", "signature", "version", "kind"] {
        if !out.iter().any(|(k, _)| k == need) {
            return Err(format!("app.res: missing {need}"));
        }
    }
    let kind = &out.iter().find(|(k, _)| k == "kind").unwrap().1;
    if !matches!(kind.as_str(), "windowed" | "resident" | "console") {
        return Err(format!("app.res: kind {kind} is not windowed|resident|console"));
    }
    let sig = &out.iter().find(|(k, _)| k == "signature").unwrap().1;
    if sig.split('.').count() < 3 || sig.chars().any(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '-')) {
        return Err(format!("app.res: signature {sig} is not reverse-DNS"));
    }
    Ok(out)
}

/// Render `svg` at `px` square and encode it as an RGBA PNG.
fn icon_png(svg: &[u8], px: u32) -> Result<Vec<u8>, String> {
    let doc = svg_core::Svg::parse(svg).map_err(|e| format!("icon.svg: {e:?}"))?;
    let opts = svg_core::Options { fonts: None, image_decoder: None, languages: Vec::new() };
    let rgba = doc.render_rgba(px, px, &opts).map_err(|e| format!("icon.svg at {px}: {e:?}"))?;
    pixel_core::png::encode::encode_rgba(px, px, &rgba).map_err(|e| format!("png {px}: {e:?}"))
}

/// The resource block for one `res/` directory.
pub fn build_block(dir: &Path, build: Option<&str>) -> Result<Vec<u8>, String> {
    let text = std::fs::read_to_string(dir.join("app.res")).map_err(|e| format!("{}/app.res: {e}", dir.display()))?;
    let kv = parse_app_res(&text)?;
    let get = |k: &str| kv.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
    let mut version = get("version").unwrap();
    if let Some(b) = build.filter(|b| !b.is_empty()) {
        if !version.contains('+') {
            version = format!("{version}+{b}");
        }
    }
    let doctypes: Vec<String> = get("doctypes")
        .map(|d| d.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let droptypes: Vec<String> = get("droptypes")
        .map(|d| d.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let svg = std::fs::read(dir.join("icon.svg")).map_err(|e| format!("{}/icon.svg: {e}", dir.display()))?;
    let pngs: Vec<Vec<u8>> = mc::RES_ICON_SIZES.iter().map(|(px, _)| icon_png(&svg, *px)).collect::<Result<_, _>>()?;
    let (name, sig, kind) = (get("name").unwrap(), get("signature").unwrap(), get("kind").unwrap());
    let doc_join = doctypes.join("\n");
    let drop_join = droptypes.join("\n");
    let mut recs: Vec<(&str, &[u8])> = vec![
        (mc::RES_KEY_NAME, name.as_bytes()),
        (mc::RES_KEY_SIGNATURE, sig.as_bytes()),
        (mc::RES_KEY_VERSION, version.as_bytes()),
        (mc::RES_KEY_KIND, kind.as_bytes()),
        (mc::RES_KEY_ICON_SVG, &svg),
    ];
    for ((_, key), png) in mc::RES_ICON_SIZES.iter().zip(pngs.iter()) {
        recs.push((key, png));
    }
    if !doc_join.is_empty() {
        recs.push((mc::RES_KEY_DOCTYPES, doc_join.as_bytes()));
    }
    if !drop_join.is_empty() {
        recs.push((mc::RES_KEY_DROPTYPES, drop_join.as_bytes())); // DROPTYPES (B477): what the program's windows take as a drop
    }
    mc::res_build(&recs).ok_or_else(|| "a record does not encode".to_string())
}

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn rd64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Append the resource note to an ELF64 LE image as the non-alloc `SHT_NOTE` section `.note.unaos.res`.
pub fn stamp(elf: &[u8], block: &[u8]) -> Result<Vec<u8>, String> {
    let (shoff, shnum) = mc::elf_shdr_table(elf).ok_or("not an ELF64 LE image with a section table")?;
    if mc::app_res(elf).is_some() {
        return Err("already carries a resource note".into());
    }
    let table = elf.get(shoff..shoff + shnum * 64).ok_or("section table runs off the file")?.to_vec();
    let shstrndx = rd16(elf, 62) as usize;
    if shstrndx == 0 || shstrndx >= shnum {
        return Err("no section-name table".into());
    }
    let strh = &table[shstrndx * 64..shstrndx * 64 + 64];
    let (str_off, str_sz) = (rd64(strh, 24) as usize, rd64(strh, 32) as usize);
    let mut names = elf.get(str_off..str_off + str_sz).ok_or("section-name table runs off the file")?.to_vec();
    let name_idx = names.len() as u32;
    names.extend_from_slice(mc::APPRES_SECTION_NAME.as_bytes());
    names.push(0);
    let note = mc::res_note(block).ok_or("block too large")?;

    let mut out = elf.to_vec();
    while out.len() % 8 != 0 {
        out.push(0);
    }
    let note_off = out.len();
    out.extend_from_slice(&note);
    let names_off = out.len();
    out.extend_from_slice(&names);
    while out.len() % 8 != 0 {
        out.push(0);
    }
    let new_shoff = out.len();
    let mut new_table = table;
    new_table[shstrndx * 64 + 24..shstrndx * 64 + 32].copy_from_slice(&(names_off as u64).to_le_bytes());
    new_table[shstrndx * 64 + 32..shstrndx * 64 + 40].copy_from_slice(&(names.len() as u64).to_le_bytes());
    let mut sh = [0u8; 64];
    sh[0..4].copy_from_slice(&name_idx.to_le_bytes());
    sh[4..8].copy_from_slice(&7u32.to_le_bytes()); // SHT_NOTE; sh_flags 0 (not SHF_ALLOC), sh_addr 0
    sh[24..32].copy_from_slice(&(note_off as u64).to_le_bytes());
    sh[32..40].copy_from_slice(&(note.len() as u64).to_le_bytes());
    sh[48..56].copy_from_slice(&4u64.to_le_bytes()); // sh_addralign
    new_table.extend_from_slice(&sh);
    out.extend_from_slice(&new_table);
    let new_num = u16::try_from(shnum + 1).map_err(|_| "too many sections")?;
    out[40..48].copy_from_slice(&(new_shoff as u64).to_le_bytes());
    out[60..62].copy_from_slice(&new_num.to_le_bytes());
    // The program headers are untouched: every PT_LOAD still names the same bytes.
    let _ = rd32;
    Ok(out)
}

fn show(bytes: &[u8]) {
    let block = mc::app_res(bytes).unwrap_or(bytes);
    let Some(recs) = mc::res_records(block) else { die("no resource block") };
    for (k, v) in recs {
        match std::str::from_utf8(v) {
            Ok(s) if !k.starts_with(midden_core::RES_KEY_ICON_PREFIX) => println!("{k} = {}", s.replace('\n', ", ")),
            _ => println!("{k} = <{} bytes>", v.len()),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut build: Option<String> = std::env::var("UNAOS_GIT_SHA").ok();
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--build" {
            build = args.get(i + 1).cloned();
            i += 2;
            continue;
        }
        pos.push(&args[i]);
        i += 1;
    }
    match pos.as_slice() {
        ["stamp", dir, elf] => {
            let block = build_block(Path::new(dir), build.as_deref()).unwrap_or_else(|e| die(&e));
            let bytes = std::fs::read(elf).unwrap_or_else(|e| die(&format!("{elf}: {e}")));
            let out = stamp(&bytes, &block).unwrap_or_else(|e| die(&format!("{elf}: {e}")));
            std::fs::write(elf, &out).unwrap_or_else(|e| die(&format!("{elf}: {e}")));
            println!("una-res: stamped {elf} block={} bytes file={} -> {}", block.len(), bytes.len(), out.len());
        }
        ["pack", dir, out] => {
            let block = build_block(Path::new(dir), build.as_deref()).unwrap_or_else(|e| die(&e));
            std::fs::write(out, &block).unwrap_or_else(|e| die(&format!("{out}: {e}")));
            println!("una-res: packed {out} block={} bytes", block.len());
        }
        ["show", f] => show(&std::fs::read(f).unwrap_or_else(|e| die(&format!("{f}: {e}")))),
        _ => die("usage: una-res [--build <id>] stamp <res-dir> <elf> | pack <res-dir> <out> | show <elf|block>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect x="4" y="4" width="56" height="56" rx="12" fill="#3a6ea5"/><circle cx="32" cy="32" r="12" fill="#fff"/></svg>"##;

    fn res_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("una-res-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("app.res"), "name = Test\nsignature = org.unaos.test\nversion = 0.1.0\nkind = windowed\ndoctypes = text/plain, image/png\ndroptypes = text/*, image/png\n").unwrap();
        std::fs::write(d.join("icon.svg"), SVG).unwrap();
        d
    }

    /// A minimal stripped-shape ELF: header, one PT_LOAD, a `.shstrtab`, and a section table.
    fn elf() -> Vec<u8> {
        let mut b = vec![0u8; 0x100];
        b[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        b[4] = 2;
        b[5] = 1;
        b[16..18].copy_from_slice(&2u16.to_le_bytes());
        b[18..20].copy_from_slice(&62u16.to_le_bytes());
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&1u16.to_le_bytes());
        b[64..68].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        let strs = b"\0.shstrtab\0";
        let str_off = b.len();
        b.extend_from_slice(strs);
        while b.len() % 8 != 0 {
            b.push(0);
        }
        let shoff = b.len();
        b.extend_from_slice(&[0u8; 64]);
        let mut sh = [0u8; 64];
        sh[0..4].copy_from_slice(&1u32.to_le_bytes());
        sh[4..8].copy_from_slice(&3u32.to_le_bytes());
        sh[24..32].copy_from_slice(&(str_off as u64).to_le_bytes());
        sh[32..40].copy_from_slice(&(strs.len() as u64).to_le_bytes());
        b.extend_from_slice(&sh);
        b[40..48].copy_from_slice(&(shoff as u64).to_le_bytes());
        b[58..60].copy_from_slice(&64u16.to_le_bytes());
        b[60..62].copy_from_slice(&2u16.to_le_bytes());
        b[62..64].copy_from_slice(&1u16.to_le_bytes());
        b
    }

    #[test]
    fn stamp_round_trips_and_the_icons_decode() {
        let d = res_dir();
        let block = build_block(&d, Some("abc1234")).unwrap();
        let img = elf();
        let out = stamp(&img, &block).unwrap();
        assert_eq!(&out[..0x40][..32], &img[..32], "the header up to e_shoff is unchanged");
        assert_eq!(&out[64..120], &img[64..120], "the program header is unchanged");
        let got = mc::app_res(&out).unwrap();
        assert_eq!(got, &block[..]);
        assert_eq!(mc::res_str(got, mc::RES_KEY_VERSION), Some("0.1.0+abc1234"));
        assert_eq!(mc::res_str(got, mc::RES_KEY_DOCTYPES), Some("text/plain\nimage/png"));
        assert_eq!(mc::res_str(got, mc::RES_KEY_DROPTYPES), Some("text/*\nimage/png"));
        for (px, key) in mc::RES_ICON_SIZES {
            let png = mc::res_get(got, key).unwrap();
            let im = pixel_core::decode_png(png).unwrap();
            assert_eq!((im.width, im.height), (px, px));
            let c = ((px / 2) * px + px / 2) as usize * 4;
            assert_eq!(&im.rgba[c..c + 4], &[255, 255, 255, 255], "{px}: the centre is the white disc");
            assert_eq!(im.rgba[3], 0, "{px}: the corner is transparent");
            println!("icon {px}px = {} bytes", png.len());
        }
        // The section name resolves through the NEW .shstrtab.
        let (shoff, n) = mc::elf_shdr_table(&out).unwrap();
        let last = &out[shoff + (n - 1) * 64..shoff + n * 64];
        let strh = &out[shoff + 64..shoff + 128];
        let (so, nm) = (rd64(strh, 24) as usize, rd32(last, 0) as usize);
        assert!(out[so + nm..].starts_with(b".note.unaos.res\0"));
        assert!(stamp(&out, &block).is_err(), "a second stamp is refused");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn app_res_is_validated() {
        assert!(parse_app_res("name = A\nsignature = org.unaos.a\nversion = 1\nkind = windowed\n").is_ok());
        assert!(parse_app_res("name = A\nsignature = a\nversion = 1\nkind = windowed\n").is_err());
        assert!(parse_app_res("name = A\nsignature = org.unaos.a\nversion = 1\nkind = daemon\n").is_err());
        assert!(parse_app_res("name = A\nsignature = org.unaos.a\nkind = windowed\n").is_err());
        assert!(parse_app_res("nme = A\n").is_err());
    }
}
