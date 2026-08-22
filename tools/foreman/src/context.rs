//! `context` — assemble the bounded diagnostic context (design §3.2).
//!
//! Pure: no I/O, no provider. Three parts, each with a hard budget so a runaway
//! log can never blow the request:
//!
//!   * the **verdict table** — the full structured result, never truncated;
//!   * a **tail of the capture** — the last N sanitized lines, plus the lines
//!     surrounding each FORBID hit and the last-landed REQUIRE;
//!   * **expected-vs-observed** — the relevant excerpt of the bench runbook.
//!
//! Budgets are configuration, not constants scattered in code.

use std::collections::BTreeSet;

use crate::capture::Capture;
use crate::verdict::Evaluation;

/// The hard bounds on the assembled context. Every field is a knob the CLI (or,
/// later, the vessel) sets; nothing here is a magic number buried in logic.
#[derive(Debug, Clone, Copy)]
pub struct Budgets {
    /// How many trailing capture lines to include.
    pub tail_lines: usize,
    /// Lines of context on either side of a FORBID hit / last-landed REQUIRE.
    pub window: usize,
    /// Ceiling on the excerpt lines drawn from the runbook.
    pub runbook_lines: usize,
    /// Absolute ceiling on the assembled context, in bytes. The verdict table is
    /// never truncated; the capture excerpt is trimmed from the FRONT (oldest
    /// first) until the whole fits.
    pub max_bytes: usize,
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets { tail_lines: 120, window: 12, runbook_lines: 120, max_bytes: 64 * 1024 }
    }
}

/// The assembled context, in parts, so a caller can render it its own way.
#[derive(Debug, Clone)]
pub struct DiagnosticContext {
    pub verdict_table: String,
    /// Selected capture lines, ascending by line number, with `…` elisions
    /// already resolved into the `elided` flags.
    pub excerpt: Vec<ExcerptLine>,
    pub runbook: Option<String>,
    pub budgets: Budgets,
    pub truncated_for_budget: bool,
}

#[derive(Debug, Clone)]
pub struct ExcerptLine {
    pub lineno: usize,
    pub text: String,
    /// True when a gap precedes this line (non-contiguous with the previous one).
    pub gap_before: bool,
}

/// Assemble the bounded context. Pure over its inputs.
pub fn assemble(
    ev: &Evaluation,
    capture: &Capture,
    runbook: Option<&str>,
    budgets: Budgets,
) -> DiagnosticContext {
    let verdict_table = crate::verdict::render_table(ev);

    // --- pick the lines of interest -------------------------------------------
    let total = capture.lines.len();
    let mut wanted: BTreeSet<usize> = BTreeSet::new();

    // The tail.
    let tail_start = total.saturating_sub(budgets.tail_lines).max(0);
    for n in (tail_start + 1)..=total {
        wanted.insert(n);
    }

    // A window around each FORBID hit — the positive evidence of a fault.
    for hit in ev.forbid_hits() {
        window_into(&mut wanted, hit, budgets.window, total);
    }

    // A window around the last-landed REQUIRE — where the run got to.
    if let Some(last) = ev.last_landed_require() {
        window_into(&mut wanted, last, budgets.window, total);
    }

    let mut excerpt: Vec<ExcerptLine> = Vec::with_capacity(wanted.len());
    let mut prev: Option<usize> = None;
    for n in &wanted {
        let text = capture
            .lines
            .get(n - 1)
            .map(|l| l.text.trim_end().to_string())
            .unwrap_or_default();
        excerpt.push(ExcerptLine {
            lineno: *n,
            text,
            gap_before: prev.is_some_and(|p| p + 1 != *n),
        });
        prev = Some(*n);
    }

    let runbook = runbook.map(|r| {
        let lines: Vec<&str> = r.lines().collect();
        let start = lines.len().saturating_sub(budgets.runbook_lines);
        lines[start..].join("\n")
    });

    let mut ctx = DiagnosticContext {
        verdict_table,
        excerpt,
        runbook,
        budgets,
        truncated_for_budget: false,
    };

    // --- enforce the absolute byte ceiling ------------------------------------
    // The verdict table is the highest-signal artifact and is never truncated;
    // the capture excerpt gives way, oldest lines first.
    while ctx.render().len() > budgets.max_bytes && !ctx.excerpt.is_empty() {
        ctx.excerpt.remove(0);
        if let Some(first) = ctx.excerpt.first_mut() {
            first.gap_before = true;
        }
        ctx.truncated_for_budget = true;
    }
    ctx
}

fn window_into(set: &mut BTreeSet<usize>, at: usize, window: usize, total: usize) {
    let lo = at.saturating_sub(window).max(1);
    let hi = (at + window).min(total);
    for n in lo..=hi {
        set.insert(n);
    }
}

impl DiagnosticContext {
    /// The verbatim text written to the transcript BEFORE any send, and sent as
    /// the request body. What the model saw is reconstructable from this string.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("=== VERDICT TABLE (complete, never truncated) ===\n");
        out.push_str(&self.verdict_table);
        out.push_str("\n=== SANITIZED CAPTURE EXCERPT ===\n");
        out.push_str(&format!(
            "(budgets: tail={} lines, window=±{} lines, max={} bytes)\n",
            self.budgets.tail_lines, self.budgets.window, self.budgets.max_bytes
        ));
        if self.truncated_for_budget {
            out.push_str("(excerpt trimmed from the front to fit the byte budget)\n");
        }
        for line in &self.excerpt {
            if line.gap_before {
                out.push_str("      …\n");
            }
            out.push_str(&format!("{:>6}: {}\n", line.lineno, line.text));
        }
        if let Some(rb) = &self.runbook {
            out.push_str("\n=== EXPECTED-VS-OBSERVED (bench runbook excerpt) ===\n");
            out.push_str(rb);
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capture, verdict};
    use std::path::Path;

    fn fixture(n: usize) -> Vec<u8> {
        let mut s = String::new();
        for i in 1..=n {
            s.push_str(&format!("line {i}\r\n"));
        }
        s.push_str("PANIC: exploded\r\n");
        for i in 1..=20 {
            s.push_str(&format!("after {i}\r\n"));
        }
        s.into_bytes()
    }

    fn eval(data: &[u8], spec: &str) -> (verdict::Evaluation, capture::Capture) {
        let cap = capture::from_bytes(Path::new("t.log"), data);
        let ds = verdict::parse_spec_bytes(spec.as_bytes()).unwrap();
        let ev = verdict::evaluate(ds, &cap, Path::new("t.spec"));
        (ev, cap)
    }

    #[test]
    fn excerpt_windows_the_forbid_hit_even_when_far_from_the_tail() {
        let data = fixture(400);
        let (ev, cap) = eval(&data, "REQUIRE line 3\n");
        let b = Budgets { tail_lines: 5, window: 3, runbook_lines: 10, max_bytes: 1 << 20 };
        let ctx = assemble(&ev, &cap, None, b);
        let text = ctx.render();
        assert!(text.contains("PANIC: exploded"), "{text}");
        // the last-landed REQUIRE window
        assert!(text.contains("line 3"), "{text}");
        // and the tail
        assert!(text.contains("after 20"), "{text}");
    }

    #[test]
    fn byte_budget_never_eats_the_verdict_table() {
        let data = fixture(2000);
        let (ev, cap) = eval(&data, "REQUIRE line 3\n");
        let table_len = verdict::render_table(&ev).len();
        let b = Budgets { tail_lines: 2000, window: 5, runbook_lines: 10, max_bytes: table_len + 400 };
        let ctx = assemble(&ev, &cap, None, b);
        assert!(ctx.truncated_for_budget);
        assert!(ctx.render().contains("MBENCH"));
        assert!(ctx.render().len() <= b.max_bytes || ctx.excerpt.is_empty());
    }

    #[test]
    fn runbook_excerpt_is_bounded() {
        let data = fixture(10);
        let (ev, cap) = eval(&data, "REQUIRE line 3\n");
        let rb: String = (1..=500).map(|i| format!("rb {i}\n")).collect();
        let b = Budgets { tail_lines: 5, window: 2, runbook_lines: 7, max_bytes: 1 << 20 };
        let ctx = assemble(&ev, &cap, Some(&rb), b);
        assert_eq!(ctx.runbook.as_ref().unwrap().lines().count(), 7);
    }
}
