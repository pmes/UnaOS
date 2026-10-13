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
//! | Performance  | [`timeline`]   | `css::last_style_timing` (the cascade and re-layout wall times `apply_stylesheets` already measures) plus the pane's own clock around `AetherEngine::tick` (script: timers, microtasks, rAF) and `AetherEngine::render_frame` (paint) |
//!
//! The pane is toggled by F12 / Ctrl+Shift+I and docked right or bottom by the chrome model
//! ([`crate::chrome::DevtoolsChrome`]); a face paints [`DevTools::render`]'s lines (or the panel structs
//! directly) in its pane. Everything here runs on the engine's thread: the page state it reads is
//! thread-local there.

pub mod console;
pub mod elements;
pub mod network;
pub mod storage;
pub mod timeline;

use crate::AetherEngine;

/// The pane's panels, in the tab-strip order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Panel {
    #[default]
    Elements,
    Console,
    Network,
    Storage,
    Performance,
}

impl Panel {
    pub const ALL: [Panel; 5] = [Panel::Elements, Panel::Console, Panel::Network, Panel::Storage, Panel::Performance];

    pub fn label(self) -> &'static str {
        match self {
            Panel::Elements => "Elements",
            Panel::Console => "Console",
            Panel::Network => "Network",
            Panel::Storage => "Storage",
            Panel::Performance => "Performance",
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
                }
            }
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
        }
        out
    }
}
