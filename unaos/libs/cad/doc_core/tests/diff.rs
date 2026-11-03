//! VUGDOCDIFF (LEDGER SR501): the semantic diff of two design documents names what changed in design terms — a
//! dimension's value, an added constraint, a moved Op, a removed entity with each dependant — and stays empty for a
//! reordered-keys hand edit, a last-digit write difference within the Tol, and a reordered sketch.

use doc_core::{Diff, Doc, Entry, Field, HEADER_ID, diff, read};

/// The SR480 fixture, taken from tests/doc.rs (ONE fixture, not a copy): two sketches, extrude e1, revolve e2, the
/// machine with its axes, tools, stock and fixture, the Ops dr1 and ct1.
fn doc_a() -> String {
    let src = include_str!("doc.rs");
    let open = "const DOC_A: &str = r#\"";
    let st = src.find(open).expect("DOC_A in tests/doc.rs") + open.len();
    let len = src[st..].find("\"#;").expect("DOC_A closes");
    src[st..st + len].to_string()
}

fn doc(text: &str) -> Doc {
    read(text).unwrap_or_else(|r| panic!("read: {r:?}\n{text}"))
}

/// The fixture with one exact substring replaced (asserted present once, so a fixture edit cannot silently void a test).
fn edit(text: &str, from: &str, to: &str) -> String {
    assert_eq!(text.matches(from).count(), 1, "the edit site {from:?} once in the fixture");
    text.replacen(from, to, 1)
}

fn d(old: &str, new: &str) -> Diff {
    diff(&doc(old), &doc(new))
}

fn field(key: &str, old: &str, new: &str) -> Field {
    Field { key: key.into(), old: Some(old.into()), new: Some(new.into()) }
}

#[test]
fn the_fixture_against_itself_is_empty() {
    let a = doc_a();
    let r = d(&a, &a);
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(r.text(), "");
}

#[test]
fn a_dimension_change_is_one_changed_with_the_key_and_both_values() {
    let a = doc_a();
    let b = edit(&a, "distance d1 sketch=s1 a=p1 b=p2 d=10\n", "distance d1 sketch=s1 a=p1 b=p2 d=17\n");
    let r = d(&a, &b);
    assert_eq!(
        r.entries,
        [Entry::Changed { id: "d1".into(), verb: "distance".into(), fields: vec![field("d", "10", "17")] }]
    );
    assert_eq!(r.text(), "~ distance d1 d: 10 -> 17\n");
    // The other way round names the same field with the values swapped.
    assert_eq!(d(&b, &a).text(), "~ distance d1 d: 17 -> 10\n");
}

#[test]
fn an_added_constraint_is_one_added() {
    let a = doc_a();
    let b = edit(
        &a,
        "distance d2 sketch=s1 a=p2 b=p3 d=8\n",
        "distance d2 sketch=s1 a=p2 b=p3 d=8\ndistance d3 sketch=s1 a=p3 b=p4 d=10\n",
    );
    let r = d(&a, &b);
    assert_eq!(r.entries, [Entry::Added { id: "d3".into(), text: "distance d3 sketch=s1 a=p3 b=p4 d=10".into() }]);
    assert_eq!(r.text(), "+ distance d3 sketch=s1 a=p3 b=p4 d=10\n");
}

#[test]
fn a_reordered_keys_hand_edit_is_empty() {
    let a = doc_a();
    let b = edit(&a, "point p2 sketch=s1 x=10 y=0\n", "point p2 sketch=s1 y=0 x=10   # moved the keys\n");
    let b = edit(&b, "line l1 sketch=s1 a=p1 b=p2\n", "line l1 b=p2 sketch=s1 a=p1\n");
    assert_ne!(a, b, "the text differs");
    let r = d(&a, &b);
    assert!(r.is_empty(), "{r:?}");
}

#[test]
fn a_last_digit_within_the_tol_is_empty_and_beyond_it_is_a_change() {
    let a = doc_a();
    let b = edit(&a, "d=10\n", "d=10.0000000001\n");
    let b = edit(&b, "angle=6.283185307179586\n", "angle=6.283185307179587\n");
    let r = d(&a, &b);
    assert!(r.is_empty(), "within lin and ang: {r:?}");
    // The go-red: past lin (1e-6) the same key is a change.
    let c = edit(&a, "d=10\n", "d=10.00001\n");
    assert_eq!(d(&a, &c).text(), "~ distance d1 d: 10 -> 10.00001\n");
}

#[test]
fn an_op_moved_is_one_moved() {
    let a = doc_a();
    let lines: Vec<&str> = a.lines().collect();
    let dr = lines.iter().position(|l| l.starts_with("drill dr1 ")).unwrap();
    let ct = lines.iter().position(|l| l.starts_with("contour ct1 ")).unwrap();
    let mut swapped: Vec<&str> = lines.clone();
    swapped.swap(dr, ct);
    let b = swapped.join("\n") + "\n";
    let r = d(&a, &b);
    assert_eq!(r.entries, [Entry::Moved { id: "ct1".into(), verb: "contour".into(), from: 4, to: 3 }]);
    assert_eq!(r.text(), "> contour ct1 moved 4 -> 3\n");
}

#[test]
fn a_removed_entity_is_removed_with_each_dependant_its_own_entry() {
    let a = doc_a();
    let mut b = a.clone();
    for l in [
        "point p4 sketch=s1 x=0 y=8\n",
        "line l3 sketch=s1 a=p3 b=p4\n",
        "line l4 sketch=s1 a=p4 b=p1\n",
        "horizontal h2 sketch=s1 line=l3\n",
        "vertical v2 sketch=s1 line=l4\n",
    ] {
        b = edit(&b, l, "");
    }
    let r = d(&a, &b);
    let ids: Vec<&str> = r
        .entries
        .iter()
        .map(|e| match e {
            Entry::Removed { id, .. } => id.as_str(),
            other => panic!("only Removed expected: {other:?}"),
        })
        .collect();
    assert_eq!(ids, ["p4", "l3", "l4", "h2", "v2"], "in the old document's order");
    assert!(r.text().starts_with("- point p4 sketch=s1 x=0 y=8\n- line l3 sketch=s1 a=p3 b=p4\n"));
    assert_eq!(r.text().lines().count(), 5);
}

#[test]
fn a_reordered_sketch_is_empty() {
    let a = doc_a();
    let b = edit(
        &a,
        "point p1 sketch=s1 x=0 y=0\npoint p2 sketch=s1 x=10 y=0\n",
        "point p2 sketch=s1 x=10 y=0\npoint p1 sketch=s1 x=0 y=0\n",
    );
    let r = d(&a, &b);
    assert!(r.is_empty(), "sketch entity order is not replay order: {r:?}");
}

#[test]
fn the_header_tol_and_an_id_less_tool_are_named() {
    let a = doc_a();
    let b = edit(&a, "lin=1e-6 ", "lin=1e-5 ");
    let r = d(&a, &b);
    assert_eq!(
        r.entries,
        [Entry::Changed { id: HEADER_ID.into(), verb: "vug-doc".into(), fields: vec![field("lin", "1e-6", "0.00001")] }]
    );
    assert_eq!(r.text(), "~ vug-doc vug-doc lin: 1e-6 -> 0.00001\n");
    let c = edit(&a, "tool number=1 kind=flat_end dc=6 ", "tool number=1 kind=flat_end dc=8 ");
    assert_eq!(d(&a, &c).text(), "~ tool tool:1 dc: 6 -> 8\n");
}

// VUGBREPEDGESNAMED (LEDGER SR524): fillet / chamfer are ordered statements, so a reordered edit pair is a Moved.
#[test]
fn a_reordered_fillet_pair_is_one_moved() {
    let a = doc_a();
    // fl1 / fl2: doc_a already declares `fix f1`.
    let f1 = "fillet fl1 on=e1 edges=[[extrude sweep 0 0]] r=1\n";
    let f2 = "fillet fl2 on=e1 edges=[[extrude sweep 0 1]] r=1\n";
    let old = format!("{a}{f1}{f2}");
    let new = format!("{a}{f2}{f1}");
    let r = d(&old, &new);
    // doc_a's ordered statements are e1 e2 dr1 ct1 (1..4); the edits are 5 and 6.
    assert_eq!(r.entries, [Entry::Moved { id: "fl2".into(), verb: "fillet".into(), from: 6, to: 5 }]);
    assert_eq!(r.text(), "> fillet fl2 moved 6 -> 5\n");
}
