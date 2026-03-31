// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSResponder, NSTextView, NSTextViewDelegate, NSTextDelegate, NSView
};
use objc2_foundation::{NSObjectProtocol};

// -----------------------------------------------------------------------------
// COMMS DELEGATE (LUMEN REACTOR CHAT)
// -----------------------------------------------------------------------------
struct CommsDelegateIvars {}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaCommsDelegate"]
    #[ivars = CommsDelegateIvars]
    pub struct CommsDelegate;

    unsafe impl CommsDelegate {
        #[unsafe(method(init))]
        pub fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(CommsDelegateIvars {});
            unsafe { msg_send![super(this), init] }
        }
    }

    // --- NSTextViewDelegate ---
    unsafe impl NSTextViewDelegate for CommsDelegate {
        #[unsafe(method(textView:doCommandBySelector:))]
        fn text_view_do_command_by_selector(
            &self,
            _text_view: &NSTextView,
            _command_selector: objc2::runtime::Sel,
        ) -> objc2::runtime::Bool {
            objc2::runtime::Bool::NO
        }
    }
);

unsafe impl NSObjectProtocol for CommsDelegate {}
unsafe impl NSTextDelegate for CommsDelegate {}
