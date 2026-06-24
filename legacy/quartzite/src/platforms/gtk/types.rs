// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use bandy::state::{HistoryItem, PreFlightPayload, WolfpackState, ShardStatus};

#[derive(Debug, Clone)]
pub enum GuiUpdate {
    HistorySeed(Vec<HistoryItem>),
    HistoryAppend(Vec<HistoryItem>),
    ShardStatusChanged { id: String, status: ShardStatus },
    ConsoleLogBatch(Vec<String>),
    ClearConsole,
    AppendInput(String),
    EditorLoad(String),
    SidebarStatus(WolfpackState), // The Pulse
    Spectrum(Vec<f32>),
    TokenUsage(i32, i32, i32), // Prompt, Candidates, Total
    ActiveDirective(String),
    ReviewPayload(PreFlightPayload), // The Interceptor
    SynapseError(String),            // Discrete failure signal
    ContextTelemetry(Vec<bandy::WeightedSkeleton>),
    RefreshMatrix(Vec<(String, String, usize)>),
    IngestMatrixTopology(Vec<String>), // Extracted dictionary paths for UI
    NetworkLog(String),
    NetworkState(String),
}
