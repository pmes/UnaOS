// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

// libs/quartzite/src/platforms/macos/spline.rs
#![cfg(target_os = "macos")]

use crate::{Event, NativeView, NativeWindow};
use async_channel::{Receiver, Sender};
use bandy::state::AppState;
use std::sync::{Arc, RwLock};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSScrollView, NSSplitView, NSSplitViewDividerStyle, NSTextView,
    NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectView,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

pub struct MacOSSpline {}

impl MacOSSpline {
    pub fn new() -> Self {
        Self {}
    }

    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        _tx_event: Sender<Event>,
        rx_gui: Receiver<GuiUpdate>,
        _rx_telemetry: Receiver<bandy::SMessage>,
    ) -> NativeView {
        let mtm = MainThreadMarker::new().expect("Must be on main thread");

        // 1. Root Container (NSSplitView)
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1200.0, 800.0));
        let split_view = unsafe {
            let view: Retained<NSSplitView> =
                msg_send![mtm.alloc::<NSSplitView>(), initWithFrame: frame];
            view.setVertical(true);
            view.setDividerStyle(NSSplitViewDividerStyle::Thin);
            let mask = NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable;
            view.setAutoresizingMask(mask);
            view
        };

        // 2. The Navigator (Left Pane)
        let left_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 800.0));
        let left_pane = unsafe {
            let effect_view: Retained<NSVisualEffectView> =
                msg_send![mtm.alloc::<NSVisualEffectView>(), initWithFrame: left_frame];
            effect_view.setMaterial(NSVisualEffectMaterial::Sidebar);
            effect_view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);

            let mask = NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable;
            effect_view.setAutoresizingMask(mask);
            effect_view
        };

        // Add dummy text to sidebar for structure visualization
        let sidebar_text = unsafe {
            let text: Retained<NSTextView> =
                msg_send![mtm.alloc::<NSTextView>(), initWithFrame: left_frame];
            text.setEditable(false);
            text.setDrawsBackground(false);
            text.setString(&*NSString::from_str("Project Navigator"));
            let mask = NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable;
            text.setAutoresizingMask(mask);
            text
        };
        unsafe {
            left_pane.addSubview(&*sidebar_text);
        }

        // 3. The Workspace (Right Pane)
        let right_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(940.0, 800.0));
        let scroll_view = unsafe {
            let scroll: Retained<NSScrollView> =
                msg_send![mtm.alloc::<NSScrollView>(), initWithFrame: right_frame];
            scroll.setHasVerticalScroller(true);
            let mask = NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable;
            scroll.setAutoresizingMask(mask);
            scroll
        };

        let text_view = unsafe {
            let text: Retained<NSTextView> =
                msg_send![mtm.alloc::<NSTextView>(), initWithFrame: right_frame];
            text.setEditable(false);
            text.setRichText(false);
            text.setFont(Some(
                &objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(12.0, 400.0),
            ));
            let mask = NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable;
            text.setAutoresizingMask(mask);
            text
        };

        unsafe {
            scroll_view.setDocumentView(Some(&*text_view));

            // Add static message
            let hello = NSString::from_str(
                ">> Lumen/Mach Substrate Active.\n>> Waiting for Neural Link...",
            );
            text_view.setString(&*hello);
        }

        // Add panes to splitter
        unsafe {
            split_view.addSubview(&*left_pane);
            split_view.addSubview(&*scroll_view);
        }

        // Suppress unused warning for rx_gui until async implementation
        let _ = rx_gui;

        Retained::into_super(split_view)
    }
}
