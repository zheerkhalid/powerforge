//! The egui user interface.

mod app;
mod apps_page;
mod auto_page;
mod awake_page;
mod charts;
mod dialogs;
mod energy_page;
mod health_page;
mod overview;
mod plans_page;
mod settings_page;
mod sleep_page;
mod theme;
mod widgets;

use crate::engine::{Control, Shared};
use std::path::PathBuf;
use std::sync::Arc;

pub use app::run;

/// How the window was started.
#[derive(Clone, Default)]
pub struct Options {
    /// Show an invented laptop; nothing on this PC is read or changed.
    pub demo: bool,
    /// Render one frame of the chosen view to this PNG (1280x800), then exit.
    pub screenshot: Option<PathBuf>,
    /// Which page / dialog to open first.
    pub view: Option<String>,
    /// Force the light theme (for screenshots).
    pub light: bool,
    /// State shared with the engine (None in demo mode).
    pub shared: Option<Arc<Shared>>,
    /// Where commands go (None in demo mode).
    pub control: Option<Arc<dyn Control>>,
}
