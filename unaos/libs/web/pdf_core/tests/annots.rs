// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDFFORMS (LEDGER SR326) KATs: hand-built 40 x 40 pt pages (72 dpi, so device y = 40 - user y). Reading: link
//! actions, destinations (explicit, the /Dests dictionary, the /Names name tree), refused actions, the field tree.
//! Painting: appearances through `Page::render` (sampled pixels from the §12.5.5 geometry AND an FNV-1a 64 pin of
//! this renderer's own output), /AS states, Hidden / NoView, generated appearances. Filling: `Form::set_value`.

mod common;

use common::annot_pdf;
use pdf_core::annots::{Dest, FieldKind, LinkAction, View, flags};
use pdf_core::render::{Rendered, TextLog};
use pdf_core::{Document, Error, Object, Ref};

fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn render(bytes: &[u8]) -> (Rendered, TextLog) {
    let doc = Document::parse(bytes).unwrap();
    let pages = doc.pages().unwrap();
    let mut log = TextLog::default();
    let r = pages[0].render(&doc, 72.0, &mut log).unwrap();
    (r, log)
}

fn px(r: &Rendered, x: usize, y: usize) -> [u8; 4] {
    let p = &r.pixmap;
    let i = (y * p.w + x) * 4;
    [p.data[i], p.data[i + 1], p.data[i + 2], p.data[i + 3]]
}

fn pin(r: &Rendered, want: u64) {
    let got = fnv(&r.pixmap.data);
    assert_eq!(got, want, "fnv pin: got {got:#018x}");
}

fn all_white(r: &Rendered) -> bool {
    r.pixmap.data.iter().all(|&b| b == 255)
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
fn rf(num: u32) -> Ref {
    Ref { num, generation: 0 }
}

#[test]
fn link_uri() {
    let pdf = annot_pdf(
        "",
        &[("/Annots [20 0 R]", "")],
        &[(20, "<< /Type /Annot /Subtype /Link /Rect [15 15 5 5] /Contents (go) /A << /S /URI /URI (https://unaos.org/a) >> >>", None)],
    );
    let doc = Document::parse(&pdf).unwrap();
    let pages = doc.pages().unwrap();
    assert_eq!(pages[0].obj, Some(rf(3)));
    let a = pages[0].annots(&doc);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].obj, Some(rf(20)));
    assert_eq!(a[0].subtype, b"Link");
    assert_eq!(a[0].rect, [5.0, 5.0, 15.0, 15.0]);
    assert_eq!(a[0].contents.as_deref(), Some("go"));
    assert_eq!(a[0].link, Some(LinkAction::Uri("https://unaos.org/a".into())));
    assert_eq!(a[0].field, None);
    assert_eq!(a[0].appearance, None);
    assert!(a[0].visible());
}

#[test]
fn goto_page2() {
    let pdf = annot_pdf(
        "/Dests 30 0 R /Names << /Dests 31 0 R >>",
        &[("/Annots [20 0 R 21 0 R 22 0 R 23 0 R 24 0 R]", ""), ("", "")],
        &[
            (20, "<< /Subtype /Link /Rect [0 0 1 1] /Dest [5 0 R /XYZ 1 2 3] >>", None),
            (21, "<< /Subtype /Link /Rect [0 0 1 1] /A << /S /GoTo /D /Chap2 >> >>", None),
            (22, "<< /Subtype /Link /Rect [0 0 1 1] /A << /S /GoTo /D (second) >> >>", None),
            (23, "<< /Subtype /Link /Rect [0 0 1 1] /Dest (alpha) >>", None),
            (24, "<< /Subtype /Link /Rect [0 0 1 1] /Dest (missing) >>", None),
            (30, "<< /Chap2 [5 0 R /FitH 33] >>", None),
            (31, "<< /Kids [32 0 R 33 0 R] >>", None),
            (32, "<< /Limits [(a) (c)] /Names [(alpha) [3 0 R /Fit]] >>", None),
            (33, "<< /Limits [(m) (z)] /Names [(second) << /D [5 0 R /XYZ 10 20 0] >>] >>", None),
        ],
    );
    let doc = Document::parse(&pdf).unwrap();
    let pages = doc.pages().unwrap();
    assert_eq!(pages[1].obj, Some(rf(5)));
    let a = pages[0].annots(&doc);
    let link = |i: usize| a[i].link.clone();
    let goto = |page, view| Some(LinkAction::GoTo(Dest { page, view }));
    assert_eq!(link(0), goto(1, View::Xyz { left: Some(1.0), top: Some(2.0), zoom: Some(3.0) }));
    assert_eq!(link(1), goto(1, View::FitH(Some(33.0))));
    assert_eq!(link(2), goto(1, View::Xyz { left: Some(10.0), top: Some(20.0), zoom: None }));
    assert_eq!(link(3), goto(0, View::Fit));
    assert_eq!(link(4), None);
    assert_eq!(doc.dest(b"second"), Some(Dest { page: 1, view: View::Xyz { left: Some(10.0), top: Some(20.0), zoom: None } }));
    assert_eq!(doc.dest(b"Chap2"), Some(Dest { page: 1, view: View::FitH(Some(33.0)) }));
    assert_eq!(doc.dest(b"zz"), None);
}

#[test]
fn refused_actions_by_name() {
    let pdf = annot_pdf(
        "",
        &[("/Annots [20 0 R 21 0 R 22 0 R 23 0 R]", "")],
        &[
            (20, "<< /Subtype /Link /Rect [0 0 1 1] /A << /S /JavaScript /JS (app.alert\\(1\\)) >> >>", None),
            (21, "<< /Subtype /Link /Rect [0 0 1 1] /A << /S /GoToR /F (other.pdf) /D [0 /Fit] >> >>", None),
            (22, "<< /Subtype /Link /Rect [0 0 1 1] /A << /S /Launch /F (x) >> >>", None),
            (23, "<< /Subtype /Link /Rect [0 0 1 1] /A << /S /Named /N /NextPage >> >>", None),
        ],
    );
    let doc = Document::parse(&pdf).unwrap();
    let a = doc.pages().unwrap()[0].annots(&doc);
    assert_eq!(a[0].link, Some(LinkAction::Refused("JavaScript")));
    assert_eq!(a[1].link, Some(LinkAction::Refused("GoToR")));
    assert_eq!(a[2].link, Some(LinkAction::Refused("Launch")));
    assert_eq!(a[3].link, Some(LinkAction::Named(b"NextPage".to_vec())));
}

#[test]
fn appearance_placed_on_rect() {
    let pdf = annot_pdf(
        "",
        &[("/Annots [20 0 R 22 0 R 24 0 R]", "")],
        &[
            // A text field's own /AP: BBox 20 x 10 onto Rect [10 10 30 20], a translation.
            (20, "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (x) /Rect [10 10 30 20] /AP << /N 21 0 R >> >>", None),
            (21, "/Type /XObject /Subtype /Form /BBox [0 0 20 10]", Some(b"0 0 1 rg 0 0 20 10 re f")),
            // BBox 40 x 40 scaled onto Rect [0 0 10 10]: its left half (x < 20) is red.
            (22, "<< /Subtype /Square /Rect [0 0 10 10] /AP << /N 23 0 R >> >>", None),
            (23, "/Subtype /Form /BBox [0 0 40 40]", Some(b"1 0 0 rg 0 0 20 40 re f")),
            // /Matrix [0 1 -1 0 0 0] (90 degrees): BBox 20 x 10 becomes x -10..0, y 0..20; onto Rect [30 30 40 40]
            // (sx 1, sy 0.5) the form's x 0..10 half lands on user y 30..35.
            (24, "<< /Subtype /Square /Rect [30 30 40 40] /AP << /N 25 0 R >> >>", None),
            (25, "/Subtype /Form /BBox [0 0 20 10] /Matrix [0 1 -1 0 0 0]", Some(b"0 1 0 rg 0 0 10 10 re f")),
        ],
    );
    let (r, _) = render(&pdf);
    let blue = [0, 0, 255, 255];
    assert_eq!(px(&r, 20, 25), blue);
    assert_eq!(px(&r, 10, 20), blue);
    assert_eq!(px(&r, 29, 29), blue);
    assert_eq!(px(&r, 9, 25), WHITE);
    assert_eq!(px(&r, 30, 25), WHITE);
    assert_eq!(px(&r, 20, 19), WHITE);
    assert_eq!(px(&r, 20, 30), WHITE);
    assert_eq!(px(&r, 2, 35), [255, 0, 0, 255]);
    assert_eq!(px(&r, 7, 35), WHITE);
    assert_eq!(px(&r, 35, 7), [0, 255, 0, 255]);
    assert_eq!(px(&r, 35, 2), WHITE);
    assert!(r.refused.is_empty());
    pin(&r, 0xa3db5d800be921d1);
}

fn checkbox_pdf(state: &str) -> Vec<u8> {
    annot_pdf(
        "",
        &[("/Annots [20 0 R]", "")],
        &[
            (20, &format!("<< /Subtype /Widget /FT /Btn /T (cb) /V /{state} /AS /{state} /Rect [0 0 20 20] /AP << /N << /On 21 0 R /Off 22 0 R >> >> >>"), None),
            (21, "/Subtype /Form /BBox [0 0 20 20]", Some(b"0 0 0 rg 0 0 20 20 re f")),
            (22, "/Subtype /Form /BBox [0 0 20 20]", Some(b"1 0 0 rg 0 0 20 20 re f")),
        ],
    )
}

#[test]
fn checkbox_as_on_off() {
    let (on, _) = render(&checkbox_pdf("On"));
    assert_eq!(px(&on, 10, 30), [0, 0, 0, 255]);
    assert_eq!(px(&on, 10, 10), WHITE);
    pin(&on, 0xab30a3a518db51e5);
    let (off, _) = render(&checkbox_pdf("Off"));
    assert_eq!(px(&off, 10, 30), [255, 0, 0, 255]);
    pin(&off, 0x48a97ddad40c7445);
    let doc = Document::parse(&checkbox_pdf("On")).unwrap();
    let a = doc.pages().unwrap()[0].annots(&doc);
    assert_eq!(a[0].appearance.as_ref().unwrap().data, b"0 0 0 rg 0 0 20 20 re f");
}

#[test]
fn hidden_and_noview_not_painted() {
    let ap = "/Subtype /Form /BBox [0 0 40 40]";
    let pdf = annot_pdf(
        "",
        &[("/Annots [20 0 R 21 0 R 22 0 R]", "")],
        &[
            (20, "<< /Subtype /Square /F 2 /Rect [0 0 40 40] /AP << /N 30 0 R >> >>", None),
            (21, "<< /Subtype /Square /F 32 /Rect [0 0 40 40] /AP << /N 30 0 R >> >>", None),
            (22, "<< /Subtype /Square /F 4 /Rect [0 0 10 10] /AP << /N 30 0 R >> >>", None),
            (30, ap, Some(b"0 0 1 rg 0 0 40 40 re f")),
        ],
    );
    let (r, _) = render(&pdf);
    assert_eq!(px(&r, 5, 35), [0, 0, 255, 255]);
    assert_eq!(px(&r, 25, 15), WHITE);
    assert_eq!(px(&r, 15, 35), WHITE);
    let doc = Document::parse(&pdf).unwrap();
    let a = doc.pages().unwrap()[0].annots(&doc);
    assert_eq!((a[0].flags, a[1].flags, a[2].flags), (flags::HIDDEN, flags::NO_VIEW, flags::PRINT));
    assert_eq!((a[0].visible(), a[1].visible(), a[2].visible()), (false, false, true));
    pin(&r, 0xd301bdb896919a8d);
}

fn tree_pdf() -> Vec<u8> {
    let on = Some(&b"0 g 0 0 10 10 re f"[..]);
    let off = Some(&b""[..]);
    annot_pdf(
        "/AcroForm 30 0 R",
        &[("/Annots [43 0 R 42 0 R 45 0 R 52 0 R 53 0 R 47 0 R 48 0 R]", "")],
        &[
            (30, "<< /Fields [40 0 R 45 0 R 46 0 R 47 0 R 48 0 R] /DA (/Helv 0 Tf 0 g) /XFA [(template) (x)] /NeedAppearances true >>", None),
            (40, "<< /T (person) /FT /Tx /Kids [41 0 R 42 0 R] >>", None),
            (41, "<< /T (name) /Parent 40 0 R /Kids [43 0 R] /V (Ada) >>", None),
            (43, "<< /Type /Annot /Subtype /Widget /Parent 41 0 R /Rect [1 2 21 12] >>", None),
            (42, "<< /T (age) /Parent 40 0 R /Subtype /Widget /Rect [1 20 21 30] /Ff 4096 >>", None),
            (45, "<< /T (agree) /FT /Btn /V /Yes /Subtype /Widget /Rect [25 25 35 35] /AS /Yes /AP << /N << /Yes 50 0 R /Off 51 0 R >> >> >>", None),
            (46, "<< /T (colour) /FT /Btn /Ff 32768 /V /Blue /Kids [52 0 R 53 0 R] >>", None),
            (52, "<< /Subtype /Widget /Parent 46 0 R /Rect [0 30 10 40] /AS /Off /AP << /N << /Red 50 0 R /Off 51 0 R >> >> >>", None),
            (53, "<< /Subtype /Widget /Parent 46 0 R /Rect [10 30 20 40] /AS /Blue /AP << /N << /Blue 50 0 R /Off 51 0 R >> >> >>", None),
            (47, "<< /T (pick) /FT /Ch /Opt [(One) [(2) (Two)]] /V (One) /Subtype /Widget /Rect [0 0 5 5] >>", None),
            (48, "<< /T (sign) /FT /Sig /Subtype /Widget /Rect [0 0 1 1] >>", None),
            (50, "/Subtype /Form /BBox [0 0 10 10]", on),
            (51, "/Subtype /Form /BBox [0 0 10 10]", off),
        ],
    )
}

#[test]
fn field_tree_names_and_kinds() {
    let pdf = tree_pdf();
    let doc = Document::parse(&pdf).unwrap();
    let form = doc.form().unwrap();
    let names: Vec<&str> = form.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["person.name", "person.age", "agree", "colour", "pick", "sign"]);
    let kinds: Vec<FieldKind> = form.fields.iter().map(|f| f.kind).collect();
    use FieldKind::*;
    assert_eq!(kinds, [Text, Text, Checkbox, Radio, Choice, Signature]);
    let f = &form.fields;
    assert_eq!(f[0].value, Object::String(b"Ada".to_vec()));
    assert_eq!(f[0].obj, Some(rf(41)));
    assert_eq!(f[0].widgets, [rf(43)]);
    assert_eq!(f[0].rect, Some([1.0, 2.0, 21.0, 12.0]));
    assert_eq!((f[1].widgets.clone(), f[1].flags), (vec![rf(42)], 4096));
    assert_eq!(f[2].on_state.as_deref(), Some(&b"Yes"[..]));
    assert_eq!(f[3].widgets, [rf(52), rf(53)]);
    assert_eq!(f[3].value, Object::Name(b"Blue".to_vec()));
    assert_eq!(f[3].on_state.as_deref(), Some(&b"Red"[..]));
    assert_eq!(f[4].options, ["One", "Two"]);
    assert_eq!(form.refused, ["XFA"]);
    assert!(form.need_appearances);
    let a = doc.pages().unwrap()[0].annots(&doc);
    assert_eq!(a.len(), 7);
    let fr = a[0].field.as_ref().unwrap();
    assert_eq!((fr.obj, fr.name.as_str()), (Some(rf(41)), "person.name"));
    let fr = a[1].field.as_ref().unwrap();
    assert_eq!((fr.obj, fr.name.as_str()), (Some(rf(42)), "person.age"));
    assert_eq!(a[4].field.as_ref().unwrap().name, "colour");
    assert!(a.iter().all(|x| x.field.is_some()));
}

#[test]
fn generated_text_appearance() {
    let pdf = annot_pdf(
        "/AcroForm 30 0 R",
        &[("/Annots [40 0 R 41 0 R]", "")],
        &[
            (30, "<< /Fields [40 0 R 41 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 31 0 R >> >> >>", None),
            (31, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>", None),
            (40, "<< /T (who) /FT /Tx /V (Hello) /DA (/Helv 12 Tf 0 0 1 rg) /Subtype /Widget /Rect [4 10 36 30] >>", None),
            (41, "<< /T (auto) /FT /Tx /V (a\\(b\\)) /Subtype /Widget /Rect [0 0 20 10] >>", None),
        ],
    );
    let (_, log) = render(&pdf);
    assert_eq!(log.runs.len(), 2);
    let r0 = &log.runs[0];
    assert_eq!((r0.font_name.as_slice(), r0.size, r0.bytes.as_slice()), (&b"Helv"[..], 12.0, &b"Hello"[..]));
    // Td 2, (20 - 12) / 2 + 0.22 x 12 = 6.64; the CTM puts the form's origin at the Rect's (4, 10): device (4, 30).
    assert!((r0.tm.e - 2.0).abs() < 1e-9 && (r0.tm.f - 6.64).abs() < 1e-9);
    assert!((r0.ctm.e - 4.0).abs() < 1e-9 && (r0.ctm.f - 30.0).abs() < 1e-9 && (r0.ctm.d + 1.0).abs() < 1e-9);
    // Auto size: min(12, (10 - 4) x 0.75) = 4.5; the value's parentheses survive the escaping.
    let r1 = &log.runs[1];
    assert_eq!((r1.size, r1.bytes.as_slice()), (4.5, &b"a(b)"[..]));
}

fn gen_check_pdf(state: &str) -> Vec<u8> {
    annot_pdf(
        "",
        &[("/Annots [40 0 R]", "")],
        &[(40, &format!("<< /T (ok) /FT /Btn /V /{state} /AS /{state} /Subtype /Widget /Rect [10 10 30 30] /DA (0 g) >>"), None)],
    )
}

#[test]
fn generated_checkbox_appearance() {
    // The check's corner (0.4w, 0.25h) = user (18, 15): device (18, 25), under a 2 pt round-joined stroke.
    let (on, _) = render(&gen_check_pdf("Yes"));
    assert_ne!(px(&on, 18, 24), WHITE);
    assert_eq!(px(&on, 12, 12), WHITE);
    pin(&on, 0xb6b96de5fdbddafb);
    let (off, _) = render(&gen_check_pdf("Off"));
    assert!(all_white(&off));
}

fn fill_pdf() -> Vec<u8> {
    annot_pdf(
        "/AcroForm 30 0 R",
        &[("/Annots [40 0 R 41 0 R 42 0 R]", "")],
        &[
            (30, "<< /Fields [40 0 R 41 0 R 42 0 R] /DA (/Helv 12 Tf 0 g) >>", None),
            (40, "<< /T (who) /FT /Tx /V (Old) /Subtype /Widget /Rect [0 0 40 20] >>", None),
            (41, "<< /T (ok) /FT /Btn /V /Off /AS /Off /Subtype /Widget /Rect [0 20 20 40] /AP << /N << /Yes 50 0 R /Off 51 0 R >> >> >>", None),
            (42, "<< /T (sign) /FT /Sig /Subtype /Widget /Rect [0 0 1 1] >>", None),
            (50, "/Subtype /Form /BBox [0 0 20 20]", Some(b"0 g 0 0 20 20 re f")),
            (51, "/Subtype /Form /BBox [0 0 20 20]", Some(b"")),
        ],
    )
}

#[test]
fn set_value_edits() {
    let pdf = fill_pdf();
    let doc = Document::parse(&pdf).unwrap();
    let form = doc.form().unwrap();
    // Text: ONE edit of the merged field / widget (/V and /AP), then the new appearance at /Size (52).
    let e = form.set_value(&doc, &form.fields[0], "World").unwrap();
    assert_eq!(e.len(), 2);
    assert_eq!(e[0].0, rf(40));
    let d = e[0].1.as_dict().unwrap();
    assert_eq!(d.get(b"V"), Some(&Object::String(b"World".to_vec())));
    let ap = d.get(b"AP").unwrap().as_dict().unwrap();
    assert_eq!(ap.get(b"N"), Some(&Object::Ref(rf(52))));
    assert_eq!(e[1].0, rf(52));
    let s = e[1].1.as_stream().unwrap();
    assert!(s.data.windows(10).any(|w| w == b"(World) Tj"), "{}", String::from_utf8_lossy(&s.data));
    assert!(s.data.windows(13).any(|w| w == b"/Helv 12 Tf\n2"));
    // A non-Latin value is a UTF-16BE text string.
    let e = form.set_value(&doc, &form.fields[0], "\u{3042}").unwrap();
    assert_eq!(e[0].1.as_dict().unwrap().get(b"V"), Some(&Object::String(vec![0xFE, 0xFF, 0x30, 0x42])));
    // Check box: /V and /AS, the widget's own states (no new object).
    let e = form.set_value(&doc, &form.fields[1], "Yes").unwrap();
    assert_eq!(e.len(), 1);
    let d = e[0].1.as_dict().unwrap();
    assert_eq!((d.get(b"V"), d.get(b"AS")), (Some(&Object::Name(b"Yes".to_vec())), Some(&Object::Name(b"Yes".to_vec()))));
    // A state the widget lacks turns it off.
    let e = form.set_value(&doc, &form.fields[1], "Maybe").unwrap();
    assert_eq!(e[0].1.as_dict().unwrap().get(b"AS"), Some(&Object::Name(b"Off".to_vec())));
    assert_eq!(form.set_value(&doc, &form.fields[2], "x"), Err(Error::Unsupported("signature field")));
}
