// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BANDY-1 golden-frame capture — see Cargo.toml. Serializes the reply-subset `SMessage`
// variants through the REAL host serializer and prints one `NAME<TAB>JSON` line per sample.
// The sample set deliberately covers every byte class the kernel escaper must reproduce:
// plain ASCII, `"` and `\`, the named short escapes (\n \r \t \b \f), an arbitrary control
// byte (\u00xx path), the empty string, and DEL (0x7f — NOT escaped by serde_json).
//
// Usage: cargo run -p bandy-golden > tools/bandy-golden/golden-frames.txt

use bandy::signals::SMessage;

fn emit(name: &str, msg: &SMessage) {
    let json = serde_json::to_string(msg).expect("host serializer must succeed");
    println!("{name}\t{json}");
}

fn main() {
    // ls-style listing (the shape bus ls replies carry)
    emit(
        "ls_listing",
        &SMessage::TerminalOutput("HELLO.BIN 1024\nK2OWN.BIN 512\n".to_string()),
    );
    // every escape class the kernel emitter can produce
    emit(
        "escapes",
        &SMessage::TerminalOutput(
            "line1\nline2\ttab\r\"quoted\" back\\slash\u{8}\u{c}".to_string(),
        ),
    );
    // an arbitrary control byte -> serde_json's \u00xx (lowercase hex) path
    emit(
        "control_byte",
        &SMessage::TerminalOutput("ctl:\u{1}\u{1f}end".to_string()),
    );
    // DEL (0x7f) is NOT escaped by serde_json — pin that
    emit("del_byte", &SMessage::TerminalOutput("del:\u{7f}end".to_string()));
    // empty payload
    emit("empty", &SMessage::TerminalOutput(String::new()));
    // error replies (denial / errno carriers)
    emit("err_eacces", &SMessage::TerminalError("cat: errno -13".to_string()));
    emit("err_enoent", &SMessage::TerminalError("cp: errno -2".to_string()));
}
