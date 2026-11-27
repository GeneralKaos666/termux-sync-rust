//! Core and terminal UI for `termux-sync`.
//!
//! The library is an implementation detail of the two binaries. It is public
//! because Cargo binaries cannot access private library items, but no API
//! stability is promised.

pub mod core;
pub mod port;
pub mod tui;
