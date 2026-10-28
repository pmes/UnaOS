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
        /// LANDINGSECRETFIELD (SR432): a secret field never draws its text: AppKit's NSSecureTextFieldCell, GTK's
        /// GlyphEntry one [`SECRET_BULLET`] per grapheme cluster ([`secret_mask`]), copy refused. Default false: every
        /// existing field and serialized tree unchanged.
        #[serde(default)]
        secret: bool,
    },
    /// NLE2ORDER (SR227): the Surface pixel contract. Every `SurfaceBlit { url == id }` carries RGBA8, straight
    /// (non-premultiplied) alpha, row-major, stride `width * 4`, on every embassy; an embassy whose native buffer
    /// differs converts ONCE at its hand-off (GTK's cairo ARgb32: [`surface_argb32`]), so no caller swizzles.
    /// AETHERMACGPU-DAMAGE (SR320) / -ENGINEDAMAGE (SR341): a blit with `damage: Some(rects)` carries only those rects'
    /// rows (each stride `w * 4`, in list order), which the receiver walks with `damage::spans` and writes into the frame it
    /// holds through [`surface_splice`]; the sender's side is [`SurfaceDamage`].
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

/// NLE2ORDER (SR227): a Surface's RGBA8 (straight alpha, the [`TetraNode::Surface`] contract) as cairo's `ARgb32`:
/// one native-endian 32-bit word per pixel, alpha in the high byte, colour premultiplied (`(c * a + 127) / 255`, the
/// identity at a = 255), so B, G, R, A in memory on a little-endian host. Pure: the GTK embassy's one hand-off
/// converter (`image_view`, `probe_view`), testable without a GTK handle.
pub fn surface_argb32(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        let a = px[3] as u32;
        let pm = |c: u8| ((c as u32 * a + 127) / 255) as u8;
        let word = u32::from_be_bytes([px[3], pm(px[0]), pm(px[1]), pm(px[2])]);
        out.extend_from_slice(&word.to_ne_bytes());
    }
    out
}

/// AETHERMACGPU (SR309): where a Surface's pixels land, in clip space, on an embassy that draws them as a GPU quad
/// (`platforms/macos/gpu_view.rs` `PageGpuView`): the 1:1 top-left pinned placement the CPU page draws
/// (`image_view` `image_dest`'s browser arm: one surface pixel per view point), as `[left, top, right, bottom]` in
/// NDC (y up). An image larger than the view overhangs past 1 / -1 (the GPU clips it); `None` for an empty view or
/// image. Pure, so the placement is testable without a Metal device.
pub fn surface_ndc(view_w: f64, view_h: f64, img_w: u32, img_h: u32) -> Option<[f32; 4]> {
    if !(view_w > 0.0 && view_h > 0.0) || img_w == 0 || img_h == 0 {
        return None;
    }
    let right = -1.0 + 2.0 * img_w as f64 / view_w;
    let bottom = 1.0 - 2.0 * img_h as f64 / view_h;
    Some([-1.0, 1.0, right as f32, bottom as f32])
}

/// AETHERMACGPU-DAMAGE (SR320): the band of rows where `next` differs from `prev`, both `width x height` RGBA frames
/// (stride `width * 4`), as a full-width rect `(0, y, width, h)`. `None` when the two cannot be compared (a size or
/// length mismatch, an empty frame: send it whole); `Some(None)` when they are identical (nothing to send). Each row is one
/// slice compare, walked from the top and from the bottom, each walk stopping at its first differing row.
pub fn surface_rows_damage(prev: &[u8], next: &[u8], width: u32, height: u32) -> Option<Option<(u32, u32, u32, u32)>> {
    let stride = width as usize * 4;
    let len = stride * height as usize;
    if len == 0 || prev.len() != len || next.len() != len {
        return None;
    }
    let differs = |y: usize| prev[y * stride..(y + 1) * stride] != next[y * stride..(y + 1) * stride];
    let h = height as usize;
    let Some(top) = (0..h).find(|&y| differs(y)) else {
        return Some(None);
    };
    let bottom = (top..h).rev().find(|&y| differs(y)).unwrap_or(top);
    Some(Some((0, top as u32, width, (bottom - top + 1) as u32)))
}

/// AETHERMACGPU-DAMAGE (SR320): write `rows` (the rect's rows, stride `w * 4`) into `kept`, a `width x height` frame of
/// 4-byte pixels (RGBA or cairo ARgb32 alike), at `(x, y)`. `false`, and `kept` untouched, when the rect leaves the frame
/// or a length does not match (the receiver then drops the blit; the sender's next whole frame repairs it).
pub fn surface_splice(kept: &mut [u8], width: u32, height: u32, rect: (u32, u32, u32, u32), rows: &[u8]) -> bool {
    let (x, y, w, h) = (rect.0 as usize, rect.1 as usize, rect.2 as usize, rect.3 as usize);
    let (fw, fh) = (width as usize, height as usize);
    if kept.len() != fw * fh * 4 || x + w > fw || y + h > fh || rows.len() != w * h * 4 {
        return false;
    }
    for r in 0..h {
        let dst = ((y + r) * fw + x) * 4;
        kept[dst..dst + w * 4].copy_from_slice(&rows[r * w * 4..(r + 1) * w * 4]);
    }
    true
}

/// AETHERMACGPU-DAMAGE (SR320): a sender's side of damage blits. It keeps the frame it last sent and turns the next one
/// into the `SurfaceBlit` to fire: the painter's own rects when it has them ([`SurfaceDamage::frame_rects`], SR341), else
/// the changed row band only (`damage: Some`), nothing when identical, the whole frame
/// (`damage: None`) on the first send, on a size change, after [`SurfaceDamage::forget`] (the receiver's frame was
/// replaced by someone else) and at least once every [`bandy::SURFACE_FULL_EVERY`] sends (the bus is lossy: a lagged
/// receiver heals). The frame is moved into the kept slot, never copied, after the band is cut.
#[derive(Debug, Default)]
pub struct SurfaceDamage {
    dims: (u32, u32),
    prev: Vec<u8>,
    since_full: u32,
}

impl SurfaceDamage {
    /// The blit to fire for this frame, `None` when it is identical to the last one sent (the row diff decides).
    pub fn frame(&mut self, url: &str, width: u32, height: u32, pixels: Vec<u8>) -> Option<bandy::SMessage> {
        self.frame_rects(url, width, height, pixels, None)
    }

    /// AETHERMACGPU-ENGINEDAMAGE (SR341): the blit to fire for this frame when the painter names what it changed: `rects`
    /// (in this frame's pixels) through `damage::merge`: nothing when they are empty, those rects' rows when they are few
    /// and small; the row diff when `rects` is `None` or covers most of the frame. The whole-frame rules come first.
    pub fn frame_rects(&mut self, url: &str, width: u32, height: u32, pixels: Vec<u8>, rects: Option<&[(u32, u32, u32, u32)]>) -> Option<bandy::SMessage> {
        let damage = if self.dims == (width, height) && self.since_full + 1 < bandy::SURFACE_FULL_EVERY {
            match rects.and_then(|r| crate::damage::merge(r, width, height)) {
                Some(list) => Some(Some(list)),
                None => surface_rows_damage(&self.prev, &pixels, width, height).map(|band| band.map(|r| vec![r])),
            }
        } else {
            None
        };
        let msg = match damage {
            Some(None) => return None,
            Some(Some(list)) if list.is_empty() => {
                self.prev = pixels;
                return None;
            }
            Some(Some(list)) => {
                let rows = crate::damage::cut(&pixels, width, &list);
                self.since_full += 1;
                bandy::SMessage::SurfaceBlit { url: url.to_string(), width, height, pixels: rows, damage: Some(list), scroll: None }
            }
            None => {
                self.since_full = 0;
                bandy::SMessage::SurfaceBlit { url: url.to_string(), width, height, pixels: pixels.clone(), damage: None, scroll: None }
            }
        };
        self.dims = (width, height);
        self.prev = pixels;
        Some(msg)
    }

    /// The receiver's frame is no longer the one last sent: the next frame goes whole.
    pub fn forget(&mut self) {
        self.dims = (0, 0);
        self.prev.clear();
        self.since_full = 0;
    }
}

#[cfg(test)]
mod surface_contract_tests {
    use super::{SurfaceDamage, surface_argb32, surface_ndc, surface_rows_damage, surface_splice};

    /// RGBA in, RGBA out: an opaque row through the hand-off decodes from cairo's native word back to the caller's
    /// RGBA, and the word is 0xAARRGGBB (probe_view's swizzle KAT, moved with the fn).
    #[test]
    fn surface_rgba_contract_kat() {
        let row = [0x11, 0x22, 0x33, 0xFF, 0xFE, 0x00, 0x80, 0xFF, 0x00, 0x00, 0x00, 0xFF];
        let out = surface_argb32(&row);
        assert_eq!(out.len(), row.len());
        assert_eq!(u32::from_ne_bytes([out[0], out[1], out[2], out[3]]), 0xFF11_2233);
        let back: Vec<u8> = out
            .chunks_exact(4)
            .flat_map(|w| {
                let [a, r, g, b] = u32::from_ne_bytes([w[0], w[1], w[2], w[3]]).to_be_bytes();
                [r, g, b, a]
            })
            .collect();
        assert_eq!(back, row);
    }

    /// cairo's ARgb32 is premultiplied: hand-computed translucent and clear pixels.
    #[test]
    fn surface_argb32_premultiplies_kat() {
        let out = surface_argb32(&[200, 100, 50, 128, 255, 255, 255, 0, 255, 0, 0, 1]);
        let word = |i: usize| u32::from_ne_bytes([out[i], out[i + 1], out[i + 2], out[i + 3]]);
        // 200*128/255 = 100.39 -> 100 (0x64); 100*128/255 = 50.2 -> 50 (0x32); 50*128/255 = 25.1 -> 25 (0x19).
        assert_eq!(word(0), 0x8064_3219);
        assert_eq!(word(4), 0x0000_0000);
        // 255*1/255 = 1.
        assert_eq!(word(8), 0x0101_0000);
        assert_eq!(out.len(), 12);
    }

    /// AETHERMACGPU (SR309): the GPU page quad is the CPU page's 1:1 top-left placement: an exact fill, a quarter
    /// (half each way), an overhang past the view's right and bottom edges, and the empty cases.
    #[test]
    fn surface_ndc_top_left_kat() {
        assert_eq!(surface_ndc(800.0, 600.0, 800, 600), Some([-1.0, 1.0, 1.0, -1.0]));
        assert_eq!(surface_ndc(800.0, 600.0, 400, 300), Some([-1.0, 1.0, 0.0, 0.0]));
        assert_eq!(surface_ndc(400.0, 300.0, 800, 600), Some([-1.0, 1.0, 3.0, -3.0]));
        assert_eq!(surface_ndc(0.0, 600.0, 800, 600), None);
        assert_eq!(surface_ndc(800.0, 600.0, 0, 600), None);
        assert_eq!(surface_ndc(800.0, 600.0, 800, 0), None);
    }

    /// AETHERMACGPU-DAMAGE (SR320): the changed row band, top and bottom walks; identical; not comparable.
    #[test]
    fn surface_rows_damage_kat() {
        let a = vec![0u8; 3 * 4 * 4]; // 3x4
        let mut b = a.clone();
        assert_eq!(surface_rows_damage(&a, &b, 3, 4), Some(None));
        b[1 * 12 + 5] = 9; // row 1
        assert_eq!(surface_rows_damage(&a, &b, 3, 4), Some(Some((0, 1, 3, 1))));
        b[3 * 12] = 9; // row 3 too: the band spans 1..=3
        assert_eq!(surface_rows_damage(&a, &b, 3, 4), Some(Some((0, 1, 3, 3))));
        assert_eq!(surface_rows_damage(&a, &b[..36], 3, 4), None);
        assert_eq!(surface_rows_damage(&a, &b, 4, 3), Some(Some((0, 1, 4, 2)))); // same bytes read 4 wide: bytes 17 and 36 are rows 1 and 2 (the caller checks dims)
        assert_eq!(surface_rows_damage(&[], &[], 0, 0), None);
    }

    /// AETHERMACGPU-DAMAGE (SR320): a sub-rect lands at its place; out-of-frame and length mismatches leave the frame alone.
    #[test]
    fn surface_splice_kat() {
        let mut kept = vec![0u8; 3 * 2 * 4]; // 3x2
        assert!(surface_splice(&mut kept, 3, 2, (1, 1, 2, 1), &[1, 2, 3, 4, 5, 6, 7, 8]));
        assert_eq!(&kept[12..], &[0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&kept[..12], &[0u8; 12]);
        let before = kept.clone();
        assert!(!surface_splice(&mut kept, 3, 2, (2, 1, 2, 1), &[0; 8]));
        assert!(!surface_splice(&mut kept, 3, 2, (0, 0, 1, 1), &[0; 8]));
        assert!(!surface_splice(&mut kept, 3, 3, (0, 0, 1, 1), &[0; 4]));
        assert_eq!(kept, before);
    }

    /// AETHERMACGPU-DAMAGE (SR320): the sender's decisions in order: first frame whole, one changed row as that row's
    /// bytes only, identical as nothing, a size change whole, the SURFACE_FULL_EVERY-th send whole, forget whole; and a
    /// receiver splicing every band it is sent ends on the sender's frame.
    #[test]
    fn surface_damage_tracker_kat() {
        use bandy::SMessage;
        let (w, h) = (4u32, 3u32);
        let blank = vec![0u8; (w * h * 4) as usize];
        let mut tx = SurfaceDamage::default();
        let mut rx: Vec<u8> = Vec::new();
        let apply = |m: Option<SMessage>, rx: &mut Vec<u8>| -> Option<Option<(u32, u32, u32, u32)>> {
            let SMessage::SurfaceBlit { width, height, pixels, damage, .. } = m? else { panic!("not a blit") };
            match &damage {
                None => *rx = pixels,
                Some(list) => {
                    for (r, range) in crate::damage::spans(list, width, height, pixels.len()).expect("spans") {
                        assert!(surface_splice(rx, width, height, r, &pixels[range]));
                    }
                }
            }
            Some(damage.map(|list| {
                assert_eq!(list.len(), 1, "the row diff sends one band");
                list[0]
            }))
        };
        assert_eq!(apply(tx.frame("viewport", w, h, blank.clone()), &mut rx), Some(None));
        let mut f = blank.clone();
        f[2 * 16 + 3] = 7;
        let m = tx.frame("viewport", w, h, f.clone());
        assert!(matches!(&m, Some(SMessage::SurfaceBlit { pixels, .. }) if pixels.len() == 16));
        assert_eq!(apply(m, &mut rx), Some(Some((0, 2, 4, 1))));
        assert_eq!(rx, f);
        assert!(tx.frame("viewport", w, h, f.clone()).is_none());
        assert_eq!(apply(tx.frame("viewport", 3, 4, vec![1; 48]), &mut rx), Some(None));
        // From a whole frame, sends 1..FULL_EVERY-1 are bands, the FULL_EVERY-th is whole again.
        let mut g = vec![1u8; 48];
        for i in 1..bandy::SURFACE_FULL_EVERY {
            g[0] = g[0].wrapping_add(1);
            assert_eq!(apply(tx.frame("viewport", 3, 4, g.clone()), &mut rx), Some(Some((0, 0, 3, 1))), "send {i}");
        }
        g[0] = g[0].wrapping_add(1);
        assert_eq!(apply(tx.frame("viewport", 3, 4, g.clone()), &mut rx), Some(None));
        assert_eq!(rx, g);
        tx.forget();
        assert_eq!(apply(tx.frame("viewport", 3, 4, g.clone()), &mut rx), Some(None));
    }

    /// AETHERMACGPU-ENGINEDAMAGE (SR341): the painter's rects: two apart rects send exactly their rows (not the band
    /// between them), an empty list sends nothing, `None` falls to the row diff, most-of-the-frame falls to the row diff,
    /// and the whole-frame rules still win; a receiver walking `damage::spans` ends on the sender's frame.
    #[test]
    fn surface_damage_rects_kat() {
        use bandy::SMessage;
        let (w, h) = (8u32, 8u32);
        let mut tx = SurfaceDamage::default();
        let mut rx = vec![0u8; (w * h * 4) as usize];
        let whole = tx.frame_rects("viewport", w, h, rx.clone(), Some(&[(0, 0, 1, 1)]));
        assert!(matches!(whole, Some(SMessage::SurfaceBlit { damage: None, .. })), "the first send is whole");
        let mut f = rx.clone();
        let set = |f: &mut Vec<u8>, x: u32, y: u32| f[((y * w + x) * 4) as usize] = 9;
        set(&mut f, 1, 0);
        set(&mut f, 6, 7);
        let rects = [(1, 0, 1, 1), (6, 7, 1, 1)];
        let Some(SMessage::SurfaceBlit { pixels, damage: Some(list), width, height, .. }) = tx.frame_rects("viewport", w, h, f.clone(), Some(&rects)) else {
            panic!("a rect blit")
        };
        assert_eq!(list, rects.to_vec());
        assert_eq!(pixels.len(), 2 * 4, "two pixels, not the 8-row band");
        for (r, range) in crate::damage::spans(&list, width, height, pixels.len()).expect("spans") {
            assert!(surface_splice(&mut rx, width, height, r, &pixels[range]));
        }
        assert_eq!(rx, f);
        assert!(tx.frame_rects("viewport", w, h, f.clone(), Some(&[])).is_none(), "nothing painted, nothing sent");
        set(&mut f, 3, 4);
        let band = tx.frame_rects("viewport", w, h, f.clone(), None);
        assert!(matches!(band, Some(SMessage::SurfaceBlit { damage: Some(ref l), .. }) if *l == vec![(0, 4, 8, 1)]), "None: the row diff");
        set(&mut f, 3, 5);
        let most = tx.frame_rects("viewport", w, h, f.clone(), Some(&[(0, 0, 8, 8)]));
        assert!(matches!(most, Some(SMessage::SurfaceBlit { damage: Some(ref l), .. }) if *l == vec![(0, 5, 8, 1)]), "whole cover: the row diff");
        tx.forget();
        assert!(matches!(tx.frame_rects("viewport", w, h, f, Some(&rects)), Some(SMessage::SurfaceBlit { damage: None, .. })), "forget: whole");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// SURFACEID (LEDGER SR332): a Surface's number on the wire
// ---------------------------------------------------------------------------------------------------------------------

/// SURFACEID (LEDGER SR332): a tree's [`TetraNode::Surface`] ids in depth-first order. A Surface's number (the `surface`
/// on bandy's pointer messages) is its index here, so the first or only Surface is 0 and a one-surface tree's wire is
/// unchanged. Pure: the vessel that declared the tree computes the same numbers the embassy that evaluates it does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SurfaceMap {
    ids: Vec<String>,
}

thread_local! {
    /// The map of the tree an embassy is evaluating on this thread ([`SurfaceMap::scope`]).
    static CURRENT_SURFACES: std::cell::RefCell<Option<SurfaceMap>> = const { std::cell::RefCell::new(None) };
}

impl SurfaceMap {
    /// Every Surface of `node`, depth-first (only `VStack` / `HStack` hold children).
    pub fn of(node: &TetraNode) -> SurfaceMap {
        fn walk(n: &TetraNode, ids: &mut Vec<String>) {
            match n {
                TetraNode::Surface { id } => ids.push(id.clone()),
                TetraNode::VStack(children) | TetraNode::HStack(children) => children.iter().for_each(|c| walk(c, ids)),
                _ => {}
            }
        }
        let mut ids = Vec::new();
        walk(node, &mut ids);
        SurfaceMap { ids }
    }
    /// The number of the Surface `id` (its first occurrence), if the tree has one.
    pub fn number(&self, id: &str) -> Option<u32> {
        self.ids.iter().position(|s| s == id).map(|i| i as u32)
    }
    /// The id of Surface number `n`.
    pub fn id(&self, n: u32) -> Option<&str> {
        self.ids.get(n as usize).map(String::as_str)
    }
    /// Run `f` (an embassy's synchronous evaluation of the tree) with this map current on this thread, so each Surface
    /// leaf reads its number through [`surface_number`]; the previous map is restored after.
    pub fn scope<R>(&self, f: impl FnOnce() -> R) -> R {
        let prev = CURRENT_SURFACES.with(|c| c.replace(Some(self.clone())));
        let out = f();
        CURRENT_SURFACES.with(|c| *c.borrow_mut() = prev);
        out
    }
}

/// The number of Surface `id` in the tree being evaluated on this thread ([`SurfaceMap::scope`]); 0 outside any scope or
/// for an id the tree does not name (the one-surface default).
pub fn surface_number(id: &str) -> u32 {
    CURRENT_SURFACES.with(|c| c.borrow().as_ref().and_then(|m| m.number(id)).unwrap_or(0))
}

#[cfg(test)]
mod surface_map_tests {
    use super::{SurfaceMap, TetraNode, surface_number};

    fn surf(id: &str) -> TetraNode {
        TetraNode::Surface { id: id.into() }
    }

    /// SURFACEID M3: depth-first numbering of a nested tree, the one-surface tree's 0, unknown ids, and the scope.
    #[test]
    fn surfaceid_map_numbers_depth_first() {
        let tree = TetraNode::VStack(vec![
            surf("aux"),
            TetraNode::HStack(vec![TetraNode::Empty, surf("editor"), TetraNode::VStack(vec![surf("meters")])]),
            surf("foot"),
        ]);
        let m = SurfaceMap::of(&tree);
        assert_eq!([m.number("aux"), m.number("editor"), m.number("meters"), m.number("foot")], [Some(0), Some(1), Some(2), Some(3)]);
        assert_eq!((m.number("nope"), m.id(1), m.id(4)), (None, Some("editor"), None));
        assert_eq!(SurfaceMap::of(&TetraNode::VStack(vec![TetraNode::Empty, surf("stria.nle")])).number("stria.nle"), Some(0));
        assert_eq!(SurfaceMap::of(&TetraNode::Empty), SurfaceMap::default());
        assert_eq!(surface_number("editor"), 0, "outside a scope: the default surface");
        let inner = SurfaceMap::of(&TetraNode::VStack(vec![surf("x"), surf("y")]));
        let got = m.scope(|| {
            let nested = inner.scope(|| (surface_number("y"), surface_number("editor")));
            (surface_number("editor"), surface_number("meters"), surface_number("nope"), nested)
        });
        assert_eq!(got, (1, 2, 0, (1, 0)));
        assert_eq!(surface_number("editor"), 0, "the scope is restored");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// AETHERMACGPU-PRESENTRECTS / -SCROLL (LEDGER SR351): the kept frame converted in place; the scroll copy-rect
// ---------------------------------------------------------------------------------------------------------------------

type Rect = crate::damage::Rect;

impl SurfaceDamage {
    /// [`SurfaceDamage::frame_rects`] for a painter that converts IN PLACE into the frame kept here (the frame last sent,
    /// identical to the receiver's: the ONE persistent copy). When this blit can carry rects (same size, not due whole,
    /// `damage::merge(rects)` gives a list), the kept frame is moved by `scroll` (`damage::scroll`), `convert(kept, list)`
    /// writes ONLY the merged list's pixels into it, and the blit carries the list cut from it plus `scroll`; no whole frame
    /// is made. Otherwise, or when `convert` refuses (it returns `false` before writing anything), `whole()` makes the frame
    /// and [`SurfaceDamage::frame_rects`] decides as before (the kept frame forgotten first when it was already moved).
    #[allow(clippy::too_many_arguments)]
    pub fn present(
        &mut self,
        url: &str,
        width: u32,
        height: u32,
        rects: &[Rect],
        scroll: Option<(i32, i32)>,
        convert: impl FnOnce(&mut [u8], &[Rect]) -> bool,
        whole: impl FnOnce() -> (u32, u32, Vec<u8>),
    ) -> Option<bandy::SMessage> {
        let fast = self.dims == (width, height)
            && self.since_full + 1 < bandy::SURFACE_FULL_EVERY
            && self.prev.len() == width as usize * height as usize * 4;
        if let Some(list) = crate::damage::merge(rects, width, height).filter(|_| fast) {
            if list.is_empty() && scroll.is_none() {
                return None;
            }
            let moved = match scroll {
                Some(s) => crate::damage::scroll(&mut self.prev, width, height, s),
                None => true,
            };
            if moved && convert(&mut self.prev, &list) {
                self.since_full += 1;
                let rows = crate::damage::cut(&self.prev, width, &list);
                return Some(bandy::SMessage::SurfaceBlit { url: url.to_string(), width, height, pixels: rows, damage: Some(list), scroll });
            }
            if scroll.is_some() {
                self.forget();
            }
        }
        let (w, h, pixels) = whole();
        self.frame_rects(url, w, h, pixels, Some(rects))
    }
}

/// AETHERMACGPU-SCROLL (SR351): a damage blit applied to the `width x height` frame a receiver keeps (4-byte pixels, RGBA
/// or cairo ARgb32 alike: `rows` in the kept frame's format): the copy-rect first (`damage::scroll` by `scroll`), then each
/// rect's rows (`damage::spans`, [`surface_splice`]). Everything is checked before anything is written: `false`, and
/// `kept` untouched, when the rects do not fit, `rows` is not their length, `kept` is not the frame or the scroll leaves
/// it (the receiver drops the blit; the sender's next whole frame repairs it). The receivers' ONE apply (GTK `image_view`,
/// AppKit `image_view::set_damage`).
pub fn surface_apply(kept: &mut [u8], width: u32, height: u32, rects: &[Rect], rows: &[u8], scroll: Option<(i32, i32)>) -> bool {
    let Some(spans) = crate::damage::spans(rects, width, height, rows.len()) else {
        return false;
    };
    let fits = |s: (i32, i32)| (s.0.unsigned_abs() as u64) < width as u64 && (s.1.unsigned_abs() as u64) < height as u64;
    if kept.len() != width as usize * height as usize * 4 || !scroll.is_none_or(fits) {
        return false;
    }
    if let Some(s) = scroll {
        crate::damage::scroll(kept, width, height, s);
    }
    spans.into_iter().all(|(r, range)| surface_splice(kept, width, height, r, &rows[range]))
}

#[cfg(test)]
mod surface_present_tests {
    use super::{SurfaceDamage, surface_apply};
    use bandy::SMessage;

    /// A frame of `w x h` whose pixel (x, y) is `f(x, y)` in every byte.
    fn frame(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| [f(x, y); 4]).collect()
    }

    /// The receivers' apply: scroll then the rects equals the reference; a misfit (rects, length, scroll, frame) refuses
    /// with the kept frame untouched.
    #[test]
    fn surface_apply_kat() {
        let (w, h) = (4u32, 4u32);
        let old = frame(w, h, |x, y| (y * 10 + x) as u8);
        // The page scrolled down one row: rows 1..4 move up, row 3 is the new band (value 99).
        let want = frame(w, h, |x, y| if y == 3 { 99 } else { ((y + 1) * 10 + x) as u8 });
        let mut kept = old.clone();
        assert!(surface_apply(&mut kept, w, h, &[(0, 3, 4, 1)], &[99u8; 16], Some((0, -1))));
        assert_eq!(kept, want);
        let mut kept = old.clone();
        assert!(surface_apply(&mut kept, w, h, &[(1, 1, 1, 1)], &[7u8; 4], None));
        assert_eq!(kept, frame(w, h, |x, y| if (x, y) == (1, 1) { 7 } else { (y * 10 + x) as u8 }));
        for (rects, rows, scroll) in [
            (vec![(0, 3, 4, 2)], vec![0u8; 32], Some((0, -1))),
            (vec![(0, 3, 4, 1)], vec![0u8; 12], Some((0, -1))),
            (vec![(0, 3, 4, 1)], vec![0u8; 16], Some((0, -4))),
            (vec![(0, 3, 4, 1)], vec![0u8; 16], Some((4, 0))),
        ] {
            let mut kept = old.clone();
            assert!(!surface_apply(&mut kept, w, h, &rects, &rows, scroll), "{rects:?} {scroll:?}");
            assert_eq!(kept, old);
        }
        assert!(!surface_apply(&mut vec![0u8; 8], w, h, &[], &[], Some((0, 1))), "not the frame");
    }

    /// The sender converts only the merged list into its kept frame and the blit carries those rows (and the scroll); the
    /// receiver ends on the painter's frame; a refusal or a whole-frame rule makes the whole frame.
    #[test]
    fn surface_damage_present_kat() {
        let (w, h) = (8u32, 8u32);
        let painter = |y0: u32| frame(w, h, move |x, y| ((y + y0) * 8 + x) as u8); // the page scrolled to row y0
        let mut tx = SurfaceDamage::default();
        let first = tx.present("viewport", w, h, &[(0, 0, 8, 8)], None, |_, _| panic!("no kept frame yet"), || (w, h, painter(0)));
        let Some(SMessage::SurfaceBlit { damage: None, pixels, scroll: None, .. }) = first else { panic!("the first send is whole") };
        let mut rx = pixels;
        // One rect: only its pixels are converted (the closure sees the merged list and the kept frame).
        let mut seen = Vec::new();
        let m = tx.present("viewport", w, h, &[(2, 2, 1, 1)], None, |kept, list| {
            seen = list.to_vec();
            kept[(2 * 8 + 2) * 4..(2 * 8 + 2) * 4 + 4].copy_from_slice(&[200; 4]);
            true
        }, || panic!("no whole frame"));
        assert_eq!(seen, vec![(2, 2, 1, 1)]);
        let Some(SMessage::SurfaceBlit { damage: Some(list), pixels, scroll: None, .. }) = m else { panic!("a rect blit") };
        assert_eq!((list.clone(), pixels.clone()), (vec![(2, 2, 1, 1)], vec![200; 4]));
        assert!(surface_apply(&mut rx, w, h, &list, &pixels, None));
        let mut expect = painter(0);
        expect[(2 * 8 + 2) * 4..(2 * 8 + 2) * 4 + 4].copy_from_slice(&[200; 4]);
        assert_eq!(rx, expect);
        // A scroll of two rows: the kept frame moves, the band (rows 6..8) is converted, the wire carries the pair.
        let next = painter(2);
        let m = tx.present("viewport", w, h, &[(0, 6, 8, 2)], Some((0, -2)), |kept, list| {
            for &(x, y, rw, rh) in list {
                for yy in y..y + rh {
                    let a = ((yy * w + x) * 4) as usize;
                    kept[a..a + (rw * 4) as usize].copy_from_slice(&next[a..a + (rw * 4) as usize]);
                }
            }
            true
        }, || panic!("no whole frame"));
        let Some(SMessage::SurfaceBlit { damage: Some(list), pixels, scroll: Some((0, -2)), .. }) = m else { panic!("a scroll blit") };
        assert_eq!((list.clone(), pixels.len()), (vec![(0, 6, 8, 2)], 2 * 8 * 4));
        assert!(surface_apply(&mut rx, w, h, &list, &pixels, Some((0, -2))));
        let mut want = next.clone();
        let a = ((2 * 8 + 2 - 2 * 8) * 4) as usize; // the rect painted at row 2 moved to row 0
        want[a..a + 4].copy_from_slice(&[200; 4]);
        assert_eq!(rx, want);
        // A refusal without a scroll: the whole frame through frame_rects (rects, cut from the whole frame).
        let m = tx.present("viewport", w, h, &[(1, 1, 1, 1)], None, |_, _| false, || (w, h, next.clone()));
        assert!(matches!(m, Some(SMessage::SurfaceBlit { damage: Some(ref l), scroll: None, .. }) if *l == vec![(1, 1, 1, 1)]));
        // A refusal after the scroll moved the kept frame: forgotten, whole.
        let m = tx.present("viewport", w, h, &[(0, 7, 8, 1)], Some((0, -1)), |_, _| false, || (w, h, painter(3)));
        assert!(matches!(m, Some(SMessage::SurfaceBlit { damage: None, scroll: None, .. })), "moved then refused: whole");
        // Nothing painted, no scroll: nothing sent; most of the frame: the whole-frame path decides.
        assert!(tx.present("viewport", w, h, &[], None, |_, _| panic!("nothing to convert"), || panic!("no frame")).is_none());
        let m = tx.present("viewport", w, h, &[(0, 0, 8, 8)], None, |_, _| panic!("merge goes whole"), || (w, h, painter(4)));
        assert!(matches!(m, Some(SMessage::SurfaceBlit { scroll: None, .. })));
    }
}

// LANDINGSECRETFIELD (SR432): the secret TextField's mask, the ONE rule GTK's GlyphEntry paints, traces and hit-tests
// through (AppKit draws its own bullets). One bullet per grapheme cluster, on font_core's boundaries (the ones
// GlyphEntry's caret steps), so the caret and selection map exactly between the text and what is drawn.

/// The one glyph a secret field draws per grapheme cluster (U+2022 BULLET).
pub const SECRET_BULLET: char = '•';

/// What a secret field draws for `text`: one [`SECRET_BULLET`] per grapheme cluster, nothing of `text` itself.
pub fn secret_mask(text: &str) -> String {
    std::iter::repeat(SECRET_BULLET).take(text_host::font_core::grapheme::clusters(text).len()).collect()
}

/// Byte offset `i` of `text` (a cluster boundary) as its byte offset in [`secret_mask`]`(text)`.
pub fn secret_offset(text: &str, i: usize) -> usize {
    text_host::font_core::grapheme::clusters(text).iter().take_while(|&&(a, _)| a < i).count() * SECRET_BULLET.len_utf8()
}

/// Byte offset `m` of [`secret_mask`]`(text)` back to the cluster boundary of `text` it stands for (past the end:
/// `text.len()`).
pub fn secret_unoffset(text: &str, m: usize) -> usize {
    text_host::font_core::grapheme::clusters(text).get(m / SECRET_BULLET.len_utf8()).map_or(text.len(), |&(a, _)| a)
}

#[cfg(test)]
mod secret_field_tests {
    use super::{secret_mask, secret_offset, secret_unoffset, TetraNode, TextAction, SECRET_BULLET};

    /// The mask carries nothing of the secret: one bullet per cluster, a combining pair and a ZWJ family one each.
    #[test]
    fn secret_mask_draws_one_bullet_per_cluster() {
        let m = secret_mask("hunter2");
        assert_eq!(m, "•••••••");
        assert!("hunter2".chars().all(|c| !m.contains(c)));
        assert_eq!(secret_mask(""), "");
        assert_eq!(secret_mask("e\u{301}x").chars().count(), 2, "a combining pair is one cluster");
        assert_eq!(secret_mask("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}").chars().count(), 1, "a ZWJ family is one cluster");
        assert!(m.chars().all(|c| c == SECRET_BULLET));
    }

    /// Every cluster boundary of the text maps into the mask and back to itself; past the end clamps to the text.
    #[test]
    fn secret_offsets_round_trip_on_every_boundary() {
        for t in ["sk-ant-TEST-SECRET", "e\u{301}a\u{1F468}\u{200D}\u{1F469}z", ""] {
            let mask = secret_mask(t);
            let mut bounds: Vec<usize> = text_host::font_core::grapheme::clusters(t).iter().map(|&(a, _)| a).collect();
            bounds.push(t.len());
            for (n, &b) in bounds.iter().enumerate() {
                let m = secret_offset(t, b);
                assert_eq!(m, n * SECRET_BULLET.len_utf8());
                assert!(mask.is_char_boundary(m));
                assert_eq!(secret_unoffset(t, m), b, "{t:?} boundary {b}");
            }
            assert_eq!(secret_offset(t, t.len() + 9), mask.len());
            assert_eq!(secret_unoffset(t, mask.len() + 9), t.len());
        }
    }

    /// The wire: a tree serialized before the field existed reads `secret: false`; a secret field round-trips.
    #[test]
    fn secret_field_serde_default_and_round_trip() {
        let old = r#"{"TextField":{"id":"url","placeholder":"Enter URL...","action":"OpenDocument"}}"#;
        let n: TetraNode = serde_json::from_str(old).unwrap();
        assert!(matches!(n, TetraNode::TextField { secret: false, .. }));
        let s = TetraNode::TextField { id: "k".into(), placeholder: "key".into(), action: TextAction::ConsoleInput, secret: true };
        let back: TetraNode = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(matches!(back, TetraNode::TextField { secret: true, ref id, .. } if id == "k"));
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// FACELIVE (LEDGER SR435): the face-push door — a running tetra vessel's tree replaced in place
// ---------------------------------------------------------------------------------------------------------------------

/// FACELIVE (SR435): the ONE key of a tree (a hash of its `Debug` form): a push whose key equals the newest is a no-op.
pub fn face_key(node: &TetraNode) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{node:?}").hash(&mut h);
    h.finish()
}

/// FACELIVE (SR435): one in-place change an embassy makes to the tree it shows. `path` is the child indices from the
/// root (empty = the root). Only the smallest changed node is touched, so a Console keeps its widget (and its bus
/// subscriber) and a field keeps its typed text and focus.
#[derive(Clone, Debug)]
pub enum FacePatch {
    /// A Console's lines / header changed: written into the existing view.
    Lines { path: Vec<usize>, console: ConsoleTetra },
    /// A TextField with the same id and action changed only its placeholder: the field's typed text and focus kept.
    Field { path: Vec<usize>, node: TetraNode },
    /// A stack changed its kind or child count: the container kept, its children evaluated afresh. `keep`
    /// (FACELIVE-FIELDKEEP, SR444): the fields under it whose typed text and focus move into the rebuilt ones.
    Restack { path: Vec<usize>, node: TetraNode, keep: Vec<FaceKeep> },
    /// Any other changed node: evaluated afresh in its parent's slot (`keep` as for Restack).
    Replace { path: Vec<usize>, node: TetraNode, keep: Vec<FaceKeep> },
}

/// FACELIVE-FIELDKEEP (SR444): a TextField under a Restack or Replace that the new subtree holds again (the same id
/// and action, the Field patch's own identity): the embassy reads the widget at `old` (absolute path, before the
/// rebuild) and writes its typed text and focus into the widget at `new` (after it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceKeep {
    pub id: String,
    pub old: Vec<usize>,
    pub new: Vec<usize>,
}

fn face_same(a: &TetraNode, b: &TetraNode) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

/// FACELIVE (SR435): the patches that turn the shown tree `old` into `new` (empty when they are equal).
pub fn face_diff(old: &TetraNode, new: &TetraNode) -> Vec<FacePatch> {
    fn walk(old: &TetraNode, new: &TetraNode, path: &mut Vec<usize>, out: &mut Vec<FacePatch>) {
        match (old, new) {
            (TetraNode::VStack(a), TetraNode::VStack(b)) | (TetraNode::HStack(a), TetraNode::HStack(b)) if a.len() == b.len() => {
                for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                    path.push(i);
                    walk(x, y, path, out);
                    path.pop();
                }
            }
            (TetraNode::VStack(_) | TetraNode::HStack(_), TetraNode::VStack(_) | TetraNode::HStack(_)) => {
                out.push(FacePatch::Restack { path: path.clone(), node: new.clone(), keep: face_keep(old, new, path) });
            }
            (TetraNode::Console(a), TetraNode::Console(b)) => {
                if a != b {
                    out.push(FacePatch::Lines { path: path.clone(), console: b.clone() });
                }
            }
            (TetraNode::TextField { id: ia, action: aa, .. }, TetraNode::TextField { id: ib, action: ab, .. })
                if ia == ib && format!("{aa:?}") == format!("{ab:?}") =>
            {
                if !face_same(old, new) {
                    out.push(FacePatch::Field { path: path.clone(), node: new.clone() });
                }
            }
            _ => {
                if !face_same(old, new) {
                    out.push(FacePatch::Replace { path: path.clone(), node: new.clone(), keep: face_keep(old, new, path) });
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(old, new, &mut Vec::new(), &mut out);
    out
}

/// FACELIVE-FIELDKEEP (SR444): the fields of `old` (rooted at `path`) that `new` holds again: each old field paired
/// with the FIRST new field of the same id and action, one keep per id. A keep stays inside its own patch's subtree.
fn face_keep(old: &TetraNode, new: &TetraNode, path: &[usize]) -> Vec<FaceKeep> {
    fn face_fields(node: &TetraNode, path: &mut Vec<usize>, out: &mut Vec<(String, String, Vec<usize>)>) {
        match node {
            TetraNode::VStack(c) | TetraNode::HStack(c) => {
                for (i, child) in c.iter().enumerate() {
                    path.push(i);
                    face_fields(child, path, out);
                    path.pop();
                }
            }
            TetraNode::TextField { id, action, .. } => out.push((id.clone(), format!("{action:?}"), path.clone())),
            _ => {}
        }
    }
    let (mut a, mut b) = (Vec::new(), Vec::new());
    face_fields(old, &mut path.to_vec(), &mut a);
    face_fields(new, &mut path.to_vec(), &mut b);
    let mut keep: Vec<FaceKeep> = Vec::new();
    for (id, action, old_path) in a {
        if keep.iter().any(|k| k.id == id) {
            continue;
        }
        if let Some((_, _, new_path)) = b.iter().find(|(i, act, _)| *i == id && *act == action) {
            keep.push(FaceKeep { id, old: old_path, new: new_path.clone() });
        }
    }
    keep
}

/// FACELIVE (SR435): how an embassy asks its main thread to apply the door's pending tree.
pub type FaceWaker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// FACELIVE (SR435): the door's state: the newest key, the tree the embassy shows, the newest pushed tree not yet
/// applied (latest wins), and the embassy's waker (none until the window opens).
pub struct FaceDoor {
    key: Option<u64>,
    shown: Option<TetraNode>,
    pending: Option<TetraNode>,
    waker: Option<FaceWaker>,
}

impl Default for FaceDoor {
    fn default() -> Self {
        Self::new()
    }
}

impl FaceDoor {
    pub const fn new() -> Self {
        FaceDoor { key: None, shown: None, pending: None, waker: None }
    }

    /// The embassy evaluates the tree this returns: a push that beat the window's opening wins over `initial`.
    pub fn open(&mut self, initial: TetraNode, waker: FaceWaker) -> TetraNode {
        let node = self.pending.take().unwrap_or(initial);
        self.key = Some(face_key(&node));
        self.shown = Some(node.clone());
        self.waker = Some(waker);
        node
    }

    /// Store `node` as the pending tree unless its key equals the newest (then nothing changes: `false`); `true` with
    /// the waker the caller calls once the door's lock is dropped.
    pub fn push(&mut self, node: TetraNode) -> (bool, Option<FaceWaker>) {
        let key = face_key(&node);
        if self.key == Some(key) {
            return (false, None);
        }
        self.key = Some(key);
        self.pending = Some(node);
        (true, self.waker.clone())
    }

    /// The pending tree and the patches from the shown one (the shown tree becomes it); `None` when nothing is pending.
    pub fn take(&mut self) -> Option<(TetraNode, Vec<FacePatch>)> {
        let new = self.pending.take()?;
        let patches = match &self.shown {
            Some(old) => face_diff(old, &new),
            None => vec![FacePatch::Restack { path: Vec::new(), node: new.clone(), keep: Vec::new() }],
        };
        self.shown = Some(new.clone());
        Some((new, patches))
    }
}

/// FACELIVE (SR435): the process's ONE door (one tetra vessel per process), locked briefly by [`push_face`] (any
/// thread) and [`take_face`] (the embassy's main thread).
static FACE_DOOR: std::sync::Mutex<FaceDoor> = std::sync::Mutex::new(FaceDoor::new());

fn face_door() -> std::sync::MutexGuard<'static, FaceDoor> {
    FACE_DOOR.lock().unwrap_or_else(|e| e.into_inner())
}

/// FACELIVE (SR435): the embassy's side at `new_tetra_vessel`: the tree to evaluate, and its waker installed.
pub fn open_face(initial: TetraNode, waker: FaceWaker) -> TetraNode {
    face_door().open(initial, waker)
}

/// FACELIVE (SR435): the vessel's side: hand the running window a new tree. `false` (a no-op) when its key equals the
/// newest pushed or shown tree; else the window is woken to patch itself in place.
pub fn push_face(node: TetraNode) -> bool {
    let (pushed, waker) = face_door().push(node);
    if let Some(wake) = waker {
        wake();
    }
    pushed
}

/// FACELIVE (SR435): the embassy's main thread: the pending tree (for its SurfaceMap) and the patches to apply.
pub fn take_face() -> Option<(TetraNode, Vec<FacePatch>)> {
    face_door().take()
}

#[cfg(test)]
mod face_door_tests {
    use super::{ConsoleLine, ConsoleTetra, FaceDoor, FaceKeep, FacePatch, TetraNode, TextAction, face_diff};
    use bandy::SMessage;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn console(text: &str) -> TetraNode {
        TetraNode::Console(ConsoleTetra { lines: vec![ConsoleLine { level: "info".into(), source: "card".into(), text: text.into() }], ..Default::default() })
    }
    fn button(id: &str, label: &str) -> TetraNode {
        TetraNode::Button { id: id.into(), label: label.into(), action: SMessage::ConsoleInput(id.into()) }
    }
    fn field(placeholder: &str) -> TetraNode {
        TetraNode::TextField { id: "card.confirm".into(), placeholder: placeholder.into(), action: TextAction::ConsoleInput, secret: false }
    }
    /// The installer's shape: a header beside a pane of console, picks, field, button.
    fn face(status: &str, picks: usize, placeholder: &str, write: &str) -> TetraNode {
        TetraNode::HStack(vec![
            button("installer.unaos", "Install UnaOS"),
            TetraNode::VStack(vec![
                console(status),
                TetraNode::HStack((0..picks).map(|i| button(&format!("card.pick.{i}"), "pick")).collect()),
                field(placeholder),
                button("card.write", write),
            ]),
        ])
    }

    /// The diff names only the smallest changed node, by kind.
    #[test]
    fn facelive_diff_kinds() {
        let a = face("idle", 1, "pick a card", "Write");
        assert!(face_diff(&a, &a.clone()).is_empty());
        let p = face_diff(&a, &face("written 4 / 8 MiB", 1, "pick a card", "Write"));
        assert!(matches!(&p[..], [FacePatch::Lines { path, console }] if *path == vec![1, 0] && console.lines[0].text == "written 4 / 8 MiB"), "{p:?}");
        let p = face_diff(&a, &face("idle", 1, "8 GB sdb", "Write"));
        assert!(matches!(&p[..], [FacePatch::Field { path, .. }] if *path == vec![1, 2]), "{p:?}");
        let p = face_diff(&a, &face("idle", 2, "pick a card", "Write"));
        assert!(matches!(&p[..], [FacePatch::Restack { path, node: TetraNode::HStack(k), .. }] if *path == vec![1, 1] && k.len() == 2), "{p:?}");
        let p = face_diff(&a, &face("idle", 1, "pick a card", "Writing"));
        assert!(matches!(&p[..], [FacePatch::Replace { path, .. }] if *path == vec![1, 3]), "{p:?}");
        let p = face_diff(&a, &TetraNode::VStack(vec![button("x", "x")]));
        assert!(matches!(&p[..], [FacePatch::Restack { path, .. }] if path.is_empty()), "{p:?}");
    }

    /// FACELIVE-FIELDKEEP (SR444): a field under a Replace or a Restack that the new subtree holds again is kept (its
    /// old and new paths); a field whose action changed is not.
    #[test]
    fn facelive_keep_field_under_restack_replace() {
        let old = TetraNode::HStack(vec![button("installer.unaos", "Install UnaOS"), TetraNode::VStack(vec![console("idle"), field("pick a card")])]);
        let new = TetraNode::HStack(vec![button("installer.unaos", "Install UnaOS"), field("pick a card")]);
        let p = face_diff(&old, &new);
        let want = vec![FaceKeep { id: "card.confirm".into(), old: vec![1, 1], new: vec![1] }];
        assert!(matches!(&p[..], [FacePatch::Replace { path, keep, .. }] if *path == vec![1] && *keep == want), "{p:?}");
        let a = face("idle", 1, "pick a card", "Write");
        let mut b = a.clone();
        if let TetraNode::HStack(top) = &mut b {
            if let TetraNode::VStack(pane) = &mut top[1] {
                pane.insert(0, console("written 1 / 8 MiB"));
            }
        }
        let p = face_diff(&a, &b);
        let want = vec![FaceKeep { id: "card.confirm".into(), old: vec![1, 2], new: vec![1, 3] }];
        assert!(matches!(&p[..], [FacePatch::Restack { path, keep, .. }] if *path == vec![1] && *keep == want), "{p:?}");
        let other = TetraNode::HStack(vec![
            button("installer.unaos", "Install UnaOS"),
            TetraNode::TextField { id: "card.confirm".into(), placeholder: "pick a card".into(), action: TextAction::OpenDocument, secret: false },
        ]);
        let p = face_diff(&old, &other);
        assert!(matches!(&p[..], [FacePatch::Replace { keep, .. }] if keep.is_empty()), "{p:?}");
    }

    /// A same-key push is a no-op (no wake); a changed one pushes once and wakes once; a push before the window opens
    /// wins its first evaluation; take yields the patches once.
    #[test]
    fn facelive_door_push_once() {
        let woken = Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        let mut door = FaceDoor::new();
        let early = face("early", 1, "pick a card", "Write");
        assert_eq!(door.push(early.clone()).0, true);
        let shown = door.open(face("probe", 1, "pick a card", "Write"), Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(format!("{shown:?}"), format!("{early:?}"), "the early push wins the first evaluation");
        assert!(door.take().is_none());
        let (pushed, waker) = door.push(early.clone());
        assert!(!pushed && waker.is_none(), "same key: a no-op");
        let next = face("written 1 / 8 MiB", 1, "pick a card", "Write");
        let (pushed, waker) = door.push(next.clone());
        assert!(pushed);
        waker.expect("the window's waker")();
        assert_eq!(woken.load(Ordering::SeqCst), 1);
        assert!(!door.push(next.clone()).0, "the pending key is the newest");
        let (full, patches) = door.take().expect("pending");
        assert_eq!(format!("{full:?}"), format!("{next:?}"));
        assert!(matches!(&patches[..], [FacePatch::Lines { .. }]), "{patches:?}");
        assert!(door.take().is_none());
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }
}
