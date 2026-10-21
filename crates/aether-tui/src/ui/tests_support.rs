#![cfg(test)]

use ratatui::Terminal;
use ratatui::backend::TestBackend;

pub(crate) fn screen_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A daemon status with nothing happening, for tests that only care
/// about one of its fields.
pub(crate) fn quiet_status() -> aether_protocol::DaemonStatus {
    aether_protocol::DaemonStatus {
        problems: Vec::new(),
        approval_default: Default::default(),
        network_default: Default::default(),
        max_concurrent_turns: None,
        max_concurrent_engine_turns: None,
        engine_turns_running: 0,
        run_budget_usd: None,
        instance_id: "test".into(),
        version: "0.1.0".into(),
        pid: 1,
        started_at: aether_core::time::now(),
        last_seq: 0,
        agents_live: 1,
        agents_running: 1,
        agents_waiting: 0,
        agents_blocked: 0,
        agents_suspended: 0,
        agents_paused: 0,
        tasks_active: 0,
        pending_approvals: 0,
        default_model: None,
        usage_total: aether_core::model::Usage::default(),
        yolo: false,
        engines: Vec::new(),
        providers: Vec::new(),
        resource_leases: Vec::new(),
        runs: Vec::new(),
        mcp: Vec::new(),
        low_disk: None,
        bridge: None,
    }
}
