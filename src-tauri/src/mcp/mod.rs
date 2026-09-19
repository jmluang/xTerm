//! Local MCP exposure for xTerm (ORQ-27).
//!
//! `auth` holds pairing tokens and per-connection grants, `protocol` is the
//! line-delimited JSON spoken between the stdio bridge and the app, `service`
//! is the in-process core that owns authorization checks, and `commands` is
//! the user-facing Tauri surface (pairing, grants, approvals).
pub mod auth;
pub mod commands;
pub mod protocol;
pub mod service;
