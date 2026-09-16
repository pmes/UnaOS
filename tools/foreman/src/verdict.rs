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
///
/// A COPY, and it must stay one — element for element AND in order. `tests/agreement.rs` asserts
/// that this module and `mbench.py` render byte-identical verdict tables over a shared corpus, and
/// the table lists directives positionally, so a set that merely AGREES is not enough. Any edit to
/// `unaos/scripts/mbench.py`'s `DEFAULT_FORBIDS` is an edit here in the same commit.
///
/// CURSOREMIT added `-> FLICKER` (the `[cursor11]` compose-through verdict for a panel present
/// published with a live arrow off the glass; `flicker_frames` is contractually 0). `-> BRACKETED`
/// from the same line is deliberately absent — it is a legitimate state under a present storm. The
/// argument in full is at mbench.py's own declaration; it is not restated here, because two copies
/// of a rationale is how two divergent ones happen.
pub const DEFAULT_FORBIDS: [&str; 4] = [r"-> FAIL", r"FAIL ::", r"PANIC", r"-> FLICKER"];

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
    /// Basename of the spec file this directive was written in (mbench's
    /// `spec_name`). EMPTY for the builtin FORBIDs — they have no spec line, so
    /// there is nowhere to send a reader, and `origin()` prints nothing.
    pub spec_name: String,
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
    fn new(
        kind: Kind,
        pattern: &str,
        need: usize,
        builtin: bool,
        spec_line: usize,
        spec_name: &str,
    ) -> Result<Self, SpecError> {
        let rx = Regex::new(pattern)
            .map_err(|e| SpecError(format!("line {spec_line}: bad regex {pattern:?}: {e}")))?;
        Ok(Directive {
            kind,
            pattern: pattern.to_string(),
            need,
            builtin,
            spec_line,
            spec_name: spec_name.to_string(),
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

    /// ` (pinned at x86-fat.spec:14)` — printed ONLY on a directive that came up
    /// short (mbench's `Directive.origin`).
    ///
    /// A green table is byte-identical to one printed before this existed: the
    /// coordinate is for the reader who has to go and change something, and that
    /// reader only appears on a red. Empty for the builtin FORBIDs.
    pub fn origin(&self) -> String {
        if self.spec_name.is_empty() || self.spec_line == 0 {
            return String::new();
        }
        format!(" (pinned at {}:{})", self.spec_name, self.spec_line)
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
                        _ => format!("0 hits — MISSING{}", self.origin()),
                    };
                }
                let mut n = format!("{} hit(s), first @ line {first}", self.hits);
                if self.kind == Kind::Count && !self.satisfied() {
                    n.push_str(&format!(" — SHORT of {}{}", self.need, self.origin()));
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
    parse_spec_bytes_named(&basename(path), &bytes)
}

/// Bytes with no file behind them: every directive is unpinned, exactly as
/// mbench's builtin FORBIDs are, so `origin()` prints nothing for them.
pub fn parse_spec_bytes(bytes: &[u8]) -> Result<Vec<Directive>, SpecError> {
    parse_spec_bytes_named("", bytes)
}

/// `parse_spec_bytes` with the spec's BASENAME — mbench pins its rows with
/// `os.path.basename(path)`, never the full path, so the two agree on a table
/// rendered from different working directories.
pub fn parse_spec_bytes_named(spec_name: &str, bytes: &[u8]) -> Result<Vec<Directive>, SpecError> {
    let mut directives = Vec::new();
    for s in scan_spec_bytes(bytes)? {
        directives.push(Directive::new(s.kind, &s.pattern, s.need, false, s.spec_line, spec_name)?);
    }
    for p in DEFAULT_FORBIDS {
        directives.push(Directive::new(Kind::Forbid, p, 1, true, 0, "")?);
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

// ---------------------------------------------------------------------------
// QEMU-FAST run sidecar — was this capture a FAST exit or a FULL wall?
// ---------------------------------------------------------------------------
//
// mbench's `read_run_sidecar` / `run_mode_note`, re-implemented here so the two
// tables can agree on the verdict line. `unaos/arroyo`'s `qemu_wait_or_complete`
// can end a QEMU run at its completion predicate plus a grace window instead of
// at the clock, which makes a capture SHORTER than the verb's nominal wall while
// still carrying every required witness. Sound for pass/fail, NOT sound for
// anything monotonic — so a reader has to be able to tell the two apart from the
// capture alone. arroyo writes `<logfile>.run` BESIDE the log, never into it.
//
// THE READ IS THREE-VALUED and that is the whole point: "fast", "full", and
// UNKNOWN — absent, unreadable, malformed, or STALE. Collapsing the third value
// into either of the first two is the bug this guards against; a reader that
// infers "not fast, therefore full" is confidently wrong in the unsafe
// direction. Staleness is DETECTED, not assumed away: the sidecar carries the
// log's byte length and sha256, so a sidecar left by a previous run of the same
// verb — which sits at exactly the same path — is caught rather than believed.
pub const RUN_SIDECAR_SUFFIX: &str = ".run";

fn sidecar_path(log_path: &Path) -> PathBuf {
    let mut s = log_path.as_os_str().to_os_string();
    s.push(RUN_SIDECAR_SUFFIX);
    PathBuf::from(s)
}

/// Read `<log_path>.run`. Returns `(mode, detail, fields)`, where `mode` is one
/// of `"fast"`, `"full"`, `"unknown"` — never anything else, and never guessed.
fn read_run_sidecar(log_path: &Path) -> (&'static str, String, HashMap<String, String>) {
    let empty = HashMap::new();
    let Ok(raw) = std::fs::read(sidecar_path(log_path)) else {
        return ("unknown", "no run sidecar".to_string(), empty);
    };
    let raw = String::from_utf8_lossy(&raw);

    let mut fields: HashMap<String, String> = HashMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        fields.insert(k.trim().to_string(), v.trim().to_string());
    }

    let mode = match fields.get("mode").map(String::as_str) {
        Some("fast") => "fast",
        Some("full") => "full",
        _ => {
            return (
                "unknown",
                "run sidecar is malformed (no usable mode=)".to_string(),
                fields,
            );
        }
    };

    // IDENTITY. Mandatory, not optional.
    let want_bytes = fields.get("log_bytes").cloned().unwrap_or_default();
    let want_sha = fields.get("log_sha256").cloned().unwrap_or_default();
    if want_bytes.is_empty() || want_sha.is_empty() {
        return (
            "unknown",
            "run sidecar carries no log identity".to_string(),
            fields,
        );
    }
    let Ok(md) = std::fs::metadata(log_path) else {
        return (
            "unknown",
            "run sidecar present but the log could not be identified".to_string(),
            fields,
        );
    };
    let got_bytes = md.len();
    let Ok(got_sha) = sha256::hex_of_file(log_path) else {
        return (
            "unknown",
            "run sidecar present but the log could not be identified".to_string(),
            fields,
        );
    };
    if got_bytes.to_string() != want_bytes || got_sha != want_sha {
        return (
            "unknown",
            format!(
                "run sidecar is STALE — it describes a {want_bytes}-byte log, this one is \
                 {got_bytes} bytes"
            ),
            fields,
        );
    }
    (mode, String::new(), fields)
}

/// The one-line `[…]` suffix the verdict line carries (mbench's
/// `run_mode_note`). ONE implementation, so no consumer can invent a second
/// reading of the same file.
pub fn run_mode_note(log_path: &Path) -> String {
    let (mode, detail, f) = read_run_sidecar(log_path);
    let get = |k: &str| f.get(k).cloned().unwrap_or_else(|| "?".to_string());
    match mode {
        "fast" => format!(
            "[fast: completion +{}s grace {}s wall {}s]",
            get("completion_at"),
            get("grace"),
            get("wall")
        ),
        "full" => format!("[full wall {}s]", get("wall")),
        _ => format!("[mode unknown: {detail}]"),
    }
}

/// SHA-256 over a file, streamed. Hand-rolled rather than pulled in as a
/// dependency: the sidecar's identity check is the ONLY hash this tool needs,
/// and `mbench.py` gets it from the standard library — a re-implementation that
/// must agree byte-for-byte should not also have to agree on a crate version.
/// Known-answer tested below against the FIPS 180-4 vectors.
mod sha256 {
    use std::io::Read;
    use std::path::Path;

    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    pub struct Sha256 {
        h: [u32; 8],
        buf: [u8; 64],
        buflen: usize,
        total: u64,
    }

    impl Sha256 {
        pub fn new() -> Self {
            Sha256 {
                h: [
                    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c,
                    0x1f83d9ab, 0x5be0cd19,
                ],
                buf: [0u8; 64],
                buflen: 0,
                total: 0,
            }
        }

        pub fn update(&mut self, mut data: &[u8]) {
            self.total = self.total.wrapping_add(data.len() as u64);
            if self.buflen > 0 {
                let want = 64 - self.buflen;
                let take = want.min(data.len());
                self.buf[self.buflen..self.buflen + take].copy_from_slice(&data[..take]);
                self.buflen += take;
                data = &data[take..];
                if self.buflen == 64 {
                    let block = self.buf;
                    self.compress(&block);
                    self.buflen = 0;
                }
            }
            while data.len() >= 64 {
                let mut block = [0u8; 64];
                block.copy_from_slice(&data[..64]);
                self.compress(&block);
                data = &data[64..];
            }
            if !data.is_empty() {
                self.buf[..data.len()].copy_from_slice(data);
                self.buflen = data.len();
            }
        }

        pub fn finish(mut self) -> [u8; 32] {
            // The length field is over the MESSAGE, so it is captured before any
            // padding is fed through `update` (which counts what it is given).
            let bitlen = self.total.wrapping_mul(8);
            let mut pad: Vec<u8> = Vec::with_capacity(72);
            pad.push(0x80);
            while (self.buflen + pad.len()) % 64 != 56 {
                pad.push(0x00);
            }
            pad.extend_from_slice(&bitlen.to_be_bytes());
            self.update(&pad);
            let mut out = [0u8; 32];
            for (i, w) in self.h.iter().enumerate() {
                out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
            }
            out
        }

        fn compress(&mut self, block: &[u8; 64]) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([
                    block[i * 4],
                    block[i * 4 + 1],
                    block[i * 4 + 2],
                    block[i * 4 + 3],
                ]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
                self.h[0], self.h[1], self.h[2], self.h[3], self.h[4], self.h[5], self.h[6],
                self.h[7],
            );
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let t1 = h
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let t2 = s0.wrapping_add(maj);
                h = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }
            self.h[0] = self.h[0].wrapping_add(a);
            self.h[1] = self.h[1].wrapping_add(b);
            self.h[2] = self.h[2].wrapping_add(c);
            self.h[3] = self.h[3].wrapping_add(d);
            self.h[4] = self.h[4].wrapping_add(e);
            self.h[5] = self.h[5].wrapping_add(f);
            self.h[6] = self.h[6].wrapping_add(g);
            self.h[7] = self.h[7].wrapping_add(h);
        }
    }

    /// One-shot, for the known-answer tests. The tool itself only ever hashes a
    /// file, and that goes through `hex_of_file`.
    #[cfg(test)]
    pub fn hex(data: &[u8]) -> String {
        let mut s = Sha256::new();
        s.update(data);
        to_hex(&s.finish())
    }

    /// Streamed, 1 MiB at a time — mbench reads the same way, and a bench
    /// capture can be hundreds of megabytes.
    pub fn hex_of_file(path: &Path) -> std::io::Result<String> {
        let mut f = std::fs::File::open(path)?;
        let mut s = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            s.update(&buf[..n]);
        }
        Ok(to_hex(&s.finish()))
    }

    fn to_hex(bytes: &[u8; 32]) -> String {
        let mut out = String::with_capacity(64);
        for b in bytes {
            out.push_str(&format!("{b:02x}"));
        }
        out
    }
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
    // QEMU-FAST: say on the verdict line WHICH KIND OF CAPTURE this verdict is
    // about. Three-valued (see `read_run_sidecar`): a reader who needs a final
    // accumulator value or a certified-clean tail must see `full` here and
    // nothing else — `unknown` is not a quiet synonym for it.
    summary.push(' ');
    summary.push_str(&run_mode_note(&ev.log_path));
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
            // SPECRUN: ONE line a caller can quote without re-implementing the
            // match. `arroyo`'s `test`/`test-fat` tails read exactly this line to
            // put `<spec>:<line>` in their own red, so the verb and the table can
            // never disagree about WHICH pin came up short. Emitted in SPEC ORDER
            // (`sorted()` is kind then spec line), so "first" means first in the file.
            let short: Vec<&Directive> = ev
                .sorted()
                .into_iter()
                .filter(|d| matches!(d.kind, Kind::Require | Kind::Count) && d.failed())
                .collect();
            if let Some(d0) = short.first() {
                out.push_str(&format!(
                    "       FIRST-SHORTFALL {}:{} {} {}\n",
                    d0.spec_name,
                    d0.spec_line,
                    d0.label(),
                    d0.pattern
                ));
                if short.len() > 1 {
                    out.push_str(&format!(
                        "       ({} further pinned line(s) also short — full table above)\n",
                        short.len() - 1
                    ));
                }
            }
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

    /// The hand-rolled hash is the one thing here that is NOT checked by the
    /// mbench corpus — no fixture carries a valid sidecar, so a wrong digest
    /// would read as a STALE sidecar and the two tools would still agree. It
    /// gets its own known answers: FIPS 180-4's two published vectors, the
    /// empty message, and a multi-block input that exercises the buffering
    /// path (`update` called with a length that is not a multiple of 64).
    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(
            sha256::hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256::hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256::hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // 1,000,000 'a' — the third published vector, and 15,625 whole blocks.
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256::hex(&million),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        // Fed in ragged chunks, the streaming path must give the same answer as
        // one shot — this is how `hex_of_file` reads a capture.
        let mut s = sha256::Sha256::new();
        for chunk in million.chunks(7) {
            s.update(chunk);
        }
        let mut hex = String::with_capacity(64);
        for b in s.finish() {
            hex.push_str(&format!("{b:02x}"));
        }
        assert_eq!(hex, sha256::hex(&million));
    }

    /// The third value is not a quiet synonym for `full`: an absent sidecar,
    /// a malformed one, one with no identity, and a STALE one must each be
    /// sayable — and none of them may render as `[full …]`.
    #[test]
    fn run_mode_is_three_valued() {
        let dir = std::env::temp_dir().join(format!("foreman-runmode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let log = dir.join("serial.log");
        std::fs::write(&log, b"hello\n").expect("log writable");
        let side = dir.join("serial.log.run");

        assert_eq!(run_mode_note(&log), "[mode unknown: no run sidecar]");

        std::fs::write(&side, b"wall=300\n").expect("sidecar writable");
        assert_eq!(
            run_mode_note(&log),
            "[mode unknown: run sidecar is malformed (no usable mode=)]"
        );

        std::fs::write(&side, b"mode=full\nwall=300\n").expect("sidecar writable");
        assert_eq!(
            run_mode_note(&log),
            "[mode unknown: run sidecar carries no log identity]"
        );

        let sha = sha256::hex(b"hello\n");
        std::fs::write(
            &side,
            format!("mode=full\nwall=300\nlog_bytes=6\nlog_sha256={sha}\n"),
        )
        .expect("sidecar writable");
        assert_eq!(run_mode_note(&log), "[full wall 300s]");

        std::fs::write(
            &side,
            format!("mode=fast\ncompletion_at=11.0\ngrace=20\nwall=31.0\nlog_bytes=6\nlog_sha256={sha}\n"),
        )
        .expect("sidecar writable");
        assert_eq!(run_mode_note(&log), "[fast: completion +11.0s grace 20s wall 31.0s]");

        // STALE: the log grew under a sidecar that still describes the old one.
        std::fs::write(&log, b"hello\nagain\n").expect("log writable");
        assert!(
            run_mode_note(&log).starts_with("[mode unknown: run sidecar is STALE"),
            "{}",
            run_mode_note(&log)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
