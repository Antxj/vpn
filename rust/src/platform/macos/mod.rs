//! macOS implementation of the platform interface (see ../mod.rs). Modules
//! shared with Linux come from ../unix, the dialogs from ../rfd_dialogs.rs.

pub mod desktop;
pub mod locale;
pub mod openvpn;
pub mod paths;
pub mod routing;
pub mod service;
pub mod startup;
pub mod updater;

pub use super::rfd_dialogs as dialogs;
pub use super::unix::{installer, renderer, secret, single};
