use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::{
    model::Effort,
    task::Priority,
    time::Timestamp,
    tool::{SideEffectClass, ToolExposure},
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    #[default]
    Denied,
    Allowed,
}

/// Where an agent's tools actually run.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum ExecutionBackend {
    /// The daemon's own machine, inside the scoped roots.
    #[default]
    Host,
    /// A container per call, with the workspace mounted writable at the same
    /// path, everything else in scope mounted read-only, and network per the
    /// scope's policy.
    Container { image: String },
}

impl ExecutionBackend {
    /// A child may move from the host into a container, never out of one, and
    /// never to a different image: a container is the narrower place to run.
    pub fn is_narrowing_of(&self, parent: &Self) -> bool {
        match (self, parent) {
            (_, Self::Host) => true,
            (Self::Container { image }, Self::Container { image: parent }) => image == parent,
            (Self::Host, Self::Container { .. }) => false,
        }
    }
}

impl fmt::Display for ExecutionBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host => f.write_str("host"),
            Self::Container { image } => write!(f, "container:{image}"),
        }
    }
}

impl std::str::FromStr for ExecutionBackend {
    type Err = String;

    /// `host`, or `container:<image>`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim() {
            "host" => Ok(Self::Host),
            other => match other.split_once(':') {
                Some(("container", image)) if !image.trim().is_empty() => Ok(Self::Container {
                    image: image.trim().to_owned(),
                }),
                _ => Err(format!(
                    "`{other}` is not a backend; use `host` or `container:<image>`"
                )),
            },
        }
    }
}

/// A change to an agent's wall-clock stop (§3.1). Every knob has to be
/// changeable on a live agent, and one that can only ever move further into
/// the future is not: removing the deadline needs a form of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeadlineUpdate {
    Set(crate::time::Timestamp),
    Clear,
}

impl DeadlineUpdate {
    pub fn applied(self) -> Option<crate::time::Timestamp> {
        match self {
            Self::Set(at) => Some(at),
            Self::Clear => None,
        }
    }
}

/// Execution scope: where an agent may act. A first-class runtime attribute,
/// not prose in a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Scope {
    /// Root directory tools operate in. Relative tool paths resolve here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,
    /// Logical repository identifier (a URL or a name) for memory scoping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Additional roots the agent may touch besides the workspace, and the
    /// paths it owns: with any set, Aether's built-in shell refuses git commands
    /// that would discard working-tree changes outside them.
    #[serde(default)]
    pub allowed_paths: Vec<PathBuf>,
    #[serde(default)]
    pub network: NetworkPolicy,
    /// Where the agent's tools run: the daemon's machine, or a container.
    #[serde(default)]
    pub backend: ExecutionBackend,
    /// Free-form labels an orchestrator may use for limits and routing.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

impl Scope {
    /// A child scope can only narrow its parent: same or nested workspace,
    /// subset of allowed paths, network no more permissive.
    pub fn is_narrowing_of(&self, parent: &Scope) -> bool {
        self.widened_field(parent).is_none()
    }

    /// The first knob on which this scope sits outside `parent`, named so a
    /// refusal can say which one. `None` when it narrows `parent`.
    pub fn widened_field(&self, parent: &Scope) -> Option<&'static str> {
        let root_ok = match (&self.workspace_root, &parent.workspace_root) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(child), Some(parent_root)) => child.starts_with(parent_root),
        };
        if !root_ok {
            return Some("workspace_root");
        }
        let paths_ok = self.allowed_paths.iter().all(|p| {
            parent.allowed_paths.iter().any(|pp| p.starts_with(pp))
                || parent
                    .workspace_root
                    .as_ref()
                    .is_some_and(|r| p.starts_with(r))
        });
        if !paths_ok {
            return Some("allowed_paths");
        }
        // Memory is keyed by repository, so naming a different one is not a
        // narrower place to work - it is a different set of memories, which
        // the parent's siblings cannot see and which cannot see theirs.
        // `None` inherits, and is how a child normally arrives.
        if let (Some(child), Some(parent)) = (&self.repository, &parent.repository)
            && child != parent
        {
            return Some("repository");
        }
        if self.network == NetworkPolicy::Allowed && parent.network == NetworkPolicy::Denied {
            return Some("network");
        }
        if !self.backend.is_narrowing_of(&parent.backend) {
            return Some("backend");
        }
        None
    }
}

/// Which side-effect classes need explicit approval before execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApprovalPolicy {
    #[serde(default)]
    pub require_for: BTreeSet<SideEffectClass>,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            require_for: BTreeSet::from([SideEffectClass::External]),
        }
    }
}

impl ApprovalPolicy {
    pub fn requires(&self, class: SideEffectClass) -> bool {
        self.require_for.contains(&class)
    }

    /// A call declared `declared` but gated as the higher `effective` keeps the
    /// gate of either, so raising a class never drops an approval.
    pub fn requires_raised(&self, declared: SideEffectClass, effective: SideEffectClass) -> bool {
        assert!(effective >= declared, "a gate class may only be raised");
        self.requires(declared) || self.requires(effective)
    }

    pub fn none() -> Self {
        Self {
            require_for: BTreeSet::new(),
        }
    }

    /// Nothing is gated: every tool call runs without asking.
    pub fn is_yolo(&self) -> bool {
        self.require_for.is_empty()
    }

    /// Whether this policy is at least as strict as `parent`'s. A child may
    /// gate more classes than its parent, never fewer: a yolo parent may hand
    /// out a gated child, a gated parent may not hand out a yolo one (§3.2).
    pub fn is_narrowing_of(&self, parent: &ApprovalPolicy) -> bool {
        parent.require_for.is_subset(&self.require_for)
    }
}

/// What approval policy an agent gets when nothing else decides one. The
/// shipped default is `Yolo`: the operator asked for auto-approve out of the
/// box, and the execution backend - not the prompt - is the isolation
/// boundary.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDefault {
    /// Auto-approve: nothing asks first.
    #[default]
    Yolo,
    /// The built-in policy, which gates external side effects.
    Ask,
}

impl ApprovalDefault {
    pub fn policy(self) -> ApprovalPolicy {
        match self {
            ApprovalDefault::Yolo => ApprovalPolicy::none(),
            ApprovalDefault::Ask => ApprovalPolicy::default(),
        }
    }
}

impl fmt::Display for ApprovalDefault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ApprovalDefault::Yolo => "yolo",
            ApprovalDefault::Ask => "ask",
        })
    }
}

/// Engine host setup a session loads. Lean drops Claude Code's native skills
/// and non-Aether MCP servers; other engines ignore it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum HostContext {
    #[default]
    Full,
    Lean,
}

impl HostContext {
    pub fn is_full(&self) -> bool {
        *self == HostContext::Full
    }

    pub fn is_lean(&self) -> bool {
        *self == HostContext::Lean
    }
}

/// Per-agent configuration the runtime enforces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentConfig {
    /// Role- or task-specific instructions layered under the harness policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// Hard stop on turns; `None` means unbounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Total token budget across the agent and its subtree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    /// Total spend in US dollars across the agent and its subtree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget: Option<f64>,
    /// Wall-clock instant after which the agent stops (§3.1). Absolute, so
    /// an orchestrator extends it by setting a later one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<Timestamp>,
    #[serde(default)]
    pub tools: ToolExposure,
    #[serde(default)]
    pub approval: ApprovalPolicy,
    #[serde(default)]
    pub priority: Priority,
    /// Stay alive after the current task completes and wait for more work
    /// (a chat root or a long-lived steward). Delegated workers default to false.
    #[serde(default)]
    pub long_lived: bool,
    /// A leaf cannot create children and receives fewer coordination instructions and tools.
    #[serde(default)]
    pub leaf: bool,
    /// At most this many of the agent's descendants, at any depth, run at
    /// once; the rest queue. Never counts the agent itself. `None` (the
    /// shipped default, matching what an omitted field deserializes to)
    /// leaves the whole subtree bounded only by the daemon's global limit -
    /// a nonzero default here would be a WHOLE-SUBTREE cap, not a per-level
    /// one, and would starve every grandchild once it filled.
    /// Set explicitly, it bounds a supervisor's entire subtree,
    /// so pick it for the fan-out you actually want below that supervisor,
    /// not per level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
    /// Engine-native host setup the session loads; omitted on the wire when full.
    #[serde(default, skip_serializing_if = "HostContext::is_full")]
    pub host_context: HostContext,
    /// Build cache directory a client accounts for itself, used instead of
    /// Aether's own per-repository partition. Children inherit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_cache: Option<PathBuf>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            instructions: None,
            effort: None,
            max_turns: None,
            max_output_tokens: None,
            token_budget: None,
            cost_budget: None,
            deadline: None,
            tools: ToolExposure::All,
            approval: ApprovalPolicy::default(),
            priority: Priority::Normal,
            long_lived: false,
            leaf: false,
            concurrency: None,
            host_context: HostContext::Full,
            build_cache: None,
        }
    }
}
