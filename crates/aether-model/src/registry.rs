//! Provider and model definitions, built in plus user overrides.

use std::collections::BTreeMap;
use std::path::Path;

use aether_core::model::{CapabilityProfile, Effort, ModelCost, ModelRef};
use serde::{Deserialize, Serialize};

use crate::auth::AuthMethods;
use crate::profile::ModelFamily;
use crate::tier::{ModelTier, tier_by_price};

/// Declares the protocol list once: the enum, `ALL` and `as_str` are all
/// generated from it, so a new variant reaches every one of them or none.
/// A hand-written `ALL` could silently fall behind the enum, and the
/// conformance suite derives its coverage from `ALL`.
macro_rules! protocols {
    ($( $(#[$meta:meta])* $variant:ident => $name:literal ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub enum Protocol {
            $( $(#[$meta])* $variant, )+
        }

        impl Protocol {
            /// Every protocol, so a suite can iterate them.
            pub const ALL: &'static [Self] = &[ $( Self::$variant, )+ ];

            pub fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $name, )+ }
            }
        }
    };
}

protocols! {
    AnthropicMessages => "anthropic-messages",
    OpenaiChatCompletions => "openai-chat-completions",
    OpenaiResponses => "openai-responses",
    OpenaiCodexResponses => "openai-codex-responses",
    /// Inference runs through the Claude Code CLI (subscription login).
    ClaudeCode => "claude-code",
    /// Inference runs through the Codex CLI (ChatGPT login).
    Codex => "codex",
}

impl Protocol {
    /// Whether runtime accounting can enforce a dollar cap on this model.
    pub fn reports_cost(self, capabilities: &CapabilityProfile) -> bool {
        match self {
            Self::ClaudeCode => true,
            Self::Codex => false,
            _ => capabilities.cost.is_some(),
        }
    }

    /// Engines are driven by the runtime, not by an HTTP adapter.
    pub fn is_engine(self) -> bool {
        matches!(self, Self::ClaudeCode | Self::Codex)
    }

    /// The API provider whose price list covers this subscription route's
    /// models: `codex` bills ChatGPT for OpenAI's models, `claude-code` bills
    /// a Claude subscription for Anthropic's. `None` for a protocol that
    /// invoices its own usage, which needs no equivalent.
    pub fn api_equivalent_provider(self) -> Option<&'static str> {
        match self {
            Self::Codex => Some("openai"),
            Self::ClaudeCode => Some("anthropic"),
            _ => None,
        }
    }
}

/// Wire-level quirks that vary between servers speaking the same protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderFlags {
    /// Send `max_completion_tokens` instead of `max_tokens`.
    pub max_completion_tokens: bool,
    /// Send `stream_options.include_usage`; some local servers reject it.
    pub stream_options: bool,
    /// Any model id is accepted, not only the listed ones.
    pub open_model_list: bool,
}

impl Default for ProviderFlags {
    fn default() -> Self {
        Self {
            max_completion_tokens: false,
            stream_options: true,
            open_model_list: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelDef {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub capabilities: CapabilityProfile,
    /// Overrides the provider's family, for a provider that serves several
    /// (a router). `None` falls back to the provider's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<ModelFamily>,
    /// Effort a turn runs at when neither agent nor caller chose one. `None`
    /// leaves it to the provider (or native harness) default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<Effort>,
    /// Capability band within the provider; `None` leaves it to the price
    /// fallback in [`Registry::catalog_tier`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<ModelTier>,
}

impl ModelDef {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        capabilities: CapabilityProfile,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            aliases: Vec::new(),
            capabilities,
            family: None,
            default_effort: None,
            tier: None,
        }
    }

    pub fn with_tier(mut self, tier: ModelTier) -> Self {
        self.tier = Some(tier);
        self
    }

    /// Declare this model's family when it differs from its provider's.
    pub fn with_family(mut self, family: ModelFamily) -> Self {
        self.family = Some(family);
        self
    }

    pub fn with_aliases(mut self, aliases: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_default_effort(mut self, effort: Effort) -> Self {
        self.default_effort = Some(effort);
        self
    }

    /// The effort a turn on this model runs at: an explicit request always
    /// wins over the model's default.
    pub fn effort_for(&self, requested: Option<Effort>) -> Option<Effort> {
        requested.or(self.default_effort)
    }

    pub fn matches(&self, id: &str) -> bool {
        self.id == id || self.aliases.iter().any(|a| a == id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderDef {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub protocol: Protocol,
    pub base_url: String,
    #[serde(default)]
    pub auth: AuthMethods,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub models: Vec<ModelDef>,
    /// Profile for models not in the list, when the list is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_capabilities: Option<CapabilityProfile>,
    #[serde(default)]
    pub flags: ProviderFlags,
    /// Cap on model turns this provider carries at once. `None` leaves it to
    /// the daemon's global limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
    /// Model family every model here belongs to, choosing the authored model
    /// profile. `None` for a provider that serves anything (a local server, a
    /// router): no profile beats the wrong one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<ModelFamily>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl ProviderDef {
    pub fn model(&self, id: &str) -> Option<&ModelDef> {
        self.models.iter().find(|m| m.matches(id))
    }

    /// Resolve a model id, honoring aliases and open model lists.
    pub fn resolve_model(&self, id: &str) -> Option<ModelDef> {
        if let Some(m) = self.model(id) {
            return Some(m.clone());
        }
        if crate::retired::successor(&self.id, id).is_some() {
            return None;
        }
        if self.flags.open_model_list || self.models.is_empty() {
            return Some(ModelDef {
                id: id.to_owned(),
                name: id.to_owned(),
                aliases: Vec::new(),
                capabilities: self.default_capabilities.clone().unwrap_or_default(),
                // An unlisted model inherits the provider's family; a router
                // that serves several declares none, so it gets no profile.
                family: None,
                default_effort: None,
                tier: None,
            });
        }
        None
    }
}

/// What one model's tokens cost, and on whose authority: dollars its own list
/// price bills, or the rate the API model behind a subscription charges, which
/// can only ever be an estimate. One or the other - the type is what keeps a
/// price from being reported as both.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModelPrice {
    Billed(ModelCost),
    ApiEquivalent(ModelCost),
}

impl ModelPrice {
    pub fn rate(self) -> ModelCost {
        match self {
            Self::Billed(cost) | Self::ApiEquivalent(cost) => cost,
        }
    }
}

/// A rate a budget can carry. A negative one would pay the operator and a
/// non-finite one poisons every total it reaches; `providers.toml` can set
/// both, so nothing downstream may assume they are impossible.
fn is_payable(cost: ModelCost) -> bool {
    cost.rates().all(|rate| rate.is_finite() && rate >= 0.0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedModel {
    pub provider: ProviderDef,
    pub model: ModelDef,
}

impl ResolvedModel {
    /// The model's family: the model's own declaration, else its provider's.
    pub fn family(&self) -> Option<ModelFamily> {
        self.model.family.or(self.provider.family)
    }

    /// The authored profile text for this model, or `None` when no family is
    /// declared. Engines get the half that applies to them.
    pub fn profile_text(&self) -> Option<String> {
        Some(
            self.family()?
                .profile()
                .text(self.provider.protocol.is_engine()),
        )
    }

    /// Whether a turn on this model records a dollar cost. Engines report
    /// their own spend - Claude Code does, Codex does not - and an API model
    /// needs a price profile for the runtime to compute one.
    pub fn reports_cost(&self) -> bool {
        self.provider
            .protocol
            .reports_cost(&self.model.capabilities)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("unknown provider `{0}`")]
    UnknownProvider(String),
    #[error("provider `{0}` is disabled")]
    DisabledProvider(String),
    #[error("{}", unknown_model_message(.provider, .model))]
    UnknownModel { provider: String, model: String },
    #[error("{path}: {reason}")]
    Config { path: String, reason: String },
}

impl RegistryError {
    /// A retired built-in id, as opposed to one this registry never knew.
    pub fn is_retirement(&self) -> bool {
        matches!(self, Self::UnknownModel { provider, model }
            if crate::retired::successor(provider, model).is_some())
    }
}

fn unknown_model_message(provider: &str, model: &str) -> String {
    match crate::retired::successor(provider, model) {
        Some(successor) => format!("{provider}/{model} was retired; use {provider}/{successor}"),
        None => format!("unknown model `{model}` for provider `{provider}`"),
    }
}

/// Overrides for one provider from `providers.toml`. Every field optional;
/// a new provider needs at least `protocol` and `base_url`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProviderOverride {
    pub name: Option<String>,
    pub protocol: Option<Protocol>,
    pub base_url: Option<String>,
    pub auth: Option<AuthMethods>,
    pub headers: Option<BTreeMap<String, String>>,
    /// Replaces the model list.
    pub models: Option<Vec<ModelDef>>,
    /// Appends to (or updates within) the model list.
    pub add_models: Vec<AddedModel>,
    pub default_capabilities: Option<CapabilityProfile>,
    pub flags: Option<ProviderFlags>,
    /// Cap on concurrent turns; 0 clears it.
    pub max_concurrent: Option<u32>,
    /// Model family; `"none"` clears it.
    pub family: Option<FamilyOverride>,
    pub enabled: Option<bool>,
}

/// One `add_models` entry: a [`ModelDef`] that may start from the capabilities
/// of a model its provider already lists, stating only what differs.
#[derive(Debug, Clone, Deserialize)]
pub struct AddedModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Id or alias of the listed model whose capabilities this entry copies.
    #[serde(default)]
    pub capabilities_from: Option<String>,
    /// Complete on its own; with `capabilities_from`, only the changed fields.
    #[serde(default)]
    pub capabilities: Option<toml::Table>,
    #[serde(default)]
    pub family: Option<ModelFamily>,
    #[serde(default)]
    pub default_effort: Option<Effort>,
    #[serde(default)]
    pub tier: Option<ModelTier>,
}

/// A family override, or the word `none` to clear an inherited one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FamilyOverride {
    None,
    #[serde(untagged)]
    Family(ModelFamily),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProvidersFile {
    pub providers: BTreeMap<String, ProviderOverride>,
}

pub fn load_providers_file(path: &Path) -> Result<Option<ProvidersFile>, RegistryError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(RegistryError::Config {
                path: path.display().to_string(),
                reason: e.to_string(),
            });
        }
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| RegistryError::Config {
            path: path.display().to_string(),
            reason: providers_file_hint(e.to_string()),
        })
}

/// The mistake we have seen: model defaults written into providers.toml.
fn providers_file_hint(reason: String) -> String {
    if reason.contains("unknown field `model`") {
        format!("{reason}\n[model] settings belong in config.toml, not providers.toml")
    } else {
        reason
    }
}

#[derive(Debug, Clone, Default)]
pub struct Registry {
    providers: Vec<ProviderDef>,
}

impl Registry {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn builtin() -> Self {
        Self {
            providers: crate::builtin::providers(),
        }
    }

    pub fn providers(&self) -> &[ProviderDef] {
        &self.providers
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderDef> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Keep only the providers matching the predicate.
    pub fn retain(&mut self, keep: impl FnMut(&ProviderDef) -> bool) {
        self.providers.retain(keep);
    }

    /// Insert or replace a provider, keeping its position when replacing.
    pub fn insert(&mut self, def: ProviderDef) {
        match self.providers.iter_mut().find(|p| p.id == def.id) {
            Some(existing) => *existing = def,
            None => self.providers.push(def),
        }
    }

    pub fn resolve(&self, r: &ModelRef) -> Result<ResolvedModel, RegistryError> {
        let provider = self
            .provider(&r.provider)
            .ok_or_else(|| RegistryError::UnknownProvider(r.provider.clone()))?;
        if !provider.enabled {
            return Err(RegistryError::DisabledProvider(r.provider.clone()));
        }
        let model =
            provider
                .resolve_model(&r.model)
                .ok_or_else(|| RegistryError::UnknownModel {
                    provider: r.provider.clone(),
                    model: r.model.clone(),
                })?;
        Ok(ResolvedModel {
            provider: provider.clone(),
            model,
        })
    }

    /// The API model behind a subscription model, when this registry lists
    /// one: `codex/<slug>` → `openai/<slug>`, `claude-code/<alias>` →
    /// `anthropic/<model>`, by the slug or alias the two tables share.
    ///
    /// Only a model the API provider actually lists counts, never one an open
    /// model list would invent, and the provider need not be enabled: a list
    /// price is a fact about a model, not a route to it.
    pub fn api_equivalent(&self, subscription: &ResolvedModel) -> Option<&ModelDef> {
        self.api_twin(&subscription.provider, &subscription.model.id)
    }

    /// [`Self::api_equivalent`] by provider and model id, so a model that was
    /// never resolved into a pair - one an engine session named - can be
    /// looked up without cloning its provider.
    fn api_twin(&self, subscription: &ProviderDef, model_id: &str) -> Option<&ModelDef> {
        let api = subscription.protocol.api_equivalent_provider()?;
        debug_assert!(
            api != subscription.id,
            "a provider is not its own API equivalent: {api}"
        );
        self.provider(api)?.model(model_id)
    }

    /// The price to estimate an unpriced subscription invocation with: the
    /// list price of its API equivalent, as an estimate and never as billed
    /// dollars. `None` where the model carries its own price - then its cost
    /// is priced, not estimated - or where no equivalent is known.
    pub fn api_equivalent_price(&self, subscription: &ResolvedModel) -> Option<ModelCost> {
        if subscription.model.capabilities.cost.is_some() {
            return None;
        }
        self.api_equivalent(subscription)?.capabilities.cost
    }

    /// The capability band of a model: its declared tier, else its output
    /// price (own, or its API equivalent's) matched to the nearest tiered model
    /// of the same provider. `None` when neither says. Operator
    /// overrides sit above this, in `Models::tier`.
    pub fn catalog_tier(&self, provider: &str, model: &str) -> Option<ModelTier> {
        let provider = self.provider(provider)?;
        let def = provider.resolve_model(model)?;
        if def.tier.is_some() {
            return def.tier;
        }
        let output = self.output_price(provider, &def)?;
        let references: Vec<_> = provider
            .models
            .iter()
            .filter_map(|m| Some((self.output_price(provider, m)?, m.tier?)))
            .collect();
        tier_by_price(output, &references)
    }

    fn output_price(&self, provider: &ProviderDef, model: &ModelDef) -> Option<f64> {
        let cost = model
            .capabilities
            .cost
            .or_else(|| self.api_twin(provider, &model.id)?.capabilities.cost)?;
        Some(cost.output_per_mtok)
    }

    /// The price for the model an engine session reported it is really running.
    ///
    /// `codex/default` and `claude-code/default` carry no price and have no
    /// API twin - the CLI picks the model, so the configuration cannot name
    /// it - and the session is the first moment anything knows which model is
    /// answering. The reported id is looked up in the configured model's own
    /// provider, by id or alias, so a session can only ever name a model that
    /// engine serves.
    ///
    /// `None` means the configured pricing stands: a model that carries its
    /// own price or has a twin to estimate from is the cost authority, and a
    /// session model this registry does not list gets no price at all rather
    /// than an invented one.
    pub fn session_model_price(
        &self,
        configured: &ResolvedModel,
        reported: &str,
    ) -> Option<ModelPrice> {
        debug_assert!(
            configured.provider.protocol.is_engine(),
            "only an engine session reports the model it picked"
        );
        if configured.model.capabilities.cost.is_some()
            || self.api_equivalent_price(configured).is_some()
        {
            return None;
        }
        let session = configured.provider.model(reported)?;
        let price = match session.capabilities.cost {
            Some(cost) => ModelPrice::Billed(cost),
            None => ModelPrice::ApiEquivalent(
                self.api_twin(&configured.provider, &session.id)?
                    .capabilities
                    .cost?,
            ),
        };
        debug_assert!(
            is_payable(price.rate()),
            "`{}` prices {reported} at a rate no budget can carry: {:?}",
            configured.provider.id,
            price.rate()
        );
        Some(price)
    }

    pub fn apply_overrides(&mut self, file: ProvidersFile) -> Result<(), RegistryError> {
        for (id, o) in file.providers {
            match self.providers.iter_mut().find(|p| p.id == id) {
                Some(existing) => apply_override(existing, o)?,
                None => {
                    let (Some(protocol), Some(base_url)) = (o.protocol, o.base_url.clone()) else {
                        return Err(RegistryError::Config {
                            path: "providers.toml".into(),
                            reason: format!("new provider `{id}` needs `protocol` and `base_url`"),
                        });
                    };
                    let mut def = ProviderDef {
                        id: id.clone(),
                        name: id.clone(),
                        protocol,
                        base_url,
                        auth: AuthMethods::none(),
                        headers: BTreeMap::new(),
                        models: Vec::new(),
                        default_capabilities: None,
                        flags: ProviderFlags {
                            open_model_list: true,
                            ..Default::default()
                        },
                        max_concurrent: None,
                        family: None,
                        enabled: true,
                    };
                    apply_override(&mut def, o)?;
                    self.providers.push(def);
                }
            }
        }
        self.refuse_unusable_models()?;
        self.validate_model_names()?;
        self.refuse_subscription_over_http()
    }

    /// A model nobody can name, or one whose window makes every context
    /// percentage undefined, is a configuration mistake rather than a model.
    fn refuse_unusable_models(&self) -> Result<(), RegistryError> {
        for provider in &self.providers {
            for model in &provider.models {
                let reason = if model.id.trim().is_empty() {
                    format!(
                        "provider `{}` lists a model with an empty `id` (name `{}`)",
                        provider.id, model.name
                    )
                } else if model.capabilities.context_window == 0 {
                    format!(
                        "provider `{}` model `{}`: `context_window` must be greater than 0",
                        provider.id, model.id
                    )
                } else {
                    continue;
                };
                return Err(RegistryError::Config {
                    path: "providers.toml".into(),
                    reason,
                });
            }
        }
        Ok(())
    }

    fn validate_model_names(&self) -> Result<(), RegistryError> {
        for provider in &self.providers {
            let mut names = BTreeMap::new();
            for model in &provider.models {
                for name in std::iter::once(&model.id)
                    .chain(model.aliases.iter())
                    .collect::<std::collections::BTreeSet<_>>()
                {
                    // Repeated aliases within one definition are harmless; distinct
                    // definitions must still have unique ids and aliases.
                    if let Some(first) = names.insert(name, &model.id) {
                        return Err(RegistryError::Config {
                            path: "providers.toml".into(),
                            reason: format!(
                                "provider `{}` models `{first}` and `{}` share id or alias `{name}`",
                                provider.id, model.id
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// VISION §1.6: a subscription is used only through the official client
    /// that owns it. The shipped defaults are built that way; this holds it
    /// after configuration too, so three lines of `providers.toml` cannot
    /// point an HTTP adapter at a subscription login.
    fn refuse_subscription_over_http(&self) -> Result<(), RegistryError> {
        for p in &self.providers {
            if !p.enabled || p.protocol.is_engine() {
                continue;
            }
            if let Some(oauth) = p.auth.oauth.as_ref().filter(|o| o.is_subscription()) {
                return Err(RegistryError::Config {
                    path: "providers.toml".into(),
                    reason: format!(
                        "provider `{}` speaks `{}`, so it may not sign in with {}; \
                         a subscription is reached through the engine that owns it",
                        p.id,
                        p.protocol.as_str(),
                        oauth.label()
                    ),
                });
            }
        }
        Ok(())
    }
}

fn apply_override(def: &mut ProviderDef, o: ProviderOverride) -> Result<(), RegistryError> {
    if let Some(v) = o.name {
        def.name = v;
    }
    if let Some(v) = o.protocol {
        def.protocol = v;
    }
    if let Some(v) = o.base_url {
        def.base_url = v;
    }
    if let Some(v) = o.auth {
        def.auth = v;
    }
    if let Some(v) = o.headers {
        def.headers = v;
    }
    if let Some(v) = o.models {
        def.models = v;
    }
    for added in o.add_models {
        let model = added_model(def, added)?;
        add_model(def, model);
    }
    if let Some(v) = o.default_capabilities {
        def.default_capabilities = Some(v);
    }
    if let Some(v) = o.flags {
        def.flags = v;
    }
    if let Some(v) = o.max_concurrent {
        def.max_concurrent = (v > 0).then_some(v);
    }
    if let Some(v) = o.family {
        def.family = match v {
            FamilyOverride::None => None,
            FamilyOverride::Family(f) => Some(f),
        };
    }
    if let Some(v) = o.enabled {
        def.enabled = v;
    }
    Ok(())
}

/// The model an `add_models` entry describes, its capabilities resolved
/// against the models `def` lists so far.
fn added_model(def: &ProviderDef, added: AddedModel) -> Result<ModelDef, RegistryError> {
    let AddedModel {
        id,
        name,
        aliases,
        capabilities_from,
        capabilities: stated,
        family,
        default_effort,
        tier,
    } = added;
    let capabilities = match (capabilities_from, stated) {
        (None, None) => Ok(CapabilityProfile::default()),
        (None, Some(stated)) => stated
            .try_into()
            .map_err(|e: toml::de::Error| format!("capabilities: {}", e.message())),
        (Some(base), stated) => inherited_capabilities(def, &base, stated.unwrap_or_default()),
    }
    .map_err(|reason| RegistryError::Config {
        path: "providers.toml".into(),
        reason: format!("provider `{}` model `{id}`: {reason}", def.id),
    })?;
    Ok(ModelDef {
        id,
        name,
        aliases,
        capabilities,
        family,
        default_effort,
        tier,
    })
}

/// The capabilities of the model `base` names, with each `stated` field
/// replacing that model's value whole.
fn inherited_capabilities(
    def: &ProviderDef,
    base: &str,
    stated: toml::Table,
) -> Result<CapabilityProfile, String> {
    let base = def.model(base).ok_or_else(|| {
        format!("`capabilities_from` names `{base}`, which this provider does not list")
    })?;
    let mut merged = toml::Table::try_from(&base.capabilities)
        .map_err(|e| format!("capabilities of `{}`: {e}", base.id))?;
    let stated_fields: Vec<String> = stated.keys().cloned().collect();
    merged.extend(stated);
    let capabilities: CapabilityProfile = merged
        .try_into()
        .map_err(|e: toml::de::Error| format!("capabilities: {}", e.message()))?;
    // A misspelt field would otherwise leave the base model's value in place, unnoticed.
    let kept = toml::Table::try_from(&capabilities).map_err(|e| format!("capabilities: {e}"))?;
    match stated_fields
        .iter()
        .find(|field| !kept.contains_key(*field))
    {
        Some(unknown) => Err(format!("capabilities: unknown field `{unknown}`")),
        None => Ok(capabilities),
    }
}

/// Replace the listed model `m` names, or append it as a new one.
fn add_model(def: &mut ProviderDef, mut m: ModelDef) {
    if let Some(existing) = def.models.iter_mut().find(|e| e.id == m.id) {
        *existing = m;
    } else if let Some(existing) = def.models.iter_mut().find(|e| e.matches(&m.id)) {
        // An id that is now an alias (e.g. a pinned `sonnet`) overrides the model it
        // names; appended, lookup would still hit the alias. Keep canonical id + aliases.
        let mut aliases = std::mem::take(&mut existing.aliases);
        for alias in std::mem::take(&mut m.aliases) {
            if !aliases.contains(&alias) {
                aliases.push(alias);
            }
        }
        m.id = std::mem::take(&mut existing.id);
        m.aliases = aliases;
        *existing = m;
    } else {
        def.models.push(m);
    }
}

#[cfg(test)]
mod tests;
