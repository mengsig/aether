//! Colour lives here and nowhere else.
//!
//! [`Theme`] resolves the semantic tokens of SPEC §5.2 once; every style the
//! UI paints is a method on it, so no call site names a colour.
//! `tests/theme.rs` enforces that.

pub mod choice;
mod colors;
pub mod files;
pub(crate) mod image;
pub mod overrides;
pub mod probe;
pub mod scheme;
pub mod tokens;

pub use choice::ThemeChoice;
pub use colors::ColorDepth;
pub use scheme::{Builtin, Hue};
pub use tokens::{TerminalPalette, Theme, ThemeMode, ThemeSpec, ToolKind};
