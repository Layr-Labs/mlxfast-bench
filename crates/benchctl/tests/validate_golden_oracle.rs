//! #77 — `benchctl validate-golden` requires the benchmark oracle by default.
//!
//! Byte-consistent with Swift preflight (which rejects a benchmark-less golden with
//! "benchmark golden file must contain a benchmark oracle"):
//!   - a structurally-valid but benchmark-LESS golden is REJECTED (exit 1) by default,
//!     and ACCEPTED (exit 0) with `--gates-only`;
//!   - a benchmark-HAVING golden is ACCEPTED (exit 0) both ways.
//!
//! Inputs are two committed fuzz fixtures (loadable under the production constants
//! steps=64 / prompt_tokens=1024 / model_type=gemma4_text):
//!   - `valid_cases_only.json` — structurally valid, no benchmark oracle (#77 target).
//!   - `valid.json`            — canonical golden, has a benchmark oracle.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../bench-core/tests/fixtures/golden_fuzz")
        .join(name)
}

fn run(args: &[&str]) -> i32 {
    let out = Command::new(env!("CARGO_BIN_EXE_benchctl"))
        .arg("validate-golden")
        .args(args)
        .output()
        .expect("spawn benchctl");
    out.status
        .code()
        .expect("benchctl exited via signal, no code")
}

#[test]
fn benchmark_less_golden_rejected_by_default_accepted_with_gates_only() {
    let g = fixture("valid_cases_only.json");
    let g = g.to_str().unwrap();

    // Default: benchmark oracle REQUIRED -> REJECT (exit 1), matching Swift preflight.
    assert_eq!(
        run(&["--golden", g]),
        1,
        "benchmark-less golden must be REJECTED (exit 1) by default"
    );

    // --gates-only: benchmark oracle requirement SKIPPED -> ACCEPT (exit 0).
    assert_eq!(
        run(&["--golden", g, "--gates-only"]),
        0,
        "benchmark-less golden must be ACCEPTED (exit 0) with --gates-only"
    );
}

#[test]
fn benchmark_having_golden_accepted_both_ways() {
    let g = fixture("valid.json");
    let g = g.to_str().unwrap();

    assert_eq!(
        run(&["--golden", g]),
        0,
        "benchmark-having golden must be ACCEPTED (exit 0) by default"
    );
    assert_eq!(
        run(&["--golden", g, "--gates-only"]),
        0,
        "benchmark-having golden must be ACCEPTED (exit 0) with --gates-only too"
    );
}
