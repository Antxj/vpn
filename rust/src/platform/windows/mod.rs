//! Windows implementation of the platform interface (see ../mod.rs).

pub mod desktop;
pub use super::rfd_dialogs as dialogs;
pub mod elevate;
pub mod installer;
pub mod locale;
pub mod openvpn;
pub mod paths;
pub mod renderer;
pub mod routing;
pub mod secret;
pub mod service;
pub mod single;
pub mod startup;
pub mod updater;
