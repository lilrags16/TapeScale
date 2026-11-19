//! `TapeScale` control-plane library.
//!
//! Physical tape operations remain behind a fenced worker boundary. The alpha release uses the
//! TapeScale Native media format with DAR as a separate archive codec, and it never promotes
//! simulated evidence to a hardware qualification claim.

pub mod api;
pub mod backup_ops;
pub mod bacula_catalog;
pub mod bacula_runtime;
pub mod config;
pub mod custody_ops;
pub mod data_path;
pub mod data_run;
pub mod device;
pub mod domain;
pub mod engine;
pub mod error;
pub mod media_ops;
pub mod native_archive;
pub mod native_format;
pub mod native_tape;
pub mod qualification;
pub mod restore_rehearsal;
pub mod source_browser;
pub mod store;
pub mod tool_runner;
pub mod truenas;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RELEASE_CHANNEL: &str = "alpha";
