// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! # Facet — The Canvas (CODEX §2: the Images handler)
//!
//! Facet owns pictures on the host: it opens and decodes a file, reads what the file declares
//! about itself (size, depth, alpha, colour space, EXIF orientation — which it APPLIES), keeps a
//! non-destructive edit list per open picture, renders a view of it into any viewport, and exports
//! the baked result as PNG. It serves all of that over the Bandy Synapse as
//! [`bandy::signals::FacetCommand`], and every other surface that meets an image (Matrix's Finder,
//! Aether, the kernel's Quarry twin) delegates to it instead of decoding on its own.
//!
//! The pieces, each its own module:
//! - [`source`] — the [`source::ImageSource`] seam (PIXELCORE's `gneiss_pal::dsp::image` shape);
//!   the `image` crate behind it is CHICKEN WIRE until PIXELCORE lands.
//! - [`meta`] — container metadata from the specifications (Facet's own reader).
//! - [`raster`] — orientation, turns, flips, crop, the triangle resize, CSS brightness/contrast.
//! - [`edit`] — the op list with whole-state undo/redo/reset.
//! - [`view`] — zoom/pan/rotate/flip framing into a viewport.
//! - [`png`] — the PNG writer (zlib/DEFLATE/CRC from the RFCs).

pub mod edit;
pub mod meta;
pub mod png;
pub mod raster;
pub mod source;
pub mod view;

use std::collections::HashMap;
use std::path::Path;

use bandy::signals::{FacetCommand, FacetEdit, FacetFormat, FacetImageInfo, FacetView};
use bandy::{SMessage, Synapse};

use edit::{EditList, Op};
use raster::Raster;
use source::ImageSource;

/// The largest file Facet will read (bigger is refused before reading it into memory).
pub const MAX_FILE_BYTES: u64 = 512 << 20;
/// The largest render viewport on either axis.
pub const MAX_VIEWPORT: u32 = 4096;

/// One open picture.
struct OpenImage {
    path: String,
    format: &'static str,
    meta: meta::Meta,
    frames: u32,
    bytes: u64,
    /// Decoded, EXIF orientation applied — the base every edit list is baked over.
    original: Raster,
    edits: EditList,
    /// The baked result of `edits`, rebuilt lazily after a change.
    baked: Option<Raster>,
}

impl OpenImage {
    fn info(&self) -> FacetImageInfo {
        let (w, h) = self.edits.size_from(self.original.width, self.original.height);
        FacetImageInfo {
            path: self.path.clone(),
            format: self.format.to_string(),
            source_width: self.meta.width,
            source_height: self.meta.height,
            width: w,
            height: h,
            orientation: self.meta.orientation,
            colour: self.meta.colour.clone(),
            bit_depth: self.meta.bit_depth,
            has_alpha: self.meta.has_alpha,
            frames: self.frames,
            bytes: self.bytes,
            edits: self.edits.ops().len() as u32,
        }
    }

    fn baked(&mut self) -> &Raster {
        if self.baked.is_none() {
            self.baked = Some(self.edits.bake(&self.original));
        }
        self.baked.as_ref().expect("just baked")
    }
}

/// The Images handler.
pub struct Facet {
    source: Box<dyn ImageSource>,
    images: HashMap<u64, OpenImage>,
    next: u64,
}

impl Default for Facet {
    fn default() -> Self {
        Self::new()
    }
}

impl Facet {
    /// A handler over this build's default [`ImageSource`].
    pub fn new() -> Self {
        Self::with_source(source::default_source())
    }

    pub fn with_source(source: Box<dyn ImageSource>) -> Self {
        Facet { source, images: HashMap::new(), next: 1 }
    }

    /// Who decodes for this handler.
    pub fn source_name(&self) -> &'static str {
        self.source.name()
    }

    /// Open and decode the file at `path`.
    pub fn open(&mut self, path: &str) -> Result<(u64, FacetImageInfo), String> {
        let md = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
        if !md.is_file() {
            return Err(format!("{path}: not a regular file"));
        }
        if md.len() > MAX_FILE_BYTES {
            return Err(format!("{path}: {} bytes exceeds the {MAX_FILE_BYTES}-byte limit", md.len()));
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        self.open_bytes(path, &bytes)
    }

    /// Open already-read bytes; `path` labels them.
    pub fn open_bytes(&mut self, path: &str, bytes: &[u8]) -> Result<(u64, FacetImageInfo), String> {
        let format = self.source.sniff(bytes).ok_or_else(|| format!("{path}: not an image Facet knows"))?;
        let meta = meta::read(bytes, format);
        let decoded = self.source.decode(bytes).map_err(|e| format!("{path}: {e}"))?;
        if decoded.rgba.len() != decoded.width as usize * decoded.height as usize * 4 {
            return Err(format!("{path}: source returned a short pixel buffer"));
        }
        let frames = decoded.frames.as_ref().map_or(1, |f| f.len() as u32);
        // The orientation Facet applies is the one ITS reader found (identical to the source's
        // report when both read the file; Facet's reader is the authority either way).
        let original = Raster::new(decoded.width, decoded.height, decoded.rgba).oriented(meta.orientation);
        let handle = self.next;
        self.next += 1;
        let img = OpenImage {
            path: path.to_string(),
            format: format.name(),
            meta,
            frames,
            bytes: bytes.len() as u64,
            original,
            edits: EditList::default(),
            baked: None,
        };
        let info = img.info();
        log::info!(
            "[FACET] :: opened {path} as handle {handle}: {} {}x{} via {}",
            info.format,
            info.width,
            info.height,
            self.source.name()
        );
        self.images.insert(handle, img);
        Ok((handle, info))
    }

    fn get(&mut self, handle: u64) -> Result<&mut OpenImage, String> {
        self.images.get_mut(&handle).ok_or_else(|| format!("no open image with handle {handle}"))
    }

    pub fn info(&mut self, handle: u64) -> Result<FacetImageInfo, String> {
        Ok(self.get(handle)?.info())
    }

    /// Apply one edit-list change; answers the new info.
    pub fn edit(&mut self, handle: u64, edit: &FacetEdit) -> Result<FacetImageInfo, String> {
        let img = self.get(handle)?;
        let (w, h) = (img.original.width, img.original.height);
        let changed = match *edit {
            FacetEdit::Crop { x, y, width, height } => img.edits.push(Op::Crop { x, y, width, height }, w, h).map(|_| true)?,
            FacetEdit::Rotate { quarter_turns } => img.edits.push(Op::Rotate { quarter_turns: quarter_turns % 4 }, w, h).map(|_| true)?,
            FacetEdit::Flip { horizontal } => img.edits.push(Op::Flip { horizontal }, w, h).map(|_| true)?,
            FacetEdit::Resize { width, height } => img.edits.push(Op::Resize { width, height }, w, h).map(|_| true)?,
            FacetEdit::Adjust { brightness, contrast } => {
                img.edits.push(Op::Adjust { brightness, contrast }, w, h).map(|_| true)?
            }
            FacetEdit::Undo => img.edits.undo(),
            FacetEdit::Redo => img.edits.redo(),
            FacetEdit::Reset => img.edits.reset(),
        };
        if changed {
            img.baked = None;
        }
        Ok(img.info())
    }

    /// The edited picture (the list baked over the oriented original).
    pub fn baked(&mut self, handle: u64) -> Result<Raster, String> {
        Ok(self.get(handle)?.baked().clone())
    }

    /// Render the edited picture through `view` into a `width x height` viewport.
    pub fn render(&mut self, handle: u64, width: u32, height: u32, view: &FacetView) -> Result<Raster, String> {
        if width == 0 || height == 0 || width > MAX_VIEWPORT || height > MAX_VIEWPORT {
            return Err(format!("viewport {width}x{height} outside 1..={MAX_VIEWPORT}"));
        }
        let img = self.get(handle)?;
        Ok(view::render(img.baked(), view, width, height, view::FIELD))
    }

    /// Bake and write the picture to `path`. Answers the bytes written.
    pub fn export(&mut self, handle: u64, path: &str, format: FacetFormat, overwrite: bool) -> Result<u64, String> {
        if format == FacetFormat::Jpeg {
            return Err("JPEG export is owed: UnaOS has no JPEG encoder yet (PNG only)".into());
        }
        let p = Path::new(path);
        if p.exists() && !overwrite {
            return Err(format!("{path}: exists (set overwrite to replace it)"));
        }
        let img = self.get(handle)?;
        let baked = img.baked();
        let bytes = png::encode(baked.width, baked.height, &baked.rgba);
        // Atomic: a sibling temp file, then rename over the target.
        let tmp = p.with_file_name(format!(
            ".{}.facet-tmp",
            p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "export".into())
        ));
        std::fs::write(&tmp, &bytes).map_err(|e| format!("{path}: {e}"))?;
        std::fs::rename(&tmp, p).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{path}: {e}")
        })?;
        Ok(bytes.len() as u64)
    }

    /// Release a handle.
    pub fn close(&mut self, handle: u64) {
        self.images.remove(&handle);
    }

    /// Serve one bus message: the answers to publish (empty for anything not a Facet request).
    /// The pure seam the async loop runs and the tests drive.
    pub fn dispatch(&mut self, cmd: &FacetCommand) -> Vec<FacetCommand> {
        let err = |receipt_id: u64, handle: Option<u64>, message: String| FacetCommand::ImageError { receipt_id, handle, message };
        let reply = match cmd {
            FacetCommand::ImageOpen { receipt_id, principal, path } => {
                log::info!("[FACET] :: open {path} for {principal:?}");
                match self.open(path) {
                    Ok((handle, info)) => FacetCommand::ImageOpened { receipt_id: *receipt_id, handle, info },
                    Err(e) => err(*receipt_id, None, e),
                }
            }
            FacetCommand::ImageInfo { receipt_id, handle } => match self.info(*handle) {
                Ok(info) => FacetCommand::ImageInfoIs { receipt_id: *receipt_id, handle: *handle, info },
                Err(e) => err(*receipt_id, Some(*handle), e),
            },
            FacetCommand::ImageEdit { receipt_id, handle, edit } => match self.edit(*handle, edit) {
                Ok(info) => FacetCommand::ImageInfoIs { receipt_id: *receipt_id, handle: *handle, info },
                Err(e) => err(*receipt_id, Some(*handle), e),
            },
            FacetCommand::ImageRender { receipt_id, handle, width, height, view } => {
                match self.render(*handle, *width, *height, view) {
                    Ok(r) => FacetCommand::ImageRendered {
                        receipt_id: *receipt_id,
                        handle: *handle,
                        width: r.width,
                        height: r.height,
                        rgba: r.rgba,
                    },
                    Err(e) => err(*receipt_id, Some(*handle), e),
                }
            }
            FacetCommand::ImageExport { receipt_id, handle, path, format, overwrite } => {
                match self.export(*handle, path, *format, *overwrite) {
                    Ok(bytes) => FacetCommand::ImageExported { receipt_id: *receipt_id, handle: *handle, path: path.clone(), bytes },
                    Err(e) => err(*receipt_id, Some(*handle), e),
                }
            }
            FacetCommand::ImageClose { handle } => {
                self.close(*handle);
                return Vec::new();
            }
            // Answers (ours or anyone's) are inert as input.
            _ => return Vec::new(),
        };
        vec![reply]
    }
}

/// Subscribe and serve Facet on `synapse` until the bus closes.
pub async fn ignite(synapse: Synapse) {
    let rx = synapse.subscribe();
    serve(synapse, rx, Facet::new()).await
}

/// The serving loop against a receiver the caller already holds — subscribe first, spawn second,
/// and no request fired in between is missed.
pub async fn serve(synapse: Synapse, mut rx: tokio::sync::broadcast::Receiver<SMessage>, mut facet: Facet) {
    log::info!("[FACET] :: The Canvas is live (source: {}).", facet.source_name());
    loop {
        match rx.recv().await {
            Ok(SMessage::Facet(cmd)) if cmd.is_request() => {
                for reply in facet.dispatch(&cmd) {
                    synapse.fire(SMessage::Facet(reply));
                }
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                // A dropped request is a caller waiting forever on its receipt; say so.
                log::warn!("[FACET] :: lagged {n} messages behind the Synapse");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                log::info!("[FACET] :: Synapse closed; The Canvas terminating.");
                break;
            }
        }
    }
}

/// How far apart two same-size pictures are (RGB channels; alpha compared too).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Diff {
    /// Largest absolute difference of any channel of any pixel.
    pub max_delta: u8,
    /// PSNR over all RGBA samples, dB (`f64::INFINITY` when identical).
    pub psnr_db: f64,
    /// Pixels with any channel different.
    pub differing_px: u64,
}

/// Compare two same-size rasters (the eyes' and the oracle tests' metric).
pub fn compare(a: &Raster, b: &Raster) -> Diff {
    assert_eq!((a.width, a.height), (b.width, b.height), "compare needs equal sizes");
    let (mut max, mut se, mut px) = (0u8, 0f64, 0u64);
    for (p, q) in a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)) {
        let mut any = false;
        for c in 0..4 {
            let d = p[c].abs_diff(q[c]);
            max = max.max(d);
            se += (d as f64) * (d as f64);
            any |= d != 0;
        }
        px += any as u64;
    }
    let mse = se / a.rgba.len().max(1) as f64;
    let psnr = if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
    Diff { max_delta: max, psnr_db: psnr, differing_px: px }
}
