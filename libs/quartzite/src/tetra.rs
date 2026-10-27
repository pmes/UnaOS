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
    /// NLE2ORDER (SR227): the Surface pixel contract. Every `SurfaceBlit { url == id }` carries RGBA8, straight
    /// (non-premultiplied) alpha, row-major, stride `width * 4`, on every embassy; an embassy whose native buffer
    /// differs converts ONCE at its hand-off (GTK's cairo ARgb32: [`surface_argb32`]), so no caller swizzles.
    /// AETHERMACGPU-DAMAGE (SR320): a blit with `damage: Some(rect)` carries only that rect's rows (stride `w * 4`), which
    /// the receiver writes into the frame it holds through [`surface_splice`]; the sender's side is [`SurfaceDamage`].
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
/// into the `SurfaceBlit` to fire: the changed row band only (`damage: Some`), nothing when identical, the whole frame
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
    /// The blit to fire for this frame, `None` when it is identical to the last one sent.
    pub fn frame(&mut self, url: &str, width: u32, height: u32, pixels: Vec<u8>) -> Option<bandy::SMessage> {
        let band = if self.dims == (width, height) && self.since_full + 1 < bandy::SURFACE_FULL_EVERY {
            surface_rows_damage(&self.prev, &pixels, width, height)
        } else {
            None
        };
        let msg = match band {
            Some(None) => return None,
            Some(Some(rect)) => {
                let stride = width as usize * 4;
                let rows = pixels[rect.1 as usize * stride..(rect.1 + rect.3) as usize * stride].to_vec();
                self.since_full += 1;
                bandy::SMessage::SurfaceBlit { url: url.to_string(), width, height, pixels: rows, damage: Some(rect) }
            }
            None => {
                self.since_full = 0;
                bandy::SMessage::SurfaceBlit { url: url.to_string(), width, height, pixels: pixels.clone(), damage: None }
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
            match damage {
                None => *rx = pixels,
                Some(r) => assert!(surface_splice(rx, width, height, r, &pixels)),
            }
            Some(damage)
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
}
