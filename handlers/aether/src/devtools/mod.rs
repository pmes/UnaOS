//! CHARTER: Aether (docs/CODEX.md §2, Web) — seam: handler (host-native Ring 3, handlers/aether), the
//! browser's developer tools; a reader of the engine's own state, never a second store of it.
//!
//! AETHERDEVTOOLS (LEDGER SR112): the developer-tools pane. Every panel READS what the engine already
//! holds; nothing here adds state to the engine, and the one thing the pane keeps of its own (the
//! evaluator's transcript, the selected node, the frame samples it timed) lives in [`DevTools`], which the
//! face owns beside the engine. Each panel names its seam:
//!
//! | panel        | module         | seam (what it reads)                                                        |
//! |--------------|----------------|-----------------------------------------------------------------------------|
//! | Elements     | [`elements`]   | `AetherEngine::document` (the html_core arena), `css::computed_report` over `AetherEngine::layout_tree` (the values layout and paint consume), `AetherEngine::box_of`, `AetherEngine::hit_test_at` for the picker |
//! | Console      | [`console`]    | `js::console_lines` (PageState.console, fed by `js::host::console_out`, which every `console.*` and every uncaught error reaches), `js::reported_errors`, the evaluator through `js::Engine::eval_string` in the PAGE's realm, `ledger::snapshot().entries()` for the engine's honest-failure lines |
//! | Network      | [`network`]    | the document's resource references resolved against `AetherEngine::current_url`, each looked up in `net::http_cache()` under the page's partition (`cache_key(site_of(top), url)`, the key gneiss_pal stores under — SR88), and `security::refusals()` (SR86) for the blocked ones |
//! | Storage      | [`storage`]    | `net::cookie_jar()`, `storage::with_area` (localStorage + sessionStorage under `storage::page_storage_key`), `storage::idb::databases` + `usage`, `storage::cache::cache_names` + `request_keys` + `usage` (SR94) |
//! | Accessibility | [`accessibility`] | `a11y::tree` (THE accessibility read API: roles, accname 1.2 names, states, hidden subtrees pruned) and `a11y::focus::order` for the Tab sequence; the selected node is the Elements panel's (AETHERA11Y, SR119) |
//! | Sources      | [`sources`]    | the document's `<script>` elements (`AetherEngine::document`), each external script's source through `js::ran_source` (what the engine ran or pre-fetched under its URL), each inline script's text; a Console frame `url:line:col` is a target ([`DevTools::goto`], AETHERDEVTOOLS3 SR128) |
//! | Performance  | [`timeline`]   | `css::last_style_timing` (the cascade and re-layout wall times `apply_stylesheets` already measures) plus the pane's own clock around `AetherEngine::tick` (script: timers, microtasks, rAF) and `AetherEngine::render_frame` (paint) |
//!
//! The pane is toggled by F12 / Ctrl+Shift+I and docked right or bottom by the chrome model
//! ([`crate::chrome::DevtoolsChrome`]); a face paints [`DevTools::render`]'s lines (or the panel structs
//! directly) in its pane. Everything here runs on the engine's thread: the page state it reads is
//! thread-local there.

pub mod accessibility;
pub mod console;
pub mod elements;
pub mod network;
pub mod sources;
pub mod storage;
pub mod timeline;

use crate::AetherEngine;

/// The pane's panels, in the tab-strip order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Panel {
    #[default]
    Elements,
    Console,
    /// AETHERDEVTOOLS3 (SR128): the page's scripts and the source of the selected one.
    Sources,
    Network,
    Storage,
    Performance,
    /// AETHERA11Y (SR119): the accessibility tree and the selected node's role, name and states.
    Accessibility,
}

impl Panel {
    pub const ALL: [Panel; 7] = [Panel::Elements, Panel::Console, Panel::Sources, Panel::Network, Panel::Storage, Panel::Performance, Panel::Accessibility];

    pub fn label(self) -> &'static str {
        match self {
            Panel::Elements => "Elements",
            Panel::Console => "Console",
            Panel::Sources => "Sources",
            Panel::Network => "Network",
            Panel::Storage => "Storage",
            Panel::Performance => "Performance",
            Panel::Accessibility => "Accessibility",
        }
    }
}

/// The pane's own state (the face keeps one per tab, beside that tab's engine).
#[derive(Debug, Default)]
pub struct DevTools {
    /// The panel showing.
    pub panel: Panel,
    /// The Elements panel's selected node (an arena id of the current document).
    pub selected: Option<usize>,
    /// The Console panel's evaluator transcript and input history.
    pub console: console::Console,
    /// The Performance panel's frame samples.
    pub timeline: timeline::Timeline,
    /// The Sources panel's selected script and current line.
    pub sources: sources::Sources,
}

impl DevTools {
    pub fn new() -> Self {
        Self::default()
    }

    /// Selects the element under viewport (`x`, `y`) (the inspect picker) and shows Elements.
    pub fn pick(&mut self, engine: &AetherEngine, x: f64, y: f64) -> Option<usize> {
        let id = elements::pick(engine, x, y)?;
        self.selected = Some(id);
        self.panel = Panel::Elements;
        Some(id)
    }

    /// Follows a frame: shows Sources with the script a frame naming `url` is in and `line` current (the
    /// Console's `    at fn (url:line:col)` lines are targets). False, and nothing changes, when the document
    /// has no script of that name.
    pub fn goto(&mut self, engine: &AetherEngine, url: &str, line: u32) -> bool {
        if !self.sources.goto(engine, url, line) {
            return false;
        }
        self.panel = Panel::Sources;
        true
    }

    /// Follows the frame a pane text line names ([`sources::frame_target`]); false for any other line.
    pub fn goto_line(&mut self, engine: &AetherEngine, text: &str) -> bool {
        match sources::frame_target(text) {
            Some((url, line, _)) => self.goto(engine, &url, line),
            None => false,
        }
    }

    /// One evaluator line in the page's realm (the Console's input field).
    pub fn evaluate(&mut self, engine: &mut AetherEngine, line: &str) -> console::Entry {
        self.console.evaluate(engine, line)
    }

    /// The showing panel as plain text lines, for a face that paints the pane as a text view.
    pub fn render(&self, engine: &AetherEngine) -> Vec<String> {
        let mut out = vec![Panel::ALL
            .iter()
            .map(|p| if *p == self.panel { format!("[{}]", p.label()) } else { p.label().to_string() })
            .collect::<Vec<_>>()
            .join("  ")];
        match self.panel {
            Panel::Elements => {
                for row in elements::tree(engine, elements::MAX_ROWS) {
                    let mark = if Some(row.id) == self.selected { '>' } else { ' ' };
                    out.push(format!("{mark}{}{}", "  ".repeat(row.depth), row.label));
                }
                if let Some(ins) = self.selected.and_then(|id| elements::inspect(engine, id)) {
                    out.push(String::new());
                    out.push(format!("<{}>  #{}", ins.tag, ins.id));
                    if let Some((x, y, w, h)) = ins.border_box {
                        out.push(format!("box {x:.1},{y:.1}  {w:.1} x {h:.1}"));
                    }
                    if let Some(bm) = self.selected.and_then(|id| elements::box_model(engine, id)) {
                        out.extend(elements::box_model_lines(&bm));
                    }
                    for (k, v) in &ins.attributes {
                        out.push(format!("@{k} = \"{v}\""));
                    }
                    for (k, v) in &ins.computed {
                        out.push(format!("{k}: {v};"));
                    }
                }
            }
            Panel::Console => {
                for e in self.console.entries() {
                    out.push(format!("{} {}", e.level.glyph(), e.text));
                }
                let issues = console::issues();
                if !issues.is_empty() {
                    out.push(String::new());
                    out.push(format!("Issues ({})", issues.len()));
                    for i in issues {
                        out.push(format!("  {} {} x{}", i.category, i.name, i.count));
                    }
                }
            }
            Panel::Network => {
                out.push(format!("{:<7} {:<6} {:<10} {:<24} {:>9} {:>7}  {}", "method", "status", "type", "mime", "size", "ms", "url"));
                for r in network::requests(engine) {
                    out.push(r.row());
                    if r.has_detail() {
                        out.extend(r.detail_lines());
                    }
                }
            }
            Panel::Sources => out.extend(self.sources.lines(engine)),
            Panel::Storage => {
                let v = storage::view(engine);
                out.extend(v.lines());
            }
            Panel::Performance => {
                out.push(format!("{:>6} {:>9} {:>9} {:>9} {:>9}", "frame", "script", "style", "layout", "paint"));
                for s in self.timeline.samples() {
                    out.push(format!(
                        "{:>6} {:>9.2} {:>9.2} {:>9.2} {:>9.2}",
                        s.frame, s.script_ms, s.style_ms, s.layout_ms, s.paint_ms
                    ));
                }
                let t = self.timeline.totals();
                out.push(format!("{:>6} {:>9.2} {:>9.2} {:>9.2} {:>9.2}", "total", t.script_ms, t.style_ms, t.layout_ms, t.paint_ms));
            }
            Panel::Accessibility => out.extend(accessibility::lines(engine, self.selected)),
        }
        out
    }
}
