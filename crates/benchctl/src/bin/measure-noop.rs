//! `measure-noop` — measure the per-prompt `noop_decode_speedup` reference for a track's
//! `timed_prompt_pool`, on the STOCK engine, single-stream, apples-to-apples with the scored
//! per-prompt decode speedup.
//!
//! WHAT IT COMPUTES (RULED recipe, orchestrator 2026-08-25). Per pool prompt, on the stock
//! engine, single-stream (NON-batched), `--pairs` (N=4) pairs of:
//!   * a SERIAL leg  — free-run window with wire spec `{"mode":"serial"}`
//!   * an MTP leg    — free-run window with wire spec `{"mode":"mtp","mtp":{"depth":1}}`
//!
//! For each leg the SCORED number is benchd's OWN parent-side wall clock
//! `seconds_per_token = decode-phase elapsed / committed tokens` — obtained by calling the SAME
//! runner free-run timing the scored measure-job single-stream path uses, via the TIME-ONLY
//! sibling [`bench_runner::run_free_run_decode_phase_fresh_time_only`]. That variant shares the
//! scored `run_free_run_decode_phase_fresh` body byte-for-byte (`crates/bench-runner/src/timing.rs`,
//! `measure_free_run_decode` divides the parent `Instant` span by N); it differs ONLY in tolerating
//! the §2.7 committed-token mismatch — the stock engine legitimately diverges from the teacher-
//! forced tape under free-run, and a RATE measurement must not abort on that. We do NOT hand-roll a
//! timer and do NOT read the engine's self-reported `decode_ns` (audit-only, per the recipe TIMING
//! section). The scored/participant paths keep `VerifyMode::Verify` and abort as before.
//!
//!   serial_mean = mean(serial seconds_per_token over N)   (arithmetic mean)
//!   mtp_mean    = mean(mtp    seconds_per_token over N)
//!   noop_decode_speedup = serial_mean / mtp_mean          (RATIO-OF-MEANS, serial numerator)
//!
//! This equals how a candidate's per-prompt decode speedup (`raw_ratio_of_means`) is computed
//! (`measure_job.rs` ratio-of-means at :5082-5098 / `score.rs:366`). NOT ratio-of-per-pair-ratios,
//! NOT per-pair median, NOT min.
//!
//! WHY BOTH LEGS ARE FREE-RUN WINDOWS. This mirrors the scored single-stream path exactly: the
//! serial control there is a depth-0 FREE-RUN leg carrying `SpecConfig::serial()` on the wire
//! (`main.rs` `serial_wire_spec = timed_decode_wire_spec()` → `run_free_run_decode_phase_fresh`),
//! and the candidate a free-run leg carrying its declared spec. Reusing that one runner function
//! is what makes the span byte-identical to the scored path.
//!
//! The `depth:1` on the MTP leg is sent EXPLICITLY (`SpecConfig::mtp(1)` serializes to
//! `{"mode":"mtp","mtp":{"depth":1}}`): omitting the depth resolves to depth 3 on gemma, which
//! would measure the wrong leg (recipe LEG INVOCATION note; `MTPEnvelope.swift` resolveDepth).
//!
//! SPAWN. One fresh `runtime-worker` per leg via [`bench_runner::ChildStdioTransport::spawn`]
//! (`--weights <TARGET> --mtp-head <ASSISTANT> --speculative-protocol v1.1`; the `v1.1` gate is
//! REQUIRED for free-run legs — `run_free_run_decode_phase_fresh` refuses an engine that does not
//! advertise `free_run_decode`) and the [`bench_runner::Session`] hello handshake.
//!
//! This binary WRITES + measures; it never edits a fixture. It emits a JSON array + a table the
//! operator transcribes into `timed_prompt_pool[]` by hand.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bench_protocol::SpecConfig;
use bench_runner::{
    run_free_run_decode_phase_fresh_time_only, ChildStdioTransport, Session, TimingParams,
};
use sha2::{Digest, Sha256};

/// Default per-prompt decode window (committed tokens per leg). The tape must oracle at least
/// this many rows.
const DEFAULT_COUNT: usize = 128;
/// Default number of serial+mtp pairs per prompt (recipe N=4).
const DEFAULT_PAIRS: usize = 4;

// ----------------------------------------------------------------------------------------------
// PURE REDUCTION — the load-bearing arithmetic, isolated so the unit test can pin the exact
// RATIO-OF-MEANS value (a wrong reduction — ratio-of-per-pair-ratios, median, min — differs).
// ----------------------------------------------------------------------------------------------

/// The per-prompt reduction over N pairs of (serial spt, mtp spt).
#[derive(Debug, Clone, PartialEq)]
struct NoopReduction {
    serial_mean: f64,
    mtp_mean: f64,
    /// serial_mean / mtp_mean — RATIO-OF-MEANS, serial in the numerator.
    noop_decode_speedup: f64,
    pairs: usize,
    /// Diagnostic dispersion of the per-pair ratios: (max-min)/mean, as a percent. NOT scored.
    spread_pct: f64,
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Reduce N pairs to the per-prompt `noop_decode_speedup` (ratio-of-means) plus the spread
/// diagnostic over the per-pair ratios. `serial` and `mtp` are the per-pair parent-measured
/// seconds-per-token, in pair order, equal length and non-empty.
fn reduce_pairs(serial: &[f64], mtp: &[f64]) -> NoopReduction {
    assert_eq!(serial.len(), mtp.len(), "serial/mtp pair counts must match");
    assert!(!serial.is_empty(), "need at least one pair");

    let serial_mean = mean(serial);
    let mtp_mean = mean(mtp);
    // RATIO-OF-MEANS — mean(serial) / mean(mtp). NOT mean(serial_i/mtp_i), NOT median, NOT min.
    let noop_decode_speedup = serial_mean / mtp_mean;

    // Diagnostic only: dispersion of the per-pair ratios (serial_i / mtp_i).
    let ratios: Vec<f64> = serial.iter().zip(mtp.iter()).map(|(s, m)| s / m).collect();
    let ratio_mean = mean(&ratios);
    let ratio_min = ratios.iter().cloned().fold(f64::INFINITY, f64::min);
    let ratio_max = ratios.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let spread_pct = if ratio_mean != 0.0 {
        (ratio_max - ratio_min) / ratio_mean * 100.0
    } else {
        0.0
    };

    NoopReduction {
        serial_mean,
        mtp_mean,
        noop_decode_speedup,
        pairs: serial.len(),
        spread_pct,
    }
}

// ----------------------------------------------------------------------------------------------
// CLI
// ----------------------------------------------------------------------------------------------

struct Args {
    worker_bin: String,
    weights: PathBuf,
    mtp_head: PathBuf,
    contract: PathBuf,
    pool_dir: PathBuf,
    pairs: usize,
    count: usize,
}

const USAGE: &str = "\
measure-noop — measure per-prompt noop_decode_speedup for a track's timed_prompt_pool
(stock engine, single-stream, apples-to-apples with the scored per-prompt decode speedup).

USAGE:
    measure-noop --worker-bin <PATH> --weights <TARGET_DIR> --mtp-head <ASSISTANT_DIR> \\
                 --contract <TRACK_FIXTURE.json> --pool-dir <DIR> [--pairs 4] [--count 128]

FLAGS:
    --worker-bin <PATH>   Engine executable (spawned as `<bin> runtime-worker --weights <DIR>`).
    --weights <DIR>       TARGET (backbone) weights directory.
    --mtp-head <DIR>      ASSISTANT / MTP head directory (loaded on BOTH legs).
    --contract <PATH>     Track fixture JSON; its timed_prompt_pool[].sha256 list is measured.
    --pool-dir <DIR>      Directory holding the fetched pool object JSONs (matched by sha256).
    --pairs <N>           Serial+MTP pairs per prompt (default 4).
    --count <N>           Committed decode tokens per leg (default 128).
";

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut worker_bin = None;
    let mut weights = None;
    let mut mtp_head = None;
    let mut contract = None;
    let mut pool_dir = None;
    let mut pairs = DEFAULT_PAIRS;
    let mut count = DEFAULT_COUNT;

    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("flag {flag} needs a value"))
        };
        match flag.as_str() {
            "--worker-bin" => worker_bin = Some(value()?),
            "--weights" => weights = Some(PathBuf::from(value()?)),
            "--mtp-head" => mtp_head = Some(PathBuf::from(value()?)),
            "--contract" => contract = Some(PathBuf::from(value()?)),
            "--pool-dir" => pool_dir = Some(PathBuf::from(value()?)),
            "--pairs" => pairs = value()?.parse().map_err(|e| format!("--pairs: {e}"))?,
            "--count" => count = value()?.parse().map_err(|e| format!("--count: {e}"))?,
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown flag {other}\n\n{USAGE}")),
        }
    }
    if pairs == 0 {
        return Err("--pairs must be positive".to_string());
    }
    if count == 0 {
        return Err("--count must be positive".to_string());
    }
    Ok(Args {
        worker_bin: worker_bin.ok_or("--worker-bin is required")?,
        weights: weights.ok_or("--weights is required")?,
        mtp_head: mtp_head.ok_or("--mtp-head is required")?,
        contract: contract.ok_or("--contract is required")?,
        pool_dir: pool_dir.ok_or("--pool-dir is required")?,
        pairs,
        count,
    })
}

// ----------------------------------------------------------------------------------------------
// POOL RESOLUTION — one contract sha256 → its pool object bytes on disk.
// ----------------------------------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// One timed_prompt_pool entry, pulled from the raw contract JSON so we can surface the
/// (serde-ignored-by-measure-job) `r2_path` for the operator's transcription.
struct PoolPin {
    sha256: String,
    bytes: Option<u64>,
    r2_path: Option<String>,
}

fn read_pool_pins(contract_path: &Path) -> Result<Vec<PoolPin>, String> {
    let bytes = std::fs::read(contract_path)
        .map_err(|e| format!("--contract read failed ({}): {e}", contract_path.display()))?;
    let doc: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("--contract parse failed: {e}"))?;
    let pool = doc
        .get("timed_prompt_pool")
        .and_then(|v| v.as_array())
        .ok_or("--contract has no timed_prompt_pool array")?;
    let mut pins = Vec::with_capacity(pool.len());
    for (i, entry) in pool.iter().enumerate() {
        let sha256 = entry
            .get("sha256")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("timed_prompt_pool[{i}] has no string sha256"))?
            .to_string();
        pins.push(PoolPin {
            sha256,
            bytes: entry.get("bytes").and_then(|v| v.as_u64()),
            r2_path: entry
                .get("r2_path")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }
    if pins.is_empty() {
        return Err("--contract timed_prompt_pool is empty".to_string());
    }
    Ok(pins)
}

/// Locate the pool object file whose bytes hash to `sha256`, verifying the hash (and byte count
/// when the pin carries one). Tries `<pool_dir>/<sha256>.json` first, then scans `*.json`.
fn resolve_pool_object(pool_dir: &Path, pin: &PoolPin) -> Result<PathBuf, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let direct = pool_dir.join(format!("{}.json", pin.sha256));
    if direct.is_file() {
        candidates.push(direct);
    }
    let entries = std::fs::read_dir(pool_dir)
        .map_err(|e| format!("--pool-dir read failed ({}): {e}", pool_dir.display()))?;
    for e in entries {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.extension().and_then(|x| x.to_str()) == Some("json") && !candidates.contains(&p) {
            candidates.push(p);
        }
    }
    for path in candidates {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if sha256_hex(&bytes) == pin.sha256 {
            if let Some(want) = pin.bytes {
                if want != bytes.len() as u64 {
                    return Err(format!(
                        "pool object {} matches sha256 {} but is {} bytes, pin declares {}",
                        path.display(),
                        pin.sha256,
                        bytes.len(),
                        want
                    ));
                }
            }
            return Ok(path);
        }
    }
    Err(format!(
        "no file in {} hashes to pinned sha256 {}",
        pool_dir.display(),
        pin.sha256
    ))
}

// ----------------------------------------------------------------------------------------------
// MEASUREMENT
// ----------------------------------------------------------------------------------------------

/// The stock-engine spawn args for one leg: `--mtp-head <DIR> --speculative-protocol v1.1`.
/// (`--weights <DIR>` is prepended by the transport.) Mirrors `measure_job::leg_spawn_args` for a
/// free-run leg; `v1.1` is REQUIRED so the engine advertises `free_run_decode`.
fn leg_extra_args(mtp_head: &Path) -> Vec<String> {
    vec![
        "--mtp-head".to_string(),
        mtp_head.to_string_lossy().to_string(),
        "--speculative-protocol".to_string(),
        "v1.1".to_string(),
    ]
}

/// Run ONE free-run leg and return benchd's own parent-measured `seconds_per_token`. `spec` is
/// carried on the timed window (spec-never-ignored: the runner discards the session if the
/// engine's echo diverges), so the serial vs mtp-depth-1 leg is unambiguous end to end.
fn run_leg(
    worker_bin: &str,
    weights: &str,
    mtp_head: &Path,
    params: &TimingParams,
    spec: SpecConfig,
) -> bench_runner::Result<f64> {
    let extra = leg_extra_args(mtp_head);
    let mut spawn = || -> bench_runner::Result<Session<ChildStdioTransport>> {
        let transport = ChildStdioTransport::spawn(worker_bin, weights, &extra)?;
        let (session, _hello) = Session::connect(transport)?;
        Ok(session)
    };
    // No thermal cool gate: this bring-up tool runs under operator supervision and the cool gate
    // is a pre-timer thermal step, never a timing input. (The scored path threads
    // `coolgate::cool_gate_report`, which lives in the benchctl BINARY crate and is not reachable
    // from a sibling bin without a lib refactor — see the report.)
    let mut gate = |_phase: &str| -> bench_runner::Result<()> { Ok(()) };
    let params = params.clone().with_spec(Some(spec));
    // TIME-ONLY: a noop RATE measurement must NOT abort when the stock engine's free-run stream
    // diverges from the teacher-forced tape (it legitimately does). This variant tolerates the
    // §2.7 mismatch; it computes the identical parent-clock rate and feeds no scored value. Every
    // scored/participant path uses the Verify entry point instead.
    let timing = run_free_run_decode_phase_fresh_time_only(&mut spawn, &mut gate, &params)?;
    // H1 — benchd's OWN parent clock is the only scored number.
    Ok(timing.seconds_per_token)
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&argv)?;

    let worker_bin = args.worker_bin.as_str();
    let weights = args.weights.to_string_lossy().to_string();
    let pins = read_pool_pins(&args.contract)?;

    eprintln!(
        "measure-noop: {} pool prompts × {} pairs (serial + mtp depth-1), {} committed tokens/leg",
        pins.len(),
        args.pairs,
        args.count
    );

    let mut rows: Vec<serde_json::Value> = Vec::with_capacity(pins.len());
    // (sha256, r2_path, speedup, pairs, spread_pct) for the human table.
    let mut table: Vec<(String, Option<String>, f64, usize, f64)> = Vec::new();

    for (idx, pin) in pins.iter().enumerate() {
        let path = resolve_pool_object(&args.pool_dir, pin)?;
        // Load + validate the tape, then build the SAME decode-only TimingParams the scored
        // single-stream path builds (`measure_job::timing_params`, Tape arm): seed_tokens,
        // reference_seed_token, row_argmax_chain, N=count.
        let tape = bench_core::tape::load_timed_prompt_tape_from_path(&path, None)
            .map_err(|e| format!("pool object {}: {e}", path.display()))?;
        if tape.sha256 != pin.sha256 {
            return Err(format!(
                "pool object {} hashes to {} but pin declares {}",
                path.display(),
                tape.sha256,
                pin.sha256
            ));
        }
        if tape.row_count() < args.count {
            return Err(format!(
                "pool object {} oracles {} rows; --count {} needs at least that many",
                pin.sha256,
                tape.row_count(),
                args.count
            ));
        }
        let params = TimingParams::decode_only(
            tape.seed_tokens.clone(),
            tape.reference_seed_token,
            tape.row_argmax_chain(),
            args.count,
        );

        eprintln!(
            "  [{}/{}] {} — measuring {} pairs...",
            idx + 1,
            pins.len(),
            &pin.sha256[..pin.sha256.len().min(12)],
            args.pairs
        );

        let mut serial_spt: Vec<f64> = Vec::with_capacity(args.pairs);
        let mut mtp_spt: Vec<f64> = Vec::with_capacity(args.pairs);
        for pair in 0..args.pairs {
            let s = run_leg(
                worker_bin,
                &weights,
                &args.mtp_head,
                &params,
                SpecConfig::serial(),
            )
            .map_err(|e| format!("prompt {} serial leg (pair {pair}): {e}", pin.sha256))?;
            // depth:1 EXPLICIT — {"mode":"mtp"} alone resolves to depth 3 on gemma (wrong leg).
            let m = run_leg(
                worker_bin,
                &weights,
                &args.mtp_head,
                &params,
                SpecConfig::mtp(1),
            )
            .map_err(|e| format!("prompt {} mtp leg (pair {pair}): {e}", pin.sha256))?;
            serial_spt.push(s);
            mtp_spt.push(m);
        }

        let r = reduce_pairs(&serial_spt, &mtp_spt);
        let mut obj = serde_json::Map::new();
        obj.insert("sha256".into(), pin.sha256.clone().into());
        if let Some(r2) = &pin.r2_path {
            obj.insert("r2_path".into(), r2.clone().into());
        }
        obj.insert(
            "noop_decode_speedup".into(),
            serde_json::json!(r.noop_decode_speedup),
        );
        obj.insert(
            "noop_decode_speedup_pairs".into(),
            serde_json::json!(r.pairs),
        );
        obj.insert(
            "noop_decode_speedup_spread_pct".into(),
            serde_json::json!(r.spread_pct),
        );
        rows.push(serde_json::Value::Object(obj));
        table.push((
            pin.sha256.clone(),
            pin.r2_path.clone(),
            r.noop_decode_speedup,
            r.pairs,
            r.spread_pct,
        ));
    }

    // Human-readable table (stderr, so stdout stays pure JSON for piping).
    eprintln!(
        "\n  {:<20}  {:>12}  {:>6}  {:>10}  r2_path",
        "sha256", "noop_speedup", "pairs", "spread%"
    );
    for (sha, r2, speedup, pairs, spread) in &table {
        eprintln!(
            "  {:<20}  {:>12.4}  {:>6}  {:>9.2}%  {}",
            &sha[..sha.len().min(20)],
            speedup,
            pairs,
            spread,
            r2.as_deref().unwrap_or("-")
        );
    }
    eprintln!();

    // The transcribable JSON array on stdout.
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::Value::Array(rows)).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("measure-noop: {msg}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutation-proof: pin the EXACT ratio-of-means value and show a wrong reduction differs.
    ///
    /// serial=[4,2], mtp=[1,2]:
    ///   serial_mean = 3.0, mtp_mean = 1.5  → RATIO-OF-MEANS = 3.0/1.5 = 2.0 (exact).
    ///   mean-of-per-pair-ratios = (4/1 + 2/2)/2 = 2.5   (a WRONG reduction)
    ///   median-of-per-pair-ratios (sorted [1.0,4.0]) = 2.5   (a WRONG reduction)
    ///   min-of-per-pair-ratios = 1.0    (a WRONG reduction)
    /// Only ratio-of-means yields 2.0, so the assertion rejects each wrong reduction.
    #[test]
    fn ratio_of_means_is_exact_and_rejects_wrong_reductions() {
        let serial = [4.0_f64, 2.0];
        let mtp = [1.0_f64, 2.0];
        let r = reduce_pairs(&serial, &mtp);

        // EXACT ratio-of-means (numerator = serial).
        assert_eq!(r.serial_mean, 3.0);
        assert_eq!(r.mtp_mean, 1.5);
        assert_eq!(r.noop_decode_speedup, 2.0);

        // A wrong reduction would have produced one of these instead.
        let ratios = [serial[0] / mtp[0], serial[1] / mtp[1]]; // [4.0, 1.0]
        let mean_of_ratios = (ratios[0] + ratios[1]) / 2.0; // 2.5
        let min_of_ratios = ratios[1]; // 1.0
        assert_ne!(r.noop_decode_speedup, mean_of_ratios);
        assert_ne!(r.noop_decode_speedup, min_of_ratios);

        // Spread diagnostic over the per-pair ratios: (4.0-1.0)/2.5 = 1.2 = 120%.
        assert_eq!(r.spread_pct, 120.0);
        assert_eq!(r.pairs, 2);
    }

    /// N=4 default shape, and a case where serial is faster than mtp (speedup < 1), to confirm
    /// the numerator is serial and the mean is arithmetic.
    #[test]
    fn four_pair_arithmetic_mean_ratio() {
        let serial = [1.0_f64, 1.0, 1.0, 1.0];
        let mtp = [2.0_f64, 2.0, 2.0, 2.0];
        let r = reduce_pairs(&serial, &mtp);
        assert_eq!(r.serial_mean, 1.0);
        assert_eq!(r.mtp_mean, 2.0);
        assert_eq!(r.noop_decode_speedup, 0.5); // 1.0 / 2.0
        assert_eq!(r.pairs, 4);
        assert_eq!(r.spread_pct, 0.0); // identical ratios ⇒ zero spread
    }
}
