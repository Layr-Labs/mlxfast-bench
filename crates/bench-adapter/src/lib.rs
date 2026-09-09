//! Engine Protocol v1 adapter SDK: the NDJSON-over-stdio loop a track engine
//! reuses instead of writing its own.
//!
//! A track engine repository implements the [`engine::Engine`] trait and hands
//! a factory to [`adapter::Adapter`]. The adapter reads request lines from
//! stdin, drives an engine minted fresh per phase, and writes response lines to
//! stdout. The whole protocol surface — framing, ordering, the phase-close
//! barrier, the `completed_work` counter, fresh-engine-per-phase,
//! session-discard-on-error, and the v1.1 free-run pair — is proven against the
//! deterministic [`mock`] backend with no GPU.
//!
//! The wire types come from `bench-protocol`, the single normative definition
//! of the wire. This crate defines no wire type of its own.
//!
//! ## THE ENGINE REPORTS RAW COUNTERS ONLY
//!
//! No rate, no ratio, no elapsed time, no speedup, on any verb. Measurement and
//! scoring live in benchd, which times each request from its own side and does
//! every division. `tests::free_run_response_carries_no_derived_metric` is the
//! tripwire, and its scope is the top-level response keys — the surface where
//! the engine chooses the key names.
//!
//! LIFT ORIGIN: adapted from the CUDA track's standalone `protocol-adapter`
//! crate (`cudafast-qwen38-125b-a6b-engine-dev/harness/protocol-adapter/src/`:
//! `adapter.rs`, `engine.rs`, `mock.rs`, `tests.rs`, `lib.rs`), whose vendored
//! `protocol.rs` copy of the wire is replaced here by `bench-protocol`.

pub mod adapter;
pub mod engine;
pub mod mock;

pub use adapter::{Adapter, FREE_RUN_MAX_COUNT};
pub use engine::{Engine, EngineError, EngineFactory, FreeRunResult, Route, Step};

#[cfg(test)]
mod tests;
