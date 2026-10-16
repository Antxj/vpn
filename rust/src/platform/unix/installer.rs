//! Linux and macOS: there is no installer to run. Linux uses the
//! distribution's OpenVPN; macOS the one embedded in the app, installed by
//! the one-time authorization (macos/service.rs). Same interface as the
//! Windows module, never available.

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
