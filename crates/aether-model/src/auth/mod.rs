//! Authentication, isolated from inference: credential storage, resolution,
//! and the login flows. Provider changes stay inside this module.

pub mod cli;
pub mod helper;
pub mod import;
pub mod interaction;
pub mod methods;
pub mod oauth;
pub mod resolve;
pub mod store;
pub mod types;

pub use cli::{CliAuth, CliStatus};
pub use helper::CredentialHelper;
pub use import::CredentialImport;
pub use interaction::{
    AuthEvent, AuthInteraction, AuthPrompt, ScriptedInteraction, SelectOption, TerminalInteraction,
};
pub use methods::{AuthMethods, OAuthKind};
pub use resolve::{AuthResolver, LoginMethod, NO_AUTH_SOURCE, ResolvedAuth};
pub use store::{FileCredentialStore, MemoryCredentialStore};
pub use types::{AuthError, AuthKind, Credential, CredentialInfo, CredentialStore, ModifyFn};
