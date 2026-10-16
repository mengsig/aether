//! Terminal UI: a transcript-first client of the Aether daemon, with the
//! agent board docked beside it.
//!
//! The reducer in [`app`] is pure and the renderer in [`ui`] only paints, so
//! the interaction model is testable without a terminal. [`run`] wires both
//! to a real terminal and a live [`aether_client::Client`].

// Both branches are authored UI text; arguments are never converted.
macro_rules! ui_format {
    ($theme:expr, $unicode:literal, $ascii:literal $(, $($args:tt)*)?) => {
        if $theme.ascii() {
            format!($ascii $(, $($args)*)?)
        } else {
            format!($unicode $(, $($args)*)?)
        }
    };
}
pub(crate) use ui_format;

pub mod app;
mod clipboard;
pub mod commands;
pub mod format;
pub mod image_input;
mod input_layout;
mod keyboard;
pub mod keys;
pub mod motion;
pub mod plain;
pub mod prefs;
mod runner;
pub mod testing;
pub mod ui;
pub mod viewed;
pub mod voice;
pub mod workspace;

pub use app::{App, AppEvent, Effect, TuiOptions};
pub use runner::run;

pub use plain::run as run_plain;

pub mod view_modes;
