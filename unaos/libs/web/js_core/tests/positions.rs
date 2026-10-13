//! JSPOS (LEDGER SR127): source positions — the per-function line table, V8-shaped stack frames
//! (`    at fn (url:line:col)`), and the SyntaxError's offending-token position.

use js_core::bytecode::{line_col, LineIndex, LineTable};
use js_core::vm::ops::SourcePosition;
use js_core::vm::{NullHost, Value, Vm};

/// Runs `src` as the script `t.js`; returns the thrown value's `stack` (or panics when nothing threw).
fn thrown(src: &str) -> (Vm, Value) {
    let mut vm = Vm::new(Box::new(NullHost));
    let e = vm.run_script_str_at(src, "t.js").expect_err("the script throws");
    (vm, e)
}

fn stack_of(vm: &mut Vm, e: &Value) -> String {
    let Value::Object(o) = e else { panic!("not an object: {}", vm.error_string(e)) };
    let s = vm.get(*o, &js_core::vm::PropertyKey::from_str("stack")).expect("stack");
    match s {
        Value::String(s) => s.to_rust(),
        _ => panic!("stack is not a string"),
    }
}

fn pos(file: &str, line: u32, col: u32) -> Option<SourcePosition> {
    Some(SourcePosition { file: file.to_string(), line, col })
}

#[test]
fn runtime_throw_inside_a_nested_call_names_its_line_and_column() {
    let src = "function inner() {\n  throw new Error('deep');\n}\nfunction outer() {\n  return inner();\n}\nouter();\n";
    let (vm, e) = thrown(src);
    assert_eq!(vm.error_position(&e), pos("t.js", 2, 9), "the innermost frame is the `new Error` in inner()");
}

#[test]
fn syntax_error_carries_the_offending_tokens_line() {
    let (mut vm, e) = thrown("var a = 1;\nvar b = 2;\nvar c = );\n");
    assert!(vm.error_string(&e).starts_with("SyntaxError"), "{}", vm.error_string(&e));
    assert_eq!(vm.error_position(&e), pos("t.js", 3, 9));
    assert!(stack_of(&mut vm, &e).ends_with("\n    at t.js:3:9"));
}

#[test]
fn stack_with_three_frames_has_the_v8_shape() {
    let src = "function inner() {\n  throw new Error('deep');\n}\nfunction outer() {\n  return inner();\n}\nouter();\n";
    let (mut vm, e) = thrown(src);
    assert_eq!(stack_of(&mut vm, &e), "Error: deep\n    at inner (t.js:2:9)\n    at outer (t.js:5:10)\n    at t.js:7:1");
    assert_eq!(vm.error_stack(&e), "Error: deep\n    at inner (t.js:2:9)\n    at outer (t.js:5:10)\n    at t.js:7:1");
}

#[test]
fn a_multi_line_template_literal_moves_the_lines_below_it() {
    let src = "var t = `one\ntwo\nthree`;\nfunction f() { throw new Error('after template'); }\nf();\n";
    let (mut vm, e) = thrown(src);
    assert_eq!(stack_of(&mut vm, &e), "Error: after template\n    at f (t.js:4:22)\n    at t.js:5:1");
}

#[test]
fn a_line_after_comments_counts_their_lines() {
    let src = "// a line comment\n/* a block\r\n   comment */\nthrow new Error('c');\n";
    let (vm, e) = thrown(src);
    assert_eq!(vm.error_position(&e), pos("t.js", 4, 7));
}

#[test]
fn unnamed_scripts_print_anonymous_and_constructors_print_new() {
    let mut vm = Vm::new(Box::new(NullHost));
    let v = vm.run_script_str("function C() { this.s = new Error('x').stack; }\nnew C().s").expect("runs");
    let Value::String(s) = v else { panic!("not a string") };
    assert_eq!(s.to_rust(), "Error: x\n    at new C (<anonymous>:1:25)\n    at <anonymous>:2:1");
}

#[test]
fn line_table_round_trips_and_looks_up_the_covering_row() {
    let src: Vec<u16> = "a\nbb\r\nccc\u{2028}d".encode_utf16().collect();
    let idx = LineIndex::new(&src);
    assert_eq!(idx.line_col(0), (1, 1));
    assert_eq!(idx.line_col(3), (2, 2));
    assert_eq!(idx.line_col(6), (3, 1));
    assert_eq!(idx.line_col(10), (4, 1));
    assert_eq!(line_col(&src, 8), (3, 3));
    let t = LineTable::build(&[(0, 0), (3, 3), (4, 3), (9, 10), (300, 6)], &idx, None);
    assert_eq!(t.rows(), vec![(0, 1, 1), (3, 2, 2), (9, 4, 1), (300, 3, 1)], "a row equal to the previous is dropped");
    assert_eq!(t.lookup(5), Some((2, 2)));
    assert_eq!(t.lookup(299), Some((4, 1)));
    assert_eq!(t.lookup(1000), Some((3, 1)));
    assert!(t.bytes.len() <= 13, "{} bytes for four rows", t.bytes.len());
}

#[test]
fn a_thrown_non_error_is_placed_at_its_throw_statement() {
    // AETHERDEVTOOLS3 (SR128): `throw "x"` records no trace; `error_position` falls back to `Vm::last_throw`.
    let (vm, e) = thrown("function f() {\n  throw 'x';\n}\nf();\n");
    assert!(matches!(e, Value::String(_)), "the thrown value is the string itself");
    assert_eq!(vm.error_position(&e), pos("t.js", 2, 3), "the `throw` keyword of f()");
    let (vm, e) = thrown("var n = 1;\n    throw { code: n };\n");
    assert_eq!(vm.error_position(&e), pos("t.js", 2, 5), "a thrown plain object, top-level");
}
