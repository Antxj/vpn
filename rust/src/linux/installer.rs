//! Linux: OpenVPN comes from the distribution's package manager, so there is
//! no embedded installer. Same interface as the Windows module, never available.

use std::sync::mpsc::Sender;

pub const MSI_VERSION: &str = "";

#[allow(dead_code)]
pub enum Event {
    /// Finished: Ok(restart_recommended) or Err(message).
    Done(Result<bool, String>),
}

pub fn is_available() -> bool {
    false
}

pub fn install_in_background(_tx: Sender<Event>, _ctx: eframe::egui::Context) {}
