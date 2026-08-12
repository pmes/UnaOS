use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScrollAnchor {
    Top,
    Bottom,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScrollBehavior {
    AutoScroll,
    Manual,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StreamAlign {
    Start,
    End,
    Center,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamTetra {
    pub input_anchor: ScrollAnchor,
    pub scroll_behavior: ScrollBehavior,
    pub alignment: StreamAlign,
}

impl Default for StreamTetra {
    fn default() -> Self {
        Self {
            input_anchor: ScrollAnchor::Bottom,
            scroll_behavior: ScrollBehavior::AutoScroll,
            alignment: StreamAlign::Start,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TextAction {
    OpenDocument, // Maps text to SMessage::OpenDocument { url }
    ConsoleInput, // Maps text to SMessage::ConsoleInput(text)
    BrowserText,  // Maps text to SMessage::BrowserText(text)
}

/// One rendered Console line: the record's severity/source tags plus its
/// display-safe text. `text` has already been through Tabula's log sanitizer,
/// so a stray control byte is shown as its Unicode Control Picture (never
/// obeyed) and escape sequences are stripped — the same read-only,
/// control-byte-safe treatment the editor gives a named log file.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsoleLine {
    pub level: String,
    pub source: String,
    pub text: String,
}

/// The Console app's render tetra: the sanitized scrollback snapshot plus the
/// honest since-boot eviction count, the scroll-lock flag, and the active text
/// filter. Read-only by construction — the node carries no input field, because
/// the system log is not the view's to write.
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ConsoleTetra {
    pub lines: Vec<ConsoleLine>,
    pub dropped: u64,
    pub paused: bool,
    pub filter: String,
}

impl ConsoleTetra {
    /// Bridge `comscan`'s `LogViewState` into a renderable tetra, running every
    /// record's content through Tabula's log sanitizer. This is the arm the Qt/
    /// GTK bridge deferred: the Console pane is now a first-class `ViewEntity`,
    /// so `WorkspaceTetra::from_state` converts it here rather than collapsing
    /// it to `Empty`.
    pub fn from_log_view(lv: &bandy::state::LogViewState) -> Self {
        let lines = lv
            .lines
            .iter()
            .map(|l| ConsoleLine {
                level: l.level.clone(),
                source: l.source.clone(),
                text: sanitize_line(&l.content),
            })
            .collect();
        Self {
            lines,
            dropped: lv.dropped,
            paused: lv.paused,
            filter: lv.filter.clone(),
        }
    }
}

/// Sanitize one log record's content for display through Tabula's sanitizer,
/// with the single trailing newline it may leave trimmed (a scrollback row is
/// one line). Control bytes become Control Pictures, escape sequences are
/// stripped, and invalid UTF-8 is decoded lossily — a corrupt byte off the
/// cable can never wreck the pane.
pub fn sanitize_line(content: &str) -> String {
    let mut s = tabula::logview::sanitize(content.as_bytes()).text;
    if s.ends_with('\n') {
        s.pop();
    }
    s
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TetraNode {
    Matrix, // Future MatrixTetra (Sidebar)
    Stream(StreamTetra), // Structuring Comms
    Console(ConsoleTetra), // Read-only live system-log view (Console.app equivalent)
    Empty,  // Placeholder

    // Layout
    VStack(Vec<TetraNode>),
    HStack(Vec<TetraNode>),

    // Controls
    Button {
        id: String,
        label: String,
        action: bandy::SMessage,
    },
    TextField {
        id: String,
        placeholder: String,
        action: TextAction,
    },
    Surface {
        id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceTetra {
    pub left_pane: TetraNode,
    pub right_pane: TetraNode,
    pub split_ratio: f32,
}

impl Default for WorkspaceTetra {
    fn default() -> Self {
        Self {
            left_pane: TetraNode::Matrix,
            right_pane: TetraNode::Stream(StreamTetra::default()),
            split_ratio: 0.25,
        }
    }
}

impl WorkspaceTetra {
    pub fn from_state(state: &bandy::state::WorkspaceState) -> Self {
        let left_pane = match &state.left_pane {
            bandy::state::ViewEntity::Topology(_) => TetraNode::Matrix,
            bandy::state::ViewEntity::Stream(s) => TetraNode::Stream(StreamTetra {
                input_anchor: match s.input_anchor {
                    bandy::state::ScrollAnchor::Top => ScrollAnchor::Top,
                    bandy::state::ScrollAnchor::Bottom => ScrollAnchor::Bottom,
                },
                scroll_behavior: match s.scroll_behavior {
                    bandy::state::ScrollBehavior::AutoScroll => ScrollBehavior::AutoScroll,
                    bandy::state::ScrollBehavior::Manual => ScrollBehavior::Manual,
                },
                alignment: match s.alignment {
                    bandy::state::StreamAlign::Start => StreamAlign::Start,
                    bandy::state::StreamAlign::End => StreamAlign::End,
                    bandy::state::StreamAlign::Center => StreamAlign::Center,
                },
            }),
            // The Qt tetra bridge has no editor node yet; a code pane collapses
            // to Empty here (the macOS AppKit backend renders it directly).
            bandy::state::ViewEntity::Editor(_) => TetraNode::Empty,
            // The deferred arm, now lifted: the Console pane renders read-only,
            // control-byte-safe, through Tabula's sanitizer.
            bandy::state::ViewEntity::Console(lv) => {
                TetraNode::Console(ConsoleTetra::from_log_view(lv))
            }
            bandy::state::ViewEntity::Empty => TetraNode::Empty,
        };

        let right_pane = match &state.right_pane {
            bandy::state::ViewEntity::Topology(_) => TetraNode::Matrix,
            bandy::state::ViewEntity::Stream(s) => TetraNode::Stream(StreamTetra {
                input_anchor: match s.input_anchor {
                    bandy::state::ScrollAnchor::Top => ScrollAnchor::Top,
                    bandy::state::ScrollAnchor::Bottom => ScrollAnchor::Bottom,
                },
                scroll_behavior: match s.scroll_behavior {
                    bandy::state::ScrollBehavior::AutoScroll => ScrollBehavior::AutoScroll,
                    bandy::state::ScrollBehavior::Manual => ScrollBehavior::Manual,
                },
                alignment: match s.alignment {
                    bandy::state::StreamAlign::Start => StreamAlign::Start,
                    bandy::state::StreamAlign::End => StreamAlign::End,
                    bandy::state::StreamAlign::Center => StreamAlign::Center,
                },
            }),
            // The Qt tetra bridge has no editor node yet; a code pane collapses
            // to Empty here (the macOS AppKit backend renders it directly).
            bandy::state::ViewEntity::Editor(_) => TetraNode::Empty,
            // The deferred arm, now lifted: the Console pane renders read-only,
            // control-byte-safe, through Tabula's sanitizer.
            bandy::state::ViewEntity::Console(lv) => {
                TetraNode::Console(ConsoleTetra::from_log_view(lv))
            }
            bandy::state::ViewEntity::Empty => TetraNode::Empty,
        };

        Self {
            left_pane,
            right_pane,
            split_ratio: state.split_ratio,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bandy::state::{LogLine, LogSource, LogViewState, ViewEntity, WorkspaceState};

    fn log_view(lines: Vec<LogLine>, dropped: u64, paused: bool) -> LogViewState {
        LogViewState {
            lines,
            filter: "gpu".to_string(),
            source: LogSource::All,
            paused,
            dropped,
        }
    }

    /// The lifted arm: a `ViewEntity::Console` becomes a `TetraNode::Console`
    /// (not the old `Empty` collapse), and the bounded-ring honesty — the
    /// eviction count and the scroll-lock — rides across the bridge intact.
    #[test]
    fn console_view_entity_bridges_to_a_console_node() {
        let lv = log_view(
            vec![LogLine {
                seq: 1,
                level: "info".into(),
                source: "kernel".into(),
                content: "link up".into(),
            }],
            7,
            true,
        );
        let ws = WorkspaceState {
            left_pane: ViewEntity::Console(lv),
            right_pane: ViewEntity::Empty,
            split_ratio: 0.25,
            bottom_pane: None,
        };
        let tetra = WorkspaceTetra::from_state(&ws);
        match tetra.left_pane {
            TetraNode::Console(c) => {
                assert_eq!(c.dropped, 7, "eviction count lost across the bridge");
                assert!(c.paused, "scroll-lock lost across the bridge");
                assert_eq!(c.filter, "gpu");
                assert_eq!(c.lines.len(), 1);
                assert_eq!(c.lines[0].source, "kernel");
                assert_eq!(c.lines[0].text, "link up");
            }
            other => panic!("Console pane did not bridge to a Console node: {other:?}"),
        }
    }

    /// A control byte off the cable is SHOWN, never obeyed: the bridge runs each
    /// line through Tabula's sanitizer, so a raw NUL/BEL is rendered as its
    /// Control Picture and an ANSI escape is stripped. This is the read-only,
    /// control-byte-safe honesty that keeps a mangled record from wrecking the
    /// pane (the same reason logs are inspected with `awk`, not `grep`).
    #[test]
    fn console_lines_are_sanitized_through_tabula() {
        let lv = log_view(
            vec![LogLine {
                seq: 1,
                level: "warn".into(),
                source: "serial".into(),
                content: "a\x00b\x1b[31mc".into(),
            }],
            0,
            false,
        );
        let ws = WorkspaceState {
            left_pane: ViewEntity::Empty,
            right_pane: ViewEntity::Console(lv),
            split_ratio: 0.25,
            bottom_pane: None,
        };
        let tetra = WorkspaceTetra::from_state(&ws);
        let TetraNode::Console(c) = tetra.right_pane else {
            panic!("expected a Console node on the right pane");
        };
        let text = &c.lines[0].text;
        // The NUL is visible as its Control Picture, not a raw NUL…
        assert!(text.contains('\u{2400}'), "NUL was not shown: {text:?}");
        assert!(!text.contains('\u{0}'), "raw NUL survived: {text:?}");
        // …the ANSI escape is gone, and the letters around it remain.
        assert!(!text.contains('\u{1b}'), "escape survived: {text:?}");
        assert_eq!(text, "a\u{2400}bc");
    }

    /// A trailing newline in a record is trimmed — a scrollback row is one line,
    /// so `sanitize_line` never leaves a dangling break to double-space the view.
    #[test]
    fn sanitize_line_trims_a_single_trailing_newline() {
        assert_eq!(sanitize_line("done\n"), "done");
        assert_eq!(sanitize_line("done"), "done");
    }
}
