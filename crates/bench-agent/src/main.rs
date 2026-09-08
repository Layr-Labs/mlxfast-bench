//! bench-agent — native macOS (aarch64) timing peer for raw M5 engines.
//!
//! Wraps the engine in a generated Seatbelt profile, sanitizes env, spawns it, and OWNS
//! the parent-side wall clock; bridges to 127.0.0.1 behind a single-use, session-bound
//! token. Ring-0 trusted, release-hash pinned. No-op on Linux. See architecture.md §2 D4, §8.

fn main() {
    eprintln!("bench-agent: scaffold — see docs/architecture.md");
}
