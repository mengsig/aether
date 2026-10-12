//! Opening slash views: push the view and request the data it shows (the
//! refreshes `app/overlays.rs` does today). Owner: W7.

use super::{
    App, Artifact, ArtifactQuery, ArtifactsState, Command, Effect, Effort, ReportView, Then,
    eviction_notice,
};
use crate::ui::views::{self, ViewKind};

impl App {
    /// Push the view for `kind` onto the bottom pane and return its data requests.
    pub(crate) fn open_view(&mut self, kind: ViewKind) -> Vec<Effect> {
        if self.options.is_remote()
            && matches!(
                kind,
                ViewKind::Login | ViewKind::Doctor | ViewKind::Voice | ViewKind::VoiceInstall(_)
            )
        {
            self.flash_error("this feature is local-only; run aether --local on the host instead");
            return Vec::new();
        }
        let effects = self.view_requests(kind);
        let view: Box<dyn crate::ui::bottom::BottomView> = match kind {
            ViewKind::Workspace if self.options.is_remote() => {
                Box::new(views::remote_workspace::RemoteWorkspaceView::default())
            }
            ViewKind::Workspace => {
                let (view, missing) = views::workspace::WorkspaceView::discover(self);
                self.flag_missing_workspace_roots(&missing);
                Box::new(view)
            }
            _ => views::open(kind, self),
        };
        self.bottom.push_view(view);
        self.dirty = true;
        effects
    }

    fn view_requests(&mut self, kind: ViewKind) -> Vec<Effect> {
        match kind {
            ViewKind::Workspace if self.options.is_remote() => vec![super::host::request(
                Command::ListWorkspaces { limit: Some(200) },
                super::host::HostRequest::Workspaces,
            )],
            ViewKind::Model => vec![Effect::Request {
                command: Command::ListModels,
                then: Then::Models,
            }],
            // One refresh at a time: a second would race the first's reply.
            ViewKind::Usage if self.usage_loading => Vec::new(),
            ViewKind::Usage => vec![self.usage_request()],
            ViewKind::Approvals => vec![self.rules_request()],
            ViewKind::Memory => vec![self.memories_request()],
            ViewKind::Schedules => vec![self.schedules_request()],
            ViewKind::Mcp => vec![self.mcp_request(false)],
            ViewKind::Outbox => vec![Effect::Outbox { settle: None }],
            ViewKind::Report => self.report_requests(),
            ViewKind::Artifacts => self.artifact_list_requests(),
            ViewKind::Login => vec![Effect::Request {
                command: Command::AuthStatus,
                then: Then::Auth,
            }],
            ViewKind::ArtifactPager(id) => self.read_artifact(id),
            ViewKind::VoiceInstall(model) => self.voice_install(model),
            ViewKind::Modes
            | ViewKind::Doctor
            | ViewKind::Settings
            | ViewKind::Keys
            | ViewKind::Theme
            | ViewKind::Voice
            | ViewKind::Workspace => Vec::new(),
        }
    }

    /// Views cannot mutate `App`, so the loading state a request implies is set
    /// here as it leaves, the way the old overlays' request builders did.
    pub(crate) fn arm_view_request(&mut self, effect: &Effect) {
        let Effect::Request { command, then } = effect else {
            return;
        };
        match then {
            Then::McpServers => {
                if matches!(command, Command::McpServers { refresh: true, .. }) {
                    self.mcp_refreshing = true;
                }
            }
            Then::Schedules => {
                self.schedules.loading = true;
                self.schedules.refresh_pending = false;
                self.schedules.error = None;
            }
            Then::ScheduleHistory(_) => {
                self.schedules.history = None;
                self.schedules.history_scroll = 0;
            }
            Then::Usage => {
                self.usage_loading = true;
                self.usage_error = None;
            }
            Then::RunReport(run) => {
                self.report.run = Some(*run);
                self.report.loading = true;
                self.report.error = None;
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn report_requests(&mut self) -> Vec<Effect> {
        let run = self
            .selected
            .and_then(|agent| self.tree.nodes.get(&agent))
            .map(|node| node.agent.run);
        match run {
            Some(run) => vec![self.report_request(run)],
            None => {
                self.report = ReportView::default();
                Vec::new()
            }
        }
    }

    /// The viewed agent's artifacts. The reply lands in `artifacts.list`
    /// only while `artifacts.for_agent` names that agent, so it is armed here.
    fn artifact_list_requests(&mut self) -> Vec<Effect> {
        let Some(agent) = self.selected else {
            self.artifacts = ArtifactsState::default();
            return Vec::new();
        };
        self.artifacts = ArtifactsState {
            for_agent: Some(agent),
            ..Default::default()
        };
        vec![Effect::Request {
            command: Command::ListArtifacts {
                query: ArtifactQuery {
                    agent: Some(agent),
                    ..Default::default()
                },
            },
            then: Then::Artifacts(agent),
        }]
    }

    /// Effort the `/model` view starts on: the viewed agent's, else the one
    /// chosen for the next root, else the daemon default.
    pub(crate) fn effort_for_views(&self) -> Effort {
        self.selected_agent()
            .and_then(|agent| agent.config.effort)
            .or(self.default_effort)
            .unwrap_or(Effort::High)
    }

    /// Why an artifact's bytes cannot be shown, for the pager and the list.
    pub(crate) fn artifact_eviction_notice(&self, artifact: &Artifact) -> String {
        eviction_notice(artifact, self.now)
    }
}
