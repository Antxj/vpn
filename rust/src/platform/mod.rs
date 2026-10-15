//! Everything that talks to the operating system.
//!
//! Each system has a folder with the same modules and the same interface
//! (functions, types and constants with the same names); the rest of the app
//! only uses that interface and does not need to know which system it runs
//! on. A new system (e.g. macOS) is a new folder with the same modules.
//!
//! - `desktop`: window, tray icon, notifications
//! - `dialogs`: native dialogs (messages, questions, open/save file)
//! - `installer`: OpenVPN installation (embedded MSI on Windows)
//! - `locale`: interface language and local time of the system
//! - `openvpn`: where OpenVPN is installed and where the configs usually are
//! - `paths`: where the app keeps its data
//! - `renderer`: graphics API that draws the window
//! - `routing`: the routing table and the interface of an address
//! - `secret`: protection of the saved accounts
//! - `service`: starting OpenVPN without administrator rights, the one-time
//!   authorization and the special command line arguments
//! - `single`: single instance
//! - `startup`: start with the system
//! - `updater`: download and checks of a new version

#[cfg(windows)]
#[path = "windows/mod.rs"]
mod sistema;

#[cfg(target_os = "linux")]
#[path = "linux/mod.rs"]
mod sistema;

pub use sistema::*;

/// Local date and time (from the system clock and time zone).
#[derive(Clone, Copy, Debug, Default)]
pub struct DataHora {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}
