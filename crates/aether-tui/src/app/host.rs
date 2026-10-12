//! Host services keep remote paths out of the client's filesystem.
use super::{App, ClientError, Command, Effect, Reply, Then};
use aether_protocol::remote::{PathCompletionKind, Workspace};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRequest {
    Workspaces,
    Directory(String),
    Files { workspace: PathBuf, prefix: String },
    Validate,
    Git(PathBuf),
}

#[derive(Debug, Default)]
pub struct HostState {
    pub workspaces: Vec<Workspace>,
    pub directory: Option<(String, Vec<String>)>,
    pub files: Option<(PathBuf, String, Vec<String>)>,
    files_requested: Option<(PathBuf, String)>,
    git_at: u64,
    git_path: Option<PathBuf>,
}

pub fn request(command: Command, context: HostRequest) -> Effect {
    Effect::Request {
        command,
        then: Then::Host(context),
    }
}

impl App {
    pub(crate) fn host_effects(&mut self) -> Vec<Effect> {
        if !self.options.is_remote() || !self.connected {
            return Vec::new();
        }
        let mut effects = Vec::new();
        let Some(workspace) = self.active_workspace().map(std::path::Path::to_path_buf) else {
            return effects;
        };
        if self.host.git_path.as_ref() != Some(&workspace)
            || self.tick.saturating_sub(self.host.git_at) >= super::STATUS_REFRESH_TICKS
        {
            self.host.git_at = self.tick;
            self.host.git_path = Some(workspace.clone());
            effects.push(request(
                Command::GetGitSummary {
                    workspace: workspace.clone(),
                },
                HostRequest::Git(workspace.clone()),
            ));
        }
        if let Some(popup) = &self.bottom.popup
            && matches!(popup.kind, crate::ui::bottom::popup::PopupKind::Mention(_))
        {
            let prefix = self.input.text[..self.input.cursor]
                .rsplit('@')
                .next()
                .unwrap_or("")
                .to_owned();
            if self.host.files_requested.as_ref() != Some(&(workspace.clone(), prefix.clone())) {
                self.host.files_requested = Some((workspace.clone(), prefix.clone()));
                effects.push(request(
                    Command::CompletePath {
                        kind: PathCompletionKind::WorkspaceFile,
                        prefix: prefix.clone(),
                        workspace: Some(workspace.clone()),
                        limit: Some(200),
                    },
                    HostRequest::Files { workspace, prefix },
                ));
            }
        } else {
            self.host.files_requested = None;
        }
        effects
    }

    pub(super) fn host_reply(
        &mut self,
        context: HostRequest,
        result: Result<Reply, ClientError>,
    ) -> Vec<Effect> {
        let reply = match result {
            Ok(reply) => reply,
            Err(ClientError::Protocol(error))
                if matches!(
                    error.code,
                    aether_protocol::ErrorCode::Unimplemented
                        | aether_protocol::ErrorCode::Unavailable
                ) =>
            {
                self.flash_error(format!("host service unavailable: {}; update Aether on the host or choose an existing workspace", error.message));
                return Vec::new();
            }
            Err(error) => {
                self.flash_error(format!("host service failed: {error}"));
                return Vec::new();
            }
        };
        self.apply_host_reply(context, reply)
    }

    fn apply_host_reply(&mut self, context: HostRequest, reply: Reply) -> Vec<Effect> {
        match (context, reply) {
            (HostRequest::Workspaces, Reply::Workspaces(workspaces)) => {
                self.apply_workspaces(workspaces);
            }
            (HostRequest::Directory(prefix), Reply::PathCompletions(completions)) => {
                self.apply_directories(prefix, completions);
            }
            (HostRequest::Files { workspace, prefix }, Reply::PathCompletions(completions)) => {
                self.apply_file_completions(workspace, prefix, completions);
            }
            (HostRequest::Git(path), Reply::GitSummary(git)) => {
                self.branches.insert(
                    path,
                    git.branch.map(|branch| {
                        if git.dirty {
                            format!("{branch} *")
                        } else {
                            branch
                        }
                    }),
                );
            }
            (HostRequest::Validate, Reply::WorkspaceValidation(validation)) => {
                if validation.valid
                    && let Some(path) = validation.path.filter(|path| path.is_absolute())
                {
                    return self.use_workspace(&path);
                }
                self.flash_error(
                    validation
                        .reason
                        .unwrap_or_else(|| "host returned no valid absolute workspace".into()),
                );
            }
            (context, reply) => self.flash_error(format!(
                "unexpected host service reply for {context:?}: {reply:?}"
            )),
        }
        self.dirty = true;
        Vec::new()
    }

    fn apply_workspaces(&mut self, workspaces: Vec<Workspace>) {
        if workspaces
            .iter()
            .any(|workspace| !workspace.path.is_absolute())
        {
            self.flash_error("host returned a relative workspace path");
            return;
        }
        for workspace in &workspaces {
            if let Some(git) = &workspace.git {
                self.branches
                    .insert(workspace.path.clone(), git.branch.clone());
            }
        }
        self.host.workspaces = workspaces;
    }

    fn apply_directories(
        &mut self,
        prefix: String,
        completions: aether_protocol::remote::PathCompletions,
    ) {
        if completions.entries.len() > 200
            || completions
                .entries
                .iter()
                .any(|entry| !entry.is_dir || !PathBuf::from(&entry.path).is_absolute())
        {
            self.flash_error("host returned invalid directory completions");
            return;
        }
        self.host.directory = Some((
            prefix,
            completions
                .entries
                .into_iter()
                .map(|entry| entry.path)
                .collect(),
        ));
    }

    fn apply_file_completions(
        &mut self,
        workspace: PathBuf,
        prefix: String,
        completions: aether_protocol::remote::PathCompletions,
    ) {
        if self.host.files_requested.as_ref() != Some(&(workspace.clone(), prefix.clone())) {
            return;
        }
        let files: Vec<_> = completions
            .entries
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        if files.len() > 200
            || files.iter().any(|file| {
                PathBuf::from(file).is_absolute()
                    || PathBuf::from(file)
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
            })
        {
            self.flash_error("host returned invalid workspace file completions");
            return;
        }
        self.host.files = Some((workspace, prefix, files.clone()));
        if let Some(popup) = &mut self.bottom.popup {
            popup.files = files;
            self.sync_popup();
        }
    }
}
