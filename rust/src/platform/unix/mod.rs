//! Code shared by Linux and macOS (both Unix). Whole modules of the
//! interface are re-exported by linux/mod.rs and macos/mod.rs; the others
//! are the common part of a module that each system completes.

pub mod helper;
pub mod installer;
pub mod locale;
pub mod paths;
pub mod renderer;
pub mod routing;
pub mod secret;
pub mod single;
pub mod updater;
pub mod window;
