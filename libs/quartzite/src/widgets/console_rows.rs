//! CHARTER: Aether (CODEX §2 Web) — the system-log Console's pure rows and text, read from bandy's `LogTail` wire.
//!
//! AETHERMACDEVTOOLS (SR312): the Console app (`TetraNode::Console`, the macOS `Console.app` equivalent) is a
//! read-only, live view of `comscan`'s scrollback; what a face derives from the wire is written ONCE here,
//! platform-neutral (std + bandy + tetra), and both faces read it: `platforms/gtk/console_view.rs` and
//! `platforms/macos/console_view.rs`. Every incoming record goes through Tabula's sanitizer
//! ([`crate::tetra::sanitize_line`]) so a stray control byte is shown, never obeyed ([`rows_from_tail`]). The level
//! filter ([`LevelFilter`]) is a face choice over the rows; the store and the ring stay `comscan`'s.

use bandy::state::LogLine;

use crate::tetra::ConsoleLine;

/// One scrollback row as text: `source  text` (or just `text` when the record carries no source tag). `text` is
/// already sanitized.
pub fn fmt_line(l: &ConsoleLine) -> String {
    if l.source.is_empty() {
        l.text.clone()
    } else {
        format!("{}  {}", l.source, l.text)
    }
}

/// The whole scrollback rendered as one buffer string, oldest→newest.
pub fn render_body(lines: &[ConsoleLine]) -> String {
    lines.iter().map(fmt_line).collect::<Vec<_>>().join("\n")
}

/// The header line: what the pane is, plus the honest bounded-ring state.
pub fn render_header(dropped: u64, paused: bool) -> String {
    let mut h = String::from(":: Console — read-only system log");
    if dropped > 0 {
        h.push_str(&format!(" · {dropped} evicted"));
    }
    if paused {
        h.push_str(" · PAUSED");
    }
    h.push_str(" ::");
    h
}

/// A `LogTail`'s records as the rows a face renders, each record's content sanitized.
pub fn rows_from_tail(lines: &[LogLine]) -> Vec<ConsoleLine> {
    lines
        .iter()
        .map(|l| ConsoleLine { level: l.level.clone(), source: l.source.clone(), text: crate::tetra::sanitize_line(&l.content) })
        .collect()
}

/// The lowest level a face shows: `All` shows every row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LevelFilter {
    #[default]
    All,
    Info,
    Warn,
    Error,
}

impl LevelFilter {
    /// The filter's choices in the order a face lays them out, with their labels.
    pub const ALL: [(LevelFilter, &'static str); 4] =
        [(LevelFilter::All, "All"), (LevelFilter::Info, "Info"), (LevelFilter::Warn, "Warn"), (LevelFilter::Error, "Error")];

    /// Whether a row at `level` shows under this filter.
    pub fn passes(self, level: &str) -> bool {
        let floor = match self {
            LevelFilter::All => return true,
            LevelFilter::Info => 2,
            LevelFilter::Warn => 3,
            LevelFilter::Error => 4,
        };
        level_rank(level) >= floor
    }
}

/// A level's rank, case-insensitive: trace 0, debug 1, info 2, warn 3, error 4; an unknown spelling ranks as info.
pub fn level_rank(level: &str) -> u8 {
    match level.trim().to_ascii_lowercase().as_str() {
        "trace" => 0,
        "debug" => 1,
        "warn" | "warning" => 3,
        "error" | "err" | "fatal" | "critical" => 4,
        _ => 2,
    }
}

/// The rows that show under `filter`, in order.
pub fn filtered(lines: &[ConsoleLine], filter: LevelFilter) -> Vec<ConsoleLine> {
    lines.iter().filter(|l| filter.passes(&l.level)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(level: &str, source: &str, text: &str) -> ConsoleLine {
        ConsoleLine { level: level.into(), source: source.into(), text: text.into() }
    }

    /// The text both faces render: a row with and without its source, the body, the header's ring state, and a
    /// `LogTail` record's control byte shown as a Control Picture.
    #[test]
    fn console_rows_text_kat() {
        assert_eq!(fmt_line(&line("INFO", "net", "up")), "net  up");
        assert_eq!(fmt_line(&line("INFO", "", "up")), "up");
        assert_eq!(render_body(&[line("INFO", "a", "1"), line("WARN", "", "2")]), "a  1\n2");
        assert_eq!(render_header(0, false), ":: Console — read-only system log ::");
        assert_eq!(render_header(3, true), ":: Console — read-only system log · 3 evicted · PAUSED ::");
        let rows = rows_from_tail(&[LogLine { seq: 1, level: "WARN".into(), source: "usb".into(), content: "a\u{7}b\n".into() }]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].level, "WARN");
        assert_eq!(rows[0].source, "usb");
        assert!(!rows[0].text.contains('\u{7}') && !rows[0].text.ends_with('\n'), "{:?}", rows[0].text);
    }

    /// The level filter: All shows every row; Warn keeps warn and error (any case); an unknown level ranks as info.
    #[test]
    fn console_rows_filter_kat() {
        let rows = [line("TRACE", "", "t"), line("debug", "", "d"), line("INFO", "", "i"), line("Warn", "", "w"), line("ERROR", "", "e"), line("odd", "", "o")];
        let texts = |f: LevelFilter| filtered(&rows, f).iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("");
        assert_eq!(texts(LevelFilter::All), "tdiweo");
        assert_eq!(texts(LevelFilter::Info), "iweo");
        assert_eq!(texts(LevelFilter::Warn), "we");
        assert_eq!(texts(LevelFilter::Error), "e");
        assert_eq!(LevelFilter::default(), LevelFilter::All);
        assert_eq!(LevelFilter::ALL.map(|(_, l)| l), ["All", "Info", "Warn", "Error"]);
    }
}
