//! Safety-critical product rules.
//!
//! This module is intentionally independent from HTTP, persistence, and tape
//! engine implementations.  Callers receive explicit decisions and blockers;
//! adapters must not infer permission from the absence of an I/O error.

pub mod custody;
pub mod job;
pub mod lto;
pub mod media;
pub mod path;
pub mod rbac;
pub mod restore;
pub mod retention;
pub mod schedule;
