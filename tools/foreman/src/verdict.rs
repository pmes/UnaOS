//! `verdict` — the ONE place the witness-spec grammar is understood.
//!
//! Parses a `.spec` (`REQUIRE` / `COUNT` / `OPTIONAL` / `FORBID` / `PENDING` /
//! `COMPLETE`, plus the always-on default `FORBID` set), evaluates it over
//! sanitized capture lines, and produces a **structured** result. Rendering the
//! battery-style table is a separate function over that result (design §5.1) —
//! the interface the rest of the design depends on is the structured result,
//! not the printed table.
//!
//! `unaos/scripts/mbench.py` remains the bench's tool and its semantics are the
//! reference. This module is a re-implementation that must agree with it
//! directive-for-directive; `tests/agreement.rs` asserts that on the checked-in
//! spec corpus, and the bench compares the two on the same log.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;

/// The always-on default FORBID set (mbench's `DEFAULT_FORBIDS`).
pub const DEFAULT_FORBIDS: [&str; 3] = [r"-> FAIL", r"FAIL ::", r"PANIC"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Complete,
    Require,
    Count,
    Pending,
    Optional,
    Forbid,
}

impl Kind {
    /// mbench's `KIND_ORDER` — the table's row order.
    fn order(self) -> u8 {
        match self {
            Kind::Complete => 0,
            Kind::Require => 1,
            Kind::Count => 2,
            Kind::Pending => 3,
            Kind::Optional => 4,
            Kind::Forbid => 5,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Complete => "COMPLETE",
            Kind::Require => "REQUIRE",
            Kind::Count => "COUNT",
            Kind::Pending => "PENDING",
            Kind::Optional => "OPTIONAL",
            Kind::Forbid => "FORBID",
        }
    }
}

pub mod glyph {
    pub const OK: &str = "✅";
    pub const FAIL: &str = "❌";
    pub const PENDING: &str = "⏳";
    pub const INFO: &str = "◦";
    pub const CUT: &str = "✂️";
}

/// Exit codes. 0/1/2 are historical; 3 is the truncation verdict.
pub const RC_PASS: i32 = 0;
pub const RC_FAIL: i32 = 1;
pub const RC_ERROR: i32 = 2;
pub const RC_TRUNCATED: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Truncated,
}

impl Verdict {
    pub fn rc(self) -> i32 {
        match self {
            Verdict::Pass => RC_PASS,
            Verdict::Fail => RC_FAIL,
            Verdict::Truncated => RC_TRUNCATED,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Truncated => "TRUNCATED",
        }
    }
}

#[derive(Debug)]
pub struct SpecError(pub String);

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SpecError {}

/// One parsed directive plus the state it accumulated over a capture.
#[derive(Debug, Clone)]
pub struct Directive {
    pub kind: Kind,
    /// Regex source, exactly as written in the spec.
    pub pattern: String,
    /// Threshold (COUNT); 1 otherwise.
    pub need: usize,
    /// A default FORBID, not from the spec file.
    pub builtin: bool,
    pub spec_line: usize,
    rx: Regex,
    pub hits: usize,
    pub first_lineno: Option<usize>,
    pub first_text: Option<String>,
    /// Every matching line number, capped — the `context` module reads these to
    /// window the capture around FORBID hits without re-scanning.
    pub hit_linenos: Vec<usize>,
}

const HIT_LINENO_CAP: usize = 256;

impl Directive {
    fn new(kind: Kind, pattern: &str, need: usize, builtin: bool, spec_line: usize) -> Result<Self, SpecError> {
        let rx = Regex::new(pattern)
            .map_err(|e| SpecError(format!("line {spec_line}: bad regex {pattern:?}: {e}")))?;
        Ok(Directive {
            kind,
            pattern: pattern.to_string(),
            need,
            builtin,
            spec_line,
            rx,
            hits: 0,
            first_lineno: None,
            first_text: None,
            hit_linenos: Vec::new(),
        })
    }

    fn feed(&mut self, text: &str, lineno: usize) -> bool {
        if !self.rx.is_match(text) {
            return false;
        }
        self.hits += 1;
        if self.hit_linenos.len() < HIT_LINENO_CAP {
            self.hit_linenos.push(lineno);
        }
        if self.first_lineno.is_none() {
            self.first_lineno = Some(lineno);
            self.first_text = Some(text.trim().to_string());
        }
        true
    }

    /// For REQUIRE/COUNT: threshold met. Others never gate completion.
    /// COMPLETE deliberately answers `true` so it can never be counted into the
    /// `got/len(req)` witness tally.
    pub fn satisfied(&self) -> bool {
        match self.kind {
            Kind::Require => self.hits >= 1,
            Kind::Count => self.hits >= self.need,
            _ => true,
        }
    }

    /// COMPLETE never fails a run BY ITSELF — an absent marker is the TRUNCATED
    /// verdict (rule 2 of `Evaluation::verdict`).
    pub fn failed(&self) -> bool {
        match self.kind {
            Kind::Require | Kind::Count => !self.satisfied(),
            Kind::Forbid => self.hits > 0,
            _ => false,
        }
    }

    pub fn label(&self) -> String {
        let mut k = if self.kind == Kind::Count {
            format!("COUNT>={}", self.need)
        } else {
            self.kind.name().to_string()
        };
        if self.builtin {
            k.push('*');
        }
        k
    }

    pub fn glyph(&self) -> &'static str {
        if self.failed() {
            return glyph::FAIL;
        }
        match self.kind {
            Kind::Complete => if self.hits > 0 { glyph::OK } else { glyph::CUT },
            Kind::Pending => if self.hits > 0 { glyph::OK } else { glyph::PENDING },
            Kind::Optional => if self.hits > 0 { glyph::OK } else { glyph::INFO },
            _ => glyph::OK,
        }
    }

    pub fn note(&self) -> String {
        let first = self.first_lineno.unwrap_or(0);
        match self.kind {
            Kind::Complete => {
                if self.hits > 0 {
                    format!("end-of-run marker SEEN @ line {first} — the run got past this point")
                } else {
                    "NOT SEEN — the capture never reached this point".to_string()
                }
            }
            Kind::Forbid => {
                if self.hits > 0 {
                    format!(
                        "{} hit(s), first @ line {first}: {}",
                        self.hits,
                        self.first_text.as_deref().unwrap_or("")
                    )
                } else {
                    "0 hits".to_string()
                }
            }
            _ => {
                if self.hits == 0 {
                    return match self.kind {
                        Kind::Pending => "0 hits — awaiting metal/code (never fails)".to_string(),
                        Kind::Optional => "0 hits (informational)".to_string(),
                        _ => "0 hits — MISSING".to_string(),
                    };
                }
                let mut n = format!("{} hit(s), first @ line {first}", self.hits);
                if self.kind == Kind::Count && !self.satisfied() {
                    n.push_str(&format!(" — SHORT of {}", self.need));
                }
                if self.kind == Kind::Pending {
                    n.push_str(" — MATCHED: consider promoting to REQUIRE");
                }
                n
            }
        }
    }
}

/// One spec line after grammar parsing, BEFORE its pattern is compiled. The
/// preflight (`preflight_spec`) walks these so it can compile every pattern and
/// collect ALL the failures instead of hard-stopping on the first one.
#[derive(Debug, Clone)]
struct ScanLine {
    kind: Kind,
    pattern: String,
    need: usize,
    spec_line: usize,
    /// The spec line as written (trimmed) — what the preflight report shows.
    text: String,
}

/// Parse a `.spec` file and append the default FORBID set (mbench's `parse_spec`).
pub fn parse_spec(path: &Path) -> Result<Vec<Directive>, SpecError> {
    let bytes = std::fs::read(path).map_err(|e| SpecError(format!("{}: {e}", path.display())))?;
    parse_spec_bytes(&bytes)
}

pub fn parse_spec_bytes(bytes: &[u8]) -> Result<Vec<Directive>, SpecError> {
    let mut directives = Vec::new();
    for s in scan_spec_bytes(bytes)? {
        directives.push(Directive::new(s.kind, &s.pattern, s.need, false, s.spec_line)?);
    }
    for p in DEFAULT_FORBIDS {
        directives.push(Directive::new(Kind::Forbid, p, 1, true, 0)?);
    }
    Ok(directives)
}

/// The grammar half of `parse_spec_bytes`: directive kinds, COUNT thresholds and
/// line numbers, with no regex compiled yet.
fn scan_spec_bytes(bytes: &[u8]) -> Result<Vec<ScanLine>, SpecError> {
    let text = String::from_utf8_lossy(bytes);
    let mut scanned = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let lineno = i + 1;
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (head, rest) = match line.split_once(char::is_whitespace) {
            Some((h, r)) => (h, r.trim_start()),
            None => (line, ""),
        };
        let kind = head.to_uppercase();
        match kind.as_str() {
            "COUNT" => {
                let sub = rest.split_once(char::is_whitespace);
                let bad = || SpecError(format!("line {lineno}: COUNT wants '<n> <regex>': {line:?}"));
                let (n, pat) = sub.ok_or_else(bad)?;
                let pat = pat.trim_start();
                if pat.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) || n.is_empty() {
                    return Err(bad());
                }
                let need: usize = n.parse().map_err(|_| bad())?;
                scanned.push(ScanLine {
                    kind: Kind::Count,
                    pattern: pat.to_string(),
                    need,
                    spec_line: lineno,
                    text: line.to_string(),
                });
            }
            "REQUIRE" | "OPTIONAL" | "FORBID" | "PENDING" | "COMPLETE" => {
                if rest.is_empty() {
                    return Err(SpecError(format!("line {lineno}: {kind} wants a regex: {line:?}")));
                }
                let k = match kind.as_str() {
                    "REQUIRE" => Kind::Require,
                    "OPTIONAL" => Kind::Optional,
                    "FORBID" => Kind::Forbid,
                    "PENDING" => Kind::Pending,
                    _ => Kind::Complete,
                };
                scanned.push(ScanLine {
                    kind: k,
                    pattern: rest.to_string(),
                    need: 1,
                    spec_line: lineno,
                    text: line.to_string(),
                });
            }
            other => {
                return Err(SpecError(format!("line {lineno}: unknown directive {other:?}")));
            }
        }
    }
    Ok(scanned)
}

/// One directive whose pattern the regex crate refused.
#[derive(Debug, Clone)]
pub struct PatternFault {
    pub spec_line: usize,
    /// The spec line as written.
    pub text: String,
    /// The compiler's complaint, flattened to a single line.
    pub error: String,
}

/// Every pattern fault found in one spec — the preflight's whole report.
#[derive(Debug)]
pub struct PreflightReport {
    pub spec_path: PathBuf,
    pub faults: Vec<PatternFault>,
}

impl std::fmt::Display for PreflightReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.faults.len();
        writeln!(
            f,
            "spec preflight FAILED — {n} directive(s) in {} did not compile; nothing was evaluated.",
            self.spec_path.display()
        )?;
        for fault in &self.faults {
            writeln!(
                f,
                "  {}:{}: {} — {}",
                self.spec_path.display(),
                fault.spec_line,
                fault.text,
                fault.error
            )?;
        }
        write!(
            f,
            "  foreman evaluates specs with the Rust regex crate dialect, which refuses \
             look-around ((?=…) (?!…) (?<=…) (?<!…)) and backreferences BY DESIGN. \
             For a full-PCRE spec use the bench's mbench: \
             `python3 unaos/scripts/mbench.py --replay <LOG> --spec {}`.",
            self.spec_path.display()
        )
    }
}

impl std::error::Error for PreflightReport {}

/// Compile every directive's pattern BEFORE any evaluation starts, collecting
/// all the failures rather than hard-stopping on the first (the regex crate's
/// own error names no spec line). Silent on success: a valid spec's output is
/// byte-identical to what it was before the preflight existed.
///
/// A GRAMMAR error is deliberately not reported here — `parse_spec` still owns
/// those messages, unchanged.
pub fn preflight_spec(path: &Path) -> Result<(), PreflightReport> {
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(()); // `parse_spec` reports an unreadable spec, as before.
    };
    preflight_spec_bytes(path, &bytes)
}

pub fn preflight_spec_bytes(path: &Path, bytes: &[u8]) -> Result<(), PreflightReport> {
    let Ok(scanned) = scan_spec_bytes(bytes) else {
        return Ok(()); // grammar error: `parse_spec` reports it, unchanged.
    };
    let faults: Vec<PatternFault> = scanned
        .iter()
        .filter_map(|s| {
            Regex::new(&s.pattern).err().map(|e| PatternFault {
                spec_line: s.spec_line,
                text: s.text.clone(),
                error: flatten(&e.to_string()),
            })
        })
        .collect();
    if faults.is_empty() {
        return Ok(());
    }
    Err(PreflightReport {
        spec_path: path.to_path_buf(),
        faults,
    })
}

/// The regex crate's errors are multi-line with an ASCII caret diagram; the
/// report wants one line per offending directive.
fn flatten(msg: &str) -> String {
    msg.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.chars().all(|c| c == '^'))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The structured verdict result — the artifact the rest of the loop consumes.
#[derive(Debug)]
pub struct Evaluation {
    pub directives: Vec<Directive>,
    pub spec_path: PathBuf,
    pub log_path: PathBuf,
    pub lines_scanned: usize,
    pub last_lineno: usize,
    pub last_text: String,
    pub unterminated: bool,
}

impl Evaluation {
    pub fn markers(&self) -> Vec<&Directive> {
        self.directives.iter().filter(|d| d.kind == Kind::Complete).collect()
    }

    /// True when the spec declares end-of-run markers and this capture does not
    /// show the run reaching one. A spec with no COMPLETE directive can never be
    /// truncated (mbench's old behaviour, byte-identical).
    pub fn truncated(&self) -> bool {
        let ms = self.markers();
        if ms.is_empty() {
            return false;
        }
        self.unterminated || !ms.iter().any(|d| d.hits > 0)
    }

    /// THE single verdict authority. Precedence:
    ///   1. any FORBID hit                -> FAIL       positive evidence of a fault
    ///   2. else, capture is TRUNCATED    -> TRUNCATED  INCONCLUSIVE
    ///   3. else, any REQUIRE/COUNT short -> FAIL       a real regression
    ///   4. else                          -> PASS
    pub fn verdict(&self) -> Verdict {
        let forbidden = self
            .directives
            .iter()
            .any(|d| d.kind == Kind::Forbid && d.hits > 0);
        let short = self
            .directives
            .iter()
            .any(|d| matches!(d.kind, Kind::Require | Kind::Count) && d.failed());
        if forbidden {
            return Verdict::Fail;
        }
        if self.truncated() {
            return Verdict::Truncated;
        }
        if short {
            return Verdict::Fail;
        }
        Verdict::Pass
    }

    /// Directives in table order (mbench's `KIND_ORDER`, then spec line).
    pub fn sorted(&self) -> Vec<&Directive> {
        let mut ds: Vec<&Directive> = self.directives.iter().collect();
        ds.sort_by_key(|d| (d.kind.order(), d.spec_line));
        ds
    }

    pub fn required(&self) -> Vec<&Directive> {
        self.directives
            .iter()
            .filter(|d| matches!(d.kind, Kind::Require | Kind::Count))
            .collect()
    }

    /// The line number of the LAST-landed REQUIRE — where the run got to.
    pub fn last_landed_require(&self) -> Option<usize> {
        self.directives
            .iter()
            .filter(|d| d.kind == Kind::Require)
            .filter_map(|d| d.first_lineno)
            .max()
    }

    /// Every FORBID hit position, in capture order.
    pub fn forbid_hits(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .directives
            .iter()
            .filter(|d| d.kind == Kind::Forbid)
            .flat_map(|d| d.hit_linenos.iter().copied())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Compact per-directive facts, for cross-checking against another
    /// implementation (the mbench agreement corpus).
    pub fn fingerprint(&self) -> HashMap<String, (usize, Option<usize>)> {
        let mut m = HashMap::new();
        for d in &self.directives {
            m.insert(
                format!("{}|{}|{}|{}", d.kind.name(), d.need, d.builtin, d.pattern),
                (d.hits, d.first_lineno),
            );
        }
        m
    }
}

/// Evaluate a parsed spec over a sanitized capture.
pub fn evaluate(
    mut directives: Vec<Directive>,
    capture: &crate::capture::Capture,
    spec_path: &Path,
) -> Evaluation {
    let mut last_lineno = 0usize;
    let mut last_text = String::new();
    for line in &capture.lines {
        if !line.text.trim().is_empty() {
            last_lineno = line.lineno;
            last_text = line.text.trim().to_string();
        }
        for d in directives.iter_mut() {
            d.feed(&line.text, line.lineno);
        }
    }
    Evaluation {
        directives,
        spec_path: spec_path.to_path_buf(),
        log_path: capture.path.clone(),
        lines_scanned: capture.lines.len(),
        last_lineno,
        last_text,
        unterminated: capture.unterminated,
    }
}

fn basename(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Render the battery-style verdict table — mbench's `verdict_table`, line for
/// line, so a reader (and a diff) can put the two side by side.
pub fn render_table(ev: &Evaluation) -> String {
    let mut out = String::new();
    let verdict = ev.verdict();
    let name = basename(&ev.spec_path);
    out.push_str(&format!(
        "════════════ MBENCH VERDICT — {name} vs {} ════════════\n",
        ev.log_path.display()
    ));
    for d in ev.sorted() {
        out.push_str(&format!("  {} {:<10} {}\n", d.glyph(), d.label(), d.pattern));
        out.push_str(&format!("       {}\n", d.note()));
    }
    let req = ev.required();
    let got = req.iter().filter(|d| d.satisfied()).count();
    let forb: usize = ev
        .directives
        .iter()
        .filter(|d| d.kind == Kind::Forbid)
        .map(|d| d.hits)
        .sum();
    let pend: Vec<&Directive> = ev.directives.iter().filter(|d| d.kind == Kind::Pending).collect();
    let pmatched = pend.iter().filter(|d| d.hits > 0).count();
    out.push_str("  ─────\n");
    let mut summary = format!(
        "{got}/{} required witnesses, {forb} forbidden hit(s), {} lines scanned",
        req.len(),
        ev.lines_scanned
    );
    if !pend.is_empty() {
        summary.push_str(&format!(", pending {pmatched}/{} matched", pend.len()));
    }
    match verdict {
        Verdict::Pass => out.push_str(&format!("  {} MBENCH PASS — {summary}\n", glyph::OK)),
        Verdict::Truncated => {
            out.push_str(&format!(
                "  {} MBENCH TRUNCATED (INCONCLUSIVE) — {summary}\n",
                glyph::CUT
            ));
            let markers = ev.markers();
            if !markers.iter().any(|d| d.hits > 0) {
                let pats: Vec<String> = markers.iter().map(|d| format!("/{}/", d.pattern)).collect();
                out.push_str(&format!(
                    "       end-of-run marker NOT SEEN: {}\n",
                    pats.join(" | ")
                ));
            }
            if ev.unterminated {
                out.push_str(
                    "       capture ends MID-LINE (no terminating newline) — the writer was killed in the middle of a write\n",
                );
            }
            let tail: String = ev.last_text.chars().take(140).collect();
            out.push_str(&format!("       log stops at line {}: {tail}\n", ev.last_lineno));
            out.push_str(&format!(
                "       => NOT a pass and NOT a regression. The boot was cut short, so the {} missing witness(es) never got a chance to print.\n",
                req.len() - got
            ));
            out.push_str(
                "       => Re-run with a window long enough to finish the boot (pi4: `./arroyo kernel8-test 60`) BEFORE reading any of this as a regression.\n",
            );
        }
        Verdict::Fail => {
            out.push_str(&format!("  {} MBENCH FAIL — {summary}\n", glyph::FAIL));
            if !ev.markers().is_empty() {
                out.push_str(
                    "       (the end-of-run marker was seen — the run completed, so a missing witness here is a GENUINE regression)\n",
                );
            }
        }
    }
    out.push_str("  (* = default FORBID, always on)\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture;
    use std::path::Path;

    // mbench's canned self-test fixtures, byte-for-byte.
    const CANNED: &[u8] = b"\x1b[2J\x1b[HUEFI firmware noise \x00\x01 garbage\r\n\
:: CAPSTONE Semaphore: PASS ::\r\n\
:: \x1b[32mU4: process model \xe2\x80\x94 reaped -> PASS\x1b[0m ::\r\n\
:: U5: capabilities -> PASS ::\r\n\
half a line, no newline yet";
    const CANNED_TAIL: &[u8] = b" \xe2\x80\xa6 now finished\r\n:: CAPSTONE COMPLETE \xe2\x80\x94 all 6 verified ::\r\n";
    const CANNED_BAD: &[u8] = b":: U9: write-back -> FAIL (sector mismatch) ::\r\n";

    const SELFTEST_SPEC: &str = "# mbench self-test spec\n\
REQUIRE CAPSTONE COMPLETE\n\
COUNT 2 -> PASS\n\
OPTIONAL Semaphore: PASS\n\
PENDING NEVER-FLASHED-WITNESS\n";

    const TRUNC_SPEC: &str = "# mbench self-test spec — end-of-run marker declared\n\
COMPLETE RUN-END marker\n\
REQUIRE FIRST-WITNESS PASS\n\
REQUIRE LAST-WITNESS PASS\n";
    const TRUNC_HEAD: &[u8] = b":: FIRST-WITNESS PASS ::\r\n";
    const TRUNC_TAIL: &[u8] = b":: LAST-WITNESS PASS ::\r\n:: RUN-END marker ::\r\n";

    fn run(spec: &str, data: &[u8]) -> Evaluation {
        let cap = capture::from_bytes(Path::new("test.log"), data);
        let ds = parse_spec_bytes(spec.as_bytes()).expect("spec parses");
        evaluate(ds, &cap, Path::new("test.spec"))
    }

    fn cat(a: &[u8], b: &[u8]) -> Vec<u8> {
        let mut v = a.to_vec();
        v.extend_from_slice(b);
        v
    }

    #[test]
    fn replay_control_byte_log_passes_spec() {
        let ev = run(SELFTEST_SPEC, &cat(CANNED, CANNED_TAIL));
        assert_eq!(ev.verdict(), Verdict::Pass, "{}", render_table(&ev));
    }

    #[test]
    fn default_forbid_fails_the_run() {
        let ev = run(SELFTEST_SPEC, &cat(&cat(CANNED, CANNED_TAIL), CANNED_BAD));
        assert_eq!(ev.verdict(), Verdict::Fail);
    }

    #[test]
    fn missing_require_fails_the_run() {
        let ev = run(SELFTEST_SPEC, CANNED);
        assert_eq!(ev.verdict(), Verdict::Fail);
    }

    #[test]
    fn truncation_complete_log_passes() {
        let ev = run(TRUNC_SPEC, &cat(TRUNC_HEAD, TRUNC_TAIL));
        assert_eq!(ev.verdict(), Verdict::Pass, "{}", render_table(&ev));
    }

    #[test]
    fn truncation_cut_before_marker_is_truncated() {
        let ev = run(TRUNC_SPEC, TRUNC_HEAD);
        assert_eq!(ev.verdict(), Verdict::Truncated);
        assert_eq!(ev.verdict().rc(), RC_TRUNCATED);
    }

    #[test]
    fn truncation_complete_log_missing_witness_still_fails() {
        let ev = run(TRUNC_SPEC, &cat(TRUNC_HEAD, b":: RUN-END marker ::\r\n"));
        assert_eq!(ev.verdict(), Verdict::Fail);
    }

    #[test]
    fn truncation_midline_capture_is_truncated() {
        let ev = run(TRUNC_SPEC, &cat(&cat(TRUNC_HEAD, TRUNC_TAIL), b":: half a li"));
        assert_eq!(ev.verdict(), Verdict::Truncated);
    }

    #[test]
    fn forbid_outranks_truncation() {
        let ev = run(TRUNC_SPEC, &cat(TRUNC_HEAD, b"PANIC: something exploded\r\n"));
        assert_eq!(ev.verdict(), Verdict::Fail);
    }

    #[test]
    fn spec_without_complete_keeps_plain_fail() {
        let ev = run(SELFTEST_SPEC, CANNED);
        assert_eq!(ev.verdict().rc(), RC_FAIL);
    }

    #[test]
    fn count_short_is_reported() {
        let ev = run("COUNT 9 -> PASS\n", &cat(CANNED, CANNED_TAIL));
        let d = ev.directives.iter().find(|d| d.kind == Kind::Count).unwrap();
        assert!(d.note().contains("SHORT of 9"));
        assert_eq!(d.label(), "COUNT>=9");
    }

    #[test]
    fn pending_match_is_flagged_for_promotion() {
        let ev = run("PENDING Semaphore: PASS\n", &cat(CANNED, CANNED_TAIL));
        let d = ev.directives.iter().find(|d| d.kind == Kind::Pending).unwrap();
        assert!(d.note().contains("consider promoting to REQUIRE"));
        assert_eq!(d.glyph(), glyph::OK);
    }

    #[test]
    fn every_checked_in_spec_parses() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../unaos/scripts/specs");
        if !dir.is_dir() {
            return; // spec corpus not present in this checkout
        }
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).expect("specs dir readable") {
            let p = entry.expect("dir entry").path();
            if p.extension().and_then(|s| s.to_str()) != Some("spec") {
                continue;
            }
            parse_spec(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            seen += 1;
        }
        assert!(seen > 0, "no specs found under {}", dir.display());
    }

    #[test]
    fn preflight_is_silent_on_a_valid_spec() {
        assert!(preflight_spec_bytes(Path::new("test.spec"), SELFTEST_SPEC.as_bytes()).is_ok());
        assert!(preflight_spec_bytes(Path::new("test.spec"), TRUNC_SPEC.as_bytes()).is_ok());
        // A grammar error stays parse_spec's to report, unchanged.
        assert!(preflight_spec_bytes(Path::new("test.spec"), b"MAYBE something\n").is_ok());
    }

    #[test]
    fn preflight_names_the_look_ahead_line() {
        const SPEC: &str = "# look-around is not the regex crate's dialect\n\
REQUIRE CAPSTONE COMPLETE\n\
REQUIRE ^(?=.*ready).*init done\n\
OPTIONAL Semaphore: PASS\n";
        let err = preflight_spec_bytes(Path::new("look.spec"), SPEC.as_bytes())
            .expect_err("look-ahead must be refused");
        assert_eq!(err.faults.len(), 1);
        let f = &err.faults[0];
        assert_eq!(f.spec_line, 3);
        assert_eq!(f.text, "REQUIRE ^(?=.*ready).*init done");
        assert!(!f.error.contains('\n'), "error must be one line: {:?}", f.error);

        let report = err.to_string();
        assert!(report.contains("look.spec:3: REQUIRE ^(?=.*ready).*init done — "), "{report}");
        assert!(report.contains("nothing was evaluated"), "{report}");
        assert!(report.contains("look-around"), "{report}");
        assert!(report.contains("mbench.py"), "{report}");
        // The valid lines are NOT reported.
        assert!(!report.contains("Semaphore"), "{report}");
    }

    #[test]
    fn preflight_collects_every_offender() {
        const SPEC: &str = "REQUIRE (?<=boot )ready\n\
REQUIRE fine\n\
COUNT 2 (?!never)done\n";
        let err = preflight_spec_bytes(Path::new("many.spec"), SPEC.as_bytes())
            .expect_err("both look-arounds must be refused");
        let lines: Vec<usize> = err.faults.iter().map(|f| f.spec_line).collect();
        assert_eq!(lines, vec![1, 3]);
    }

    #[test]
    fn unknown_directive_is_a_spec_error() {
        assert!(parse_spec_bytes(b"MAYBE something\n").is_err());
        assert!(parse_spec_bytes(b"COUNT notanumber x\n").is_err());
        assert!(parse_spec_bytes(b"REQUIRE\n").is_err());
    }
}
