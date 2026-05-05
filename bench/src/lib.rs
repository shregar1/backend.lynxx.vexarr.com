//! lynxx-bench library crate.
//!
//! Hosts the `chaos` module so the `lynxx-chaos` binary can `use lynxx_bench::chaos`.
//! The open-loop bench (`src/main.rs`) does not depend on this crate; it is a
//! standalone binary kept here so the whole load-test toolkit ships together.

pub mod chaos;
