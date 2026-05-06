//! Library face of the Lynxx gateway.
//!
//! The crate is primarily a collection of binaries (`lynx-gateway`,
//! `lynxx-ingress`, `lynxx`) but a thin `lib.rs` is also exposed so that
//! integration tests under `tests/` and the `bench` workspace member can
//! reach into the same module tree without spawning a subprocess.
//!
//! The full module tree compiles into both the lib and the binaries; the
//! duplication is ~one extra compile pass and is intentional — it lets
//! integration tests assert directly against plugin internals.

#![allow(dead_code)]

pub mod admin;
pub mod audit;
pub mod auth;
pub mod cluster;
pub mod config;
pub mod db;
pub mod error;
pub mod health;
pub mod license;
pub mod metrics;
pub mod models;
pub mod plugins;
pub mod proxy;
pub mod quota;
pub mod recording;
pub mod reload;
