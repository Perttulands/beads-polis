//! beads-v2: Event-sourced work tracker.
//!
//! JSONL is the source of truth. SQLite is a derived, disposable index.

pub mod bead;
pub mod cli;
pub mod compact;
pub mod config;
pub mod doctor;
pub mod engine;
pub mod event;
pub mod index;
pub mod log;
pub mod observe;
