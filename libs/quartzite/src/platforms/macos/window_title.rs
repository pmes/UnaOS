// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The browser window's title bar: page `<title>` and favicon.
//!
//! The engine owns both facts and publishes them over bandy
//! ([`SMessage::BrowserTitleChanged`] / [`SMessage::BrowserFaviconChanged`]),
//! deduped at its post-navigation choke point. This module is the AppKit end:
//! one subscription that writes the window's `title` and its **document icon**
//! (the proxy icon at the left of the title bar).
//!
//! # The document-icon route
//!
//! `standardWindowButton(NSWindowDocumentIconButton)` returns `nil` on a window
//! with no represented URL — AppKit only materializes the proxy-icon button
//! once the window claims to represent *something*. So the binding sets a
//! placeholder `representedURL` first (a `file:` URL that is never read; it
//! exists purely to bring the button into being) and then overrides that
//! button's `image` with the favicon.
//!
//! AppKit re-derives the proxy icon whenever the represented URL or the title
//! changes, so the last favicon is retained here (as scaled RGBA) and
//! re-applied after every title write — otherwise a `document.title` update
//! would silently blank the icon.
//!
//! Following [`super::image_view`], `NSImage` / `NSURL` / the window-button
//! enum are reached through `class!` + `msg_send!` rather than typed bindings,
//! so quartzite's `Cargo.toml` stays zero-diff.

use std::sync::{Arc, Mutex};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send, ClassType, Message};
use objc2_app_kit::{NSBitmapImageRep, NSWindow};
use objc2_foundation::{MainThreadMarker, NSSize, NSString};

use bandy::{SMessage, Synapse};

/// `NSWindowButton.documentIconButton`. Reached numerically (see module docs).
const DOCUMENT_ICON_BUTTON: isize = 4;

/// Favicon raster size, in pixels. The `NSImage` is then sized to
/// [`ICON_POINTS`] points, so AppKit treats the bitmap as a 2x representation
/// and the icon stays crisp on a Retina panel.
const ICON_PIXELS: u32 = 32;
/// Point size of the title-bar icon (the AppKit proxy-icon convention).
const ICON_POINTS: f64 = 16.0;

/// The bare window title: what the bar reads with no document loaded, and the
/// fallback for a page that declares no `<title>`.
const BARE_TITLE: &str = "Aether Browser";

/// The window title for a page whose `<title>` is `page_title`.
///
/// `"<title> — Aether"`, or the bare [`BARE_TITLE`] when the page declares
/// nothing (the engine's own no-title default is that same string, so it maps
/// to the bare form rather than to `"Aether Browser — Aether"`). Titles are
/// whitespace-collapsed — real pages ship `<title>` blocks split across lines,
/// and a raw newline in an `NSWindow` title renders as a glyph — and capped, so
/// a hostile page cannot push the traffic lights off the bar.
pub fn window_title_for(page_title: &str) -> String {
    let collapsed: String = page_title.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() || collapsed == BARE_TITLE {
        return BARE_TITLE.to_string();
    }
    const MAX: usize = 120;
    let trimmed = if collapsed.chars().count() > MAX {
        let head: String = collapsed.chars().take(MAX).collect();
        format!("{head}…")
    } else {
        collapsed
    };
    format!("{trimmed} — Aether")
}

/// Nearest-neighbour resample of tightly packed RGBA to `out * out`.
///
/// Nearest is deliberate: a favicon is already a 16/32px glyph, and at this
/// size a box filter only muddies the hard edges it is made of.
fn scale_nearest(rgba: &[u8], w: u32, h: u32, out: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || out == 0 || rgba.len() < (w as usize * h as usize * 4) {
        return None;
    }
    let mut dst = vec![0u8; out as usize * out as usize * 4];
    for y in 0..out {
        let sy = (y as u64 * h as u64 / out as u64).min(h as u64 - 1) as usize;
        for x in 0..out {
            let sx = (x as u64 * w as u64 / out as u64).min(w as u64 - 1) as usize;
            let si = (sy * w as usize + sx) * 4;
            let di = (y as usize * out as usize + x as usize) * 4;
            dst[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
        }
    }
    Some(dst)
}

/// Wrap tightly packed 8-bit sRGB RGBA in an AppKit bitmap rep (same idiom as
/// [`super::image_view::FacetImageView::set_frame`]). Main thread only.
fn bitmap_rep(rgba: &[u8], size: u32) -> Option<Retained<NSBitmapImageRep>> {
    let expected = size as usize * size as usize * 4;
    if rgba.len() < expected {
        return None;
    }
    let rep: Retained<NSBitmapImageRep> = unsafe {
        let alloc: Allocated<NSBitmapImageRep> = msg_send![NSBitmapImageRep::class(), alloc];
        let planes: *mut *mut u8 = std::ptr::null_mut();
        let rep: Option<Retained<NSBitmapImageRep>> = msg_send![
            alloc,
            initWithBitmapDataPlanes: planes,
            pixelsWide: size as isize,
            pixelsHigh: size as isize,
            bitsPerSample: 8isize,
            samplesPerPixel: 4isize,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: &*NSString::from_str("NSDeviceRGBColorSpace"),
            bytesPerRow: (size as isize) * 4,
            bitsPerPixel: 32isize,
        ];
        rep?
    };
    let data: *mut u8 = unsafe { msg_send![&rep, bitmapData] };
    if data.is_null() {
        return None;
    }
    unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), data, expected) };
    Some(rep)
}

/// Build the title-bar `NSImage` from already-scaled `ICON_PIXELS` RGBA.
/// Main thread only.
fn favicon_image(scaled: &[u8]) -> Option<Retained<AnyObject>> {
    let rep = bitmap_rep(scaled, ICON_PIXELS)?;
    unsafe {
        let img: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let img: *mut AnyObject =
            msg_send![img, initWithSize: NSSize::new(ICON_POINTS, ICON_POINTS)];
        let img = Retained::from_raw(img)?;
        let _: () = msg_send![&*img, addRepresentation: &*rep];
        // The rep is 32x32 px inside a 16x16 pt image: AppKit reads that as a
        // 2x representation, which is exactly what a Retina title bar wants.
        let _: () = msg_send![&*img, setSize: NSSize::new(ICON_POINTS, ICON_POINTS)];
        Some(img)
    }
}

/// Install the favicon on the window's proxy-icon button. Main thread only.
/// Returns false when AppKit declines to give us the button.
fn apply_icon(window: &NSWindow, scaled: &[u8]) -> bool {
    let Some(image) = favicon_image(scaled) else {
        return false;
    };
    unsafe {
        let button: *mut AnyObject =
            msg_send![window, standardWindowButton: DOCUMENT_ICON_BUTTON];
        if button.is_null() {
            return false;
        }
        let _: () = msg_send![button, setImage: &*image];
    }
    true
}

/// Give the window a placeholder `representedURL` so AppKit materializes the
/// document-icon button at all (see module docs). Main thread only.
fn ensure_document_icon_button(window: &NSWindow) {
    unsafe {
        let existing: *mut AnyObject = msg_send![window, representedURL];
        if !existing.is_null() {
            return;
        }
        // Never read — it exists only to bring the proxy-icon button into
        // being. `/` is used because it always resolves, so AppKit does not
        // fall back to a "missing file" badge before we override the image.
        let path = NSString::from_str("/");
        let url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: &*path];
        if url.is_null() {
            return;
        }
        let _: () = msg_send![window, setRepresentedURL: url];
    }
}

/// Subscribe the browser window's title bar to the engine.
///
/// Spawns the subscription on its own thread with a current-thread tokio
/// runtime and hops each update back to the main queue — the AppKit main
/// thread has no reactor of its own (the idiom
/// [`super::text_field::bootstrap_text_field`] established for the url bar).
pub fn bind_browser_title(window: &NSWindow, synapse: Synapse) {
    let mtm = MainThreadMarker::new().expect("bind_browser_title must run on the main thread");
    ensure_document_icon_button(window);

    let bound = Arc::new(dispatch2::MainThreadBound::new(window.retain(), mtm));
    // The favicon currently shown, scaled and ready to rebuild: AppKit
    // re-derives the proxy icon on every title write, so each title change
    // re-applies this.
    let icon: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let mut rx = synapse.subscribe();

    std::thread::Builder::new()
        .name("window-title".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("window title subscription runtime");
            rt.block_on(async move {
                loop {
                    match rx.recv().await {
                        Ok(SMessage::BrowserTitleChanged(page_title)) => {
                            let title = window_title_for(&page_title);
                            let bound = bound.clone();
                            let icon = icon.clone();
                            dispatch2::DispatchQueue::main().exec_async(move || {
                                let mtm = MainThreadMarker::new().unwrap();
                                let window = bound.get(mtm);
                                window.setTitle(&NSString::from_str(&title));
                                // Re-assert the icon: the title write reset it.
                                if let Ok(guard) = icon.lock() {
                                    if let Some(scaled) = guard.as_ref() {
                                        apply_icon(window, scaled);
                                    }
                                }
                            });
                        }
                        Ok(SMessage::BrowserFaviconChanged { width, height, rgba }) => {
                            let Some(scaled) = scale_nearest(&rgba, width, height, ICON_PIXELS)
                            else {
                                log::warn!(
                                    "[AETHER] favicon: unusable buffer ({} bytes for {width}x{height})",
                                    rgba.len()
                                );
                                continue;
                            };
                            if let Ok(mut guard) = icon.lock() {
                                *guard = Some(scaled.clone());
                            }
                            let bound = bound.clone();
                            dispatch2::DispatchQueue::main().exec_async(move || {
                                let mtm = MainThreadMarker::new().unwrap();
                                let window = bound.get(mtm);
                                ensure_document_icon_button(window);
                                if !apply_icon(window, &scaled) {
                                    // The title still lands; only the icon is
                                    // missing. Logged rather than retried —
                                    // AppKit will not hand over the button.
                                    log::warn!(
                                        "[AETHER] favicon: no document-icon button on this window"
                                    );
                                }
                            });
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        })
        .expect("spawn window-title thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_formatting() {
        assert_eq!(window_title_for("Example Domain"), "Example Domain — Aether");
        // No <title>: the engine's own default maps back to the bare form.
        assert_eq!(window_title_for("Aether Browser"), "Aether Browser");
        assert_eq!(window_title_for(""), "Aether Browser");
        assert_eq!(window_title_for("   \n\t "), "Aether Browser");
        // Real pages ship multi-line <title> blocks; a raw newline would draw
        // as a glyph in the bar.
        assert_eq!(window_title_for("Hello\n   world"), "Hello world — Aether");
        // Hostile length is capped, not passed through.
        let long = "x".repeat(500);
        let out = window_title_for(&long);
        assert!(out.chars().count() < 140, "uncapped title: {} chars", out.chars().count());
        assert!(out.ends_with("… — Aether"));
    }

    #[test]
    fn nearest_neighbour_scale() {
        // 2x2 -> 4x4: every source pixel lands in a 2x2 block, exactly.
        let src: Vec<u8> = vec![
            1, 1, 1, 255, 2, 2, 2, 255, //
            3, 3, 3, 255, 4, 4, 4, 255,
        ];
        let out = scale_nearest(&src, 2, 2, 4).expect("scales");
        assert_eq!(out.len(), 4 * 4 * 4);
        let px = |x: usize, y: usize| out[(y * 4 + x) * 4];
        assert_eq!((px(0, 0), px(1, 0), px(2, 0), px(3, 0)), (1, 1, 2, 2));
        assert_eq!((px(0, 3), px(3, 3)), (3, 4));
        // Downscale keeps the corners.
        let down = scale_nearest(&src, 2, 2, 1).expect("scales");
        assert_eq!(down[0], 1);
        // Buffers that cannot be what they claim are refused, not indexed.
        assert!(scale_nearest(&src, 8, 8, 4).is_none());
        assert!(scale_nearest(&src, 0, 2, 4).is_none());
        assert!(scale_nearest(&src, 2, 2, 0).is_none());
    }
}
