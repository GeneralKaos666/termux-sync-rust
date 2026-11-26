//! Core of `termux-sync`: backup and restore orchestration, storage backends,
//! configuration, archive handling and maintenance commands.
//!
//! [`core`] holds the implementations. [`port`] re-exports them in the shape of
//! the original Python package for the CLI and the TUI.

pub mod core;
pub mod port;
