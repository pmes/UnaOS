// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use bandy::state::DashboardState;
use bandy::signals::SMessage;

pub trait AppHandler: 'static {
    fn handle_event(&mut self, event: SMessage);
    fn view(&self) -> DashboardState;
}
