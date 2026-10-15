//! Provider layer: the `ModelClient` contract, protocol adapters, model
//! registry with capability profiles, and authentication.
//!
//! Nothing here knows about agents, tasks or scheduling. The runtime asks
//! [`Models`] for a client by [`aether_core::model::ModelRef`] and streams
//! normalized [`aether_core::model::StreamEvent`]s back.

pub mod adapters;
pub mod auth;
pub mod builtin;
pub mod client;
pub mod config;
pub mod http;
pub mod models;
pub mod profile;
pub mod registry;
pub mod retired;
pub mod sse;
pub mod testing;
pub mod tier;

pub use client::{ModelClient, ModelStream, PartialFailure, collect, collect_partial};
pub use models::{AuthStatus, LoginMethod, ModelInfo, Models, ModelsError, ResolvedModel};
pub use profile::{FamilyProfiles, ModelFamily, ModelProfile, ProfileSetting};
pub use registry::{ModelDef, ModelPrice, Protocol, ProviderDef, ProviderFlags, Registry};
pub use tier::ModelTier;

/// Crate version, sent in every `User-Agent` header.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The user agent Aether identifies itself with. Never impersonates another client.
pub fn user_agent() -> String {
    format!("aether/{VERSION}")
}
