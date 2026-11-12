//! Shared transcript view definitions and bounded discovery of user modes.
mod catalog;
mod definition;
mod files;
mod reserved;

pub use catalog::{Catalog, Diagnostic, Mode};
pub use definition::{Category, Definition, Density, Scope, valid_name};
pub use files::discover;
pub use reserved::RESERVED_NAMES;
