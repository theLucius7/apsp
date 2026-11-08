//! Entry points of the domains whose code still lives in this crate (ADR 0128).
//!
//! Each domain declares its own routes here; the application assembles them with
//! the domain crates. When a domain moves into its own crate, its entry module
//! moves with it and becomes that crate's root.

pub mod costs;
pub mod governance;
pub mod networking;
pub mod resources;
pub mod traffic;
