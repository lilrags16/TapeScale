//! `TapeScale` control-plane library.
//!
//! The crate intentionally keeps physical tape operations behind an engine adapter. The shipping
//! engineering preview uses a deterministic simulator and never promotes simulated evidence to a
//! hardware qualification claim.

pub mod api;
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
pub mod qualification;
pub mod restore_rehearsal;
pub mod store;
pub mod tool_runner;
pub mod truenas;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
