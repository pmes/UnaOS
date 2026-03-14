// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use std::path::PathBuf;
use bandy::state::DashboardState;

#[derive(Debug)]
pub enum Event {
    Input {
        target: String,
        text: String,
    },
    TemplateAction(usize),
    NavSelect(usize),
    DockAction(usize),
    UploadRequest,
    FileSelected(PathBuf),
    ToggleSidebar,
    LoadHistory,
    MatrixFileClick(PathBuf),
    AuleIgnite,
    Timer,
    CreateNode {
        model: String,
        history: bool,
        temperature: f64,
        system_prompt: String,
    },
    NodeAction {
        action: String,
        active: bool,
    },
    ComplexInput {
        target: String,
        subject: String,
        body: String,
        point_break: bool,
        action: String,
    },
    ShardSelect(String),
    DispatchPayload(String),
}

pub trait AppHandler: 'static {
    fn handle_event(&mut self, event: Event);
    fn view(&self) -> DashboardState;
}
