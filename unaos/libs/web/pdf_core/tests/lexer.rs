// SPDX-License-Identifier: LGPL-3.0-or-later
//! PDF1 (SR300): the lexer and the object parser against ISO 32000-1 §7.2-7.3.

use pdf_core::lexer::{Lexer, Token};
use pdf_core::object::{Object, Ref, parse_indirect, parse_object, text_string};

fn all(s: &[u8]) -> Vec<Token<'_>> {
    let mut lx = Lexer::new(s);
    let mut v = Vec::new();
    while let Some(t) = lx.next() {
        v.push(t);
    }
    v
}

#[test]
fn numbers_7_3_3() {
    let t = all(b"123 43445 +17 -98 0 34.5 -3.62 +123.6 4. -.002 0.0 --5");
    assert_eq!(t[..5], [Token::Int(123), Token::Int(43445), Token::Int(17), Token::Int(-98), Token::Int(0)]);
    let reals: Vec<f64> = t[5..10].iter().map(|t| if let Token::Real(r) = t { *r } else { panic!("{t:?}") }).collect();
    for (got, want) in reals.iter().zip([34.5, -3.62, 123.6, 4.0, -0.002]) {
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
    }
    assert_eq!(t[10], Token::Real(0.0));
    assert_eq!(t[11], Token::Int(-5));
    // One correctly rounded division, not a sum of rounded tenths.
    assert_eq!(all(b"595.276 841.89 0.1")[..], [Token::Real(595.276), Token::Real(841.89), Token::Real(0.1)]);
}

#[test]
fn names_7_3_5() {
    let t = all(b"/Name1 /A;Name_With-Various***Characters? /1.2 /$$ /@pattern /.notdef /Lime#20Green /paired#28#29parentheses /The_Key_of_F#23_Minor /");
    let names: Vec<Vec<u8>> = t.into_iter().map(|t| if let Token::Name(n) = t { n } else { panic!() }).collect();
    assert_eq!(names[6], b"Lime Green");
    assert_eq!(names[7], b"paired()parentheses");
    assert_eq!(names[8], b"The_Key_of_F#_Minor");
    assert_eq!(names[1], b"A;Name_With-Various***Characters?");
    assert_eq!(names[9], b"");
}

#[test]
fn literal_strings_7_3_4_2() {
    let t = all(b"(Strings may contain balanced parentheses ( ) and\nspecial characters (*!&}^% and so on).) (\\101\\n\\0053\\(\\)\\\\) (a\\\nb) (x\r\ny) (\\q)");
    assert_eq!(t[0], Token::Str(b"Strings may contain balanced parentheses ( ) and\nspecial characters (*!&}^% and so on).".to_vec()));
    // \0053 is \005 then '3' (at most three octal digits).
    assert_eq!(t[1], Token::Str(b"A\n\x053()\\".to_vec()));
    assert_eq!(t[2], Token::Str(b"ab".to_vec()));
    assert_eq!(t[3], Token::Str(b"x\ny".to_vec()));
    assert_eq!(t[4], Token::Str(b"q".to_vec()));
}

#[test]
fn hex_strings_and_comments() {
    let t = all(b"<4E6F762073686D6F7A206B6120706F702E> % a comment\n<901FA> < 61 62 >");
    assert_eq!(t[0], Token::Str(b"Nov shmoz ka pop.".to_vec()));
    assert_eq!(t[1], Token::Str(vec![0x90, 0x1F, 0xA0]));
    assert_eq!(t[2], Token::Str(b"ab".to_vec()));
    assert_eq!(t.len(), 3);
}

#[test]
fn objects_and_refs() {
    let mut lx = Lexer::new(b"<< /Type /Example /Sub [1 0 R 2 (x) true null] /N -3.5 /D << /K 12 0 R >> >>");
    let o = parse_object(&mut lx, 0).unwrap();
    let d = o.as_dict().unwrap();
    assert_eq!(d.name(b"Type"), Some(&b"Example"[..]));
    let a = d.get(b"Sub").unwrap().as_array().unwrap();
    assert_eq!(a[0], Object::Ref(Ref { num: 1, generation: 0 }));
    assert_eq!(a[1], Object::Int(2));
    assert_eq!(a[3], Object::Bool(true));
    assert_eq!(a[4], Object::Null);
    assert_eq!(d.get(b"N"), Some(&Object::Real(-3.5)));
    assert_eq!(d.get(b"D").unwrap().as_dict().unwrap().get(b"K"), Some(&Object::Ref(Ref { num: 12, generation: 0 })));
}

#[test]
fn depth_is_bounded() {
    let deep = "[".repeat(200);
    let mut lx = Lexer::new(deep.as_bytes());
    assert_eq!(parse_object(&mut lx, 0), Err(pdf_core::Error::Depth));
}

#[test]
fn indirect_stream_with_wrong_length_falls_back_to_endstream() {
    let buf = b"7 0 obj\n<< /Length 999 >>\nstream\r\nHELLO\nendstream\nendobj\n";
    let (r, o) = parse_indirect(buf, 0, &mut |_| None).unwrap();
    assert_eq!(r, Ref { num: 7, generation: 0 });
    assert_eq!(o.as_stream().unwrap().data, b"HELLO");
    let buf = b"8 0 obj << /Length 9 0 R >> stream\nAB\nCD\nendstream endobj";
    let (_, o) = parse_indirect(buf, 0, &mut |r| (r.num == 9).then_some(5)).unwrap();
    assert_eq!(o.as_stream().unwrap().data, b"AB\nCD");
}

#[test]
fn text_strings_7_9_2_2() {
    assert_eq!(text_string(b"\xFE\xFF\x00H\x00i\xD8\x3D\xDE\x00"), "Hi\u{1F600}");
    assert_eq!(text_string(b"Caf\xE9 \x80 \x93 \xA0"), "Caf\u{E9} \u{2022} \u{FB01} \u{20AC}");
    assert_eq!(text_string(b"\xEF\xBB\xBFna\xC3\xAFve"), "na\u{EF}ve");
}
