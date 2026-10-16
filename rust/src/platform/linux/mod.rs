//! Linux implementation of the platform interface (see ../mod.rs). Modules
//! shared with macOS come from ../unix.

pub mod desktop;
pub mod dialogs;
pub mod locale;
pub mod openvpn;
pub mod paths;
pub mod routing;
pub mod service;
pub mod startup;
pub mod updater;

pub use super::unix::{installer, renderer, secret, single};
