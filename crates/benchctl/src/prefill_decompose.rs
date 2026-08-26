//! `benchctl prefill-decompose` — a bench-runner-level PREFILL-DECOMPOSITION diagnostic
//! (MLX item M-5, #68).
//!
//! Attribution-by-collapse of the accepted +2.70% single-shot prefill residual (RULING A3):
//! is that residual per-token compute, or the fixed benchd<->engine protocol/spawn overhead
//! that Swift's in-process monolith never pays?
//!
//! The method: time the prefill round-trip at several synthetic prompt sizes `n`, then fit
//! `elapsed_ms = c + m*n` by ordinary least squares. A fixed intercept `c` that is
//! independent of `n` (≈16ms) attributes the residual to the protocol/spawn floor (physical,
//! not compute); `c≈0` attributes it to per-token compute.
//!
//! DIAGNOSTIC ONLY. This path shares the engine/protocol/timing seam with the scored
//! `iterate` path (`ChildStdioTransport::spawn` + `Session::connect` + `Session::prefill`,
//! parent-side `Instant` around the round-trip), but it NEVER verifies a golden oracle — it
//! only times the round-trip. It writes no score artifact and does not touch the scoring path.
//!
//! LIFECYCLE (documented choice): a FRESH engine process is spawned per SIZE, not per rep.
//! This matches §A's fresh-per-phase intent — each size's timing is measured on a cold
//! process that has not inherited a warm graph/allocator cache from an earlier size — while
//! keeping the per-size spawn cost OUT of the timed window (the spawn + hello handshake
//! complete before the first `Instant::now`, and all `--reps` round-trips reuse that one warm
//! session). The intercept `c` therefore isolates the *per-request* protocol floor, not the
//! one-time spawn: exactly the benchd<->engine overhead a per-request Swift call would also
//! pay, which is the quantity RULING A3 is about.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use bench_runner::{ChildStdioTransport, RunnerError, Session};

/// Default synthetic prompt sizes (tokens) swept when `--sizes` is omitted.
const DEFAULT_SIZES: &[usize] = &[128, 256, 512, 1024];
/// Default per-size round-trip repetitions when `--reps` is omitted.
const DEFAULT_REPS: usize = 8;
/// Synthetic in-vocab token id repeated to build each prompt (token id 1). TIMING only —
/// there is no oracle, so the actual value need only be a valid in-vocab id.
const SYNTHETIC_TOKEN: i64 = 1;

/// Intercept threshold (ms): at/above this, the fit is read as protocol/spawn-floor
/// dominated; below it, as per-token-compute dominated. Sits between the two hypotheses
/// (c≈16ms floor vs c≈0 compute).
const FLOOR_VERDICT_THRESHOLD_MS: f64 = 8.0;

pub const USAGE: &str = "\
benchctl prefill-decompose — attribution-by-collapse of the single-shot prefill residual

USAGE:
    benchctl prefill-decompose --engine <PATH> --weights <DIR> [OPTIONS]

REQUIRED:
    --engine <PATH>   Engine executable (spawned as `<engine> runtime-worker --weights <DIR>`)
    --weights <DIR>   Transformed weights directory

OPTIONS:
    --sizes <LIST>    Comma-separated synthetic prompt sizes in tokens (default: 128,256,512,1024)
    --reps <N>        Round-trip repetitions per size (default: 8)
    -h, --help        Show this help

Spawns a FRESH engine per size, sends `--reps` synthetic prefill round-trips (token id 1
repeated n times; TIMING only, NO oracle verification), records min + mean elapsed_ms, then
fits `elapsed_ms = c + m*n` by ordinary least squares and prints c, m, R² + a verdict.
";

/// Parsed `prefill-decompose` flags.
struct Args {
    engine: String,
    weights: PathBuf,
    sizes: Vec<usize>,
    reps: usize,
}

/// One size's measured round-trip timing.
#[derive(Debug, Clone, PartialEq)]
pub struct SizeMeasurement {
    /// Synthetic prompt size in tokens.
    pub n: usize,
    /// Round-trips timed at this size.
    pub reps: usize,
    /// Fastest round-trip (ms) — least noise-contaminated sample.
    pub min_ms: f64,
    /// Mean round-trip (ms) — the point fed to the fit.
    pub mean_ms: f64,
}

/// An ordinary-least-squares line fit `y = intercept + slope*x` plus its R².
#[derive(Debug, Clone, PartialEq)]
pub struct LinearFit {
    /// Intercept `c` (ms) — the n-independent floor.
    pub intercept_ms: f64,
    /// Slope `m` (ms/token) — the per-token cost.
    pub slope_ms_per_token: f64,
    /// Coefficient of determination R² in [.., 1.0]; 1.0 for a perfect fit.
    pub r_squared: f64,
}

/// Fit `y = c + m*x` over `points` by ordinary least squares.
///
/// Returns `None` when the fit is undetermined: fewer than 2 points, or all `x` equal
/// (a vertical line has no finite slope). R² is `1 - SS_res/SS_tot`; when every `y` is
/// identical (`SS_tot == 0`) the points already lie on the horizontal line the fit
/// recovers, so R² is defined as `1.0`.
pub fn least_squares_fit(points: &[(f64, f64)]) -> Option<LinearFit> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let mean_x = points.iter().map(|&(x, _)| x).sum::<f64>() / n;
    let mean_y = points.iter().map(|&(_, y)| y).sum::<f64>() / n;

    let mut sxx = 0.0_f64;
    let mut sxy = 0.0_f64;
    for &(x, y) in points {
        let dx = x - mean_x;
        sxx += dx * dx;
        sxy += dx * (y - mean_y);
    }
    if sxx == 0.0 {
        // All x equal — slope is undetermined.
        return None;
    }
    let slope = sxy / sxx;
    let intercept = mean_y - slope * mean_x;

    let mut ss_res = 0.0_f64;
    let mut ss_tot = 0.0_f64;
    for &(x, y) in points {
        let pred = intercept + slope * x;
        ss_res += (y - pred) * (y - pred);
        ss_tot += (y - mean_y) * (y - mean_y);
    }
    let r_squared = if ss_tot == 0.0 {
        1.0
    } else {
        1.0 - ss_res / ss_tot
    };

    Some(LinearFit {
        intercept_ms: intercept,
        slope_ms_per_token: slope,
        r_squared,
    })
}

/// One-line human verdict for a fit: floor-dominated (protocol/spawn) vs compute-dominated.
fn verdict(fit: &LinearFit) -> String {
    if fit.intercept_ms >= FLOOR_VERDICT_THRESHOLD_MS {
        format!(
            "VERDICT: c={:.2}ms is a fixed n-independent floor -> the residual is \
             benchd<->engine protocol/spawn overhead (physical; Swift's in-process monolith \
             does not pay it), not per-token compute.",
            fit.intercept_ms
        )
    } else {
        format!(
            "VERDICT: c={:.2}ms is near zero -> the residual is per-token-compute dominated \
             (slope {:.4} ms/token), not a fixed protocol/spawn floor.",
            fit.intercept_ms, fit.slope_ms_per_token
        )
    }
}

/// Time `reps` prefill round-trips of an `n`-token synthetic prompt on one warm session.
///
/// Parent-side `Instant` around each `Session::prefill` round-trip, exactly as the scored
/// timing path measures prefill — but with NO oracle check on the returned token (this is a
/// timing diagnostic). Returns the per-rep elapsed times in ms.
fn time_prefill_reps<T: bench_runner::LineTransport>(
    session: &mut Session<T>,
    n: usize,
    reps: usize,
) -> Result<Vec<f64>, RunnerError> {
    let prompt = vec![SYNTHETIC_TOKEN; n];
    let mut elapsed_ms = Vec::with_capacity(reps);
    for _ in 0..reps {
        let start = Instant::now();
        // TIMING diagnostic: time the round-trip, ignore the returned token (no oracle).
        let _resp = session.prefill(&prompt)?;
        elapsed_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(elapsed_ms)
}

/// Sweep every size with a FRESH engine per size (via `spawn`) and reduce each size's reps to
/// (min, mean) ms. `spawn` yields a freshly-connected [`Session`] (post-hello); the fresh
/// process per size is the documented lifecycle choice (see module docs).
fn measure_sizes<T, F>(
    spawn: &mut F,
    sizes: &[usize],
    reps: usize,
) -> Result<Vec<SizeMeasurement>, RunnerError>
where
    T: bench_runner::LineTransport,
    F: FnMut() -> Result<Session<T>, RunnerError>,
{
    let mut out = Vec::with_capacity(sizes.len());
    for &n in sizes {
        // Fresh engine per size: a cold process that has not inherited an earlier size's warm
        // caches. The spawn + hello complete BEFORE the first timer, so the intercept isolates
        // the per-request protocol floor, not the one-time spawn cost.
        let mut session = spawn()?;
        let samples = time_prefill_reps(&mut session, n, reps)?;
        let min_ms = samples.iter().cloned().fold(f64::INFINITY, f64::min);
        let mean_ms = samples.iter().sum::<f64>() / samples.len() as f64;
        out.push(SizeMeasurement {
            n,
            reps,
            min_ms,
            mean_ms,
        });
        // `session` dropped here → ChildStdioTransport kills the child (fresh next size).
    }
    Ok(out)
}

/// Render the per-size table + the fit line + verdict.
fn render_report(measurements: &[SizeMeasurement], fit: &LinearFit) -> String {
    let mut s = String::new();
    s.push_str("prefill-decompose — elapsed_ms = c + m*n\n\n");
    s.push_str(&format!(
        "{:>8}  {:>5}  {:>10}  {:>10}\n",
        "n", "reps", "min_ms", "mean_ms"
    ));
    for m in measurements {
        s.push_str(&format!(
            "{:>8}  {:>5}  {:>10.3}  {:>10.3}\n",
            m.n, m.reps, m.min_ms, m.mean_ms
        ));
    }
    s.push('\n');
    s.push_str(&format!(
        "elapsed_ms = c + m*n (c={:.4}ms, m={:.6}ms/token, R²={:.6})\n",
        fit.intercept_ms, fit.slope_ms_per_token, fit.r_squared
    ));
    s.push_str(&verdict(fit));
    s.push('\n');
    s
}

/// `prefill-decompose` entry point.
pub fn run(args: &[String]) -> ExitCode {
    let parsed = match parse_args(args) {
        Ok(Some(p)) => p,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("benchctl prefill-decompose: {msg}");
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    let weights_str = parsed.weights.to_string_lossy().to_string();
    let engine = parsed.engine.clone();

    // Fresh engine per size (§A fresh-per-phase intent): spawn a new `runtime-worker` child
    // and complete the hello handshake; the hello is discarded (the diagnostic needs only the
    // session). Identical spawn/connect seam as the scored timing path.
    let mut spawn = || -> Result<Session<ChildStdioTransport>, RunnerError> {
        let transport = ChildStdioTransport::spawn(&engine, &weights_str, &[])?;
        let (session, _hello) = Session::connect(transport)?;
        Ok(session)
    };

    let measurements = match measure_sizes(&mut spawn, &parsed.sizes, parsed.reps) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("benchctl prefill-decompose: measurement failed: {e}");
            return ExitCode::from(1);
        }
    };

    let points: Vec<(f64, f64)> = measurements
        .iter()
        .map(|m| (m.n as f64, m.mean_ms))
        .collect();
    let fit = match least_squares_fit(&points) {
        Some(f) => f,
        None => {
            eprintln!(
                "benchctl prefill-decompose: cannot fit a line (need ≥2 distinct sizes; got {})",
                parsed.sizes.len()
            );
            return ExitCode::from(1);
        }
    };

    print!("{}", render_report(&measurements, &fit));
    ExitCode::SUCCESS
}

/// Parse `prefill-decompose` flags. `Ok(None)` means `--help` was requested.
fn parse_args(args: &[String]) -> Result<Option<Args>, String> {
    let mut engine: Option<String> = None;
    let mut weights: Option<PathBuf> = None;
    let mut sizes: Option<Vec<usize>> = None;
    let mut reps: Option<usize> = None;

    fn value<'a>(args: &'a [String], i: usize, name: &str) -> Result<&'a str, String> {
        args.get(i + 1)
            .map(|s| s.as_str())
            .ok_or_else(|| format!("flag {name} requires a value"))
    }

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(None),
            "--engine" => {
                engine = Some(value(args, i, "--engine")?.to_string());
                i += 2;
            }
            "--weights" => {
                weights = Some(PathBuf::from(value(args, i, "--weights")?));
                i += 2;
            }
            "--sizes" => {
                sizes = Some(parse_sizes(value(args, i, "--sizes")?)?);
                i += 2;
            }
            "--reps" => {
                let v = value(args, i, "--reps")?;
                let r: usize = v
                    .parse()
                    .map_err(|_| format!("invalid usize for --reps: {v:?}"))?;
                if r == 0 {
                    return Err("--reps must be positive".to_string());
                }
                reps = Some(r);
                i += 2;
            }
            other => return Err(format!("unknown flag {other:?}")),
        }
    }

    let engine = engine.ok_or("missing required --engine")?;
    let weights = weights.ok_or("missing required --weights")?;
    let sizes = sizes.unwrap_or_else(|| DEFAULT_SIZES.to_vec());
    let reps = reps.unwrap_or(DEFAULT_REPS);

    Ok(Some(Args {
        engine,
        weights,
        sizes,
        reps,
    }))
}

/// Parse `--sizes` as a comma-separated list of positive token counts. At least two DISTINCT
/// sizes are required for the line fit to be determined.
fn parse_sizes(raw: &str) -> Result<Vec<usize>, String> {
    let mut sizes = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let n: usize = part
            .parse()
            .map_err(|_| format!("invalid size in --sizes: {part:?}"))?;
        if n == 0 {
            return Err("--sizes entries must be positive".to_string());
        }
        sizes.push(n);
    }
    if sizes.len() < 2 {
        return Err("--sizes needs at least two sizes for the line fit".to_string());
    }
    let mut distinct = sizes.clone();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() < 2 {
        return Err("--sizes needs at least two DISTINCT sizes for the line fit".to_string());
    }
    Ok(sizes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recover a KNOWN line: points synthesized from c=16, m=0.05 must fit back to
    /// c≈16, m≈0.05, R²≈1.0. This is the core attribution math (RULING A3): a ≈16ms
    /// intercept is the protocol/spawn-floor signature.
    #[test]
    fn fit_recovers_known_line_c16_m005() {
        let c = 16.0;
        let m = 0.05;
        let points: Vec<(f64, f64)> = [128.0, 256.0, 512.0, 1024.0]
            .iter()
            .map(|&n| (n, c + m * n))
            .collect();
        let fit = least_squares_fit(&points).unwrap();
        assert!(
            (fit.intercept_ms - 16.0).abs() < 1e-9,
            "c={}",
            fit.intercept_ms
        );
        assert!(
            (fit.slope_ms_per_token - 0.05).abs() < 1e-12,
            "m={}",
            fit.slope_ms_per_token
        );
        assert!((fit.r_squared - 1.0).abs() < 1e-12, "R²={}", fit.r_squared);
    }

    /// A perfectly collinear set has R² == 1.0 exactly (SS_res == 0).
    #[test]
    fn fit_perfect_line_has_unit_r_squared() {
        // y = 3 + 2x on arbitrary x.
        let points = [(1.0, 5.0), (2.0, 7.0), (4.0, 11.0), (10.0, 23.0)];
        let fit = least_squares_fit(&points).unwrap();
        assert!((fit.intercept_ms - 3.0).abs() < 1e-12);
        assert!((fit.slope_ms_per_token - 2.0).abs() < 1e-12);
        assert!((fit.r_squared - 1.0).abs() < 1e-12);
    }

    /// A near-zero intercept with real slope reads as compute-dominated, not floor.
    #[test]
    fn fit_recovers_zero_intercept_line() {
        let points: Vec<(f64, f64)> = [128.0, 256.0, 512.0, 1024.0]
            .iter()
            .map(|&n| (n, 0.02 * n))
            .collect();
        let fit = least_squares_fit(&points).unwrap();
        assert!(fit.intercept_ms.abs() < 1e-9, "c={}", fit.intercept_ms);
        assert!((fit.slope_ms_per_token - 0.02).abs() < 1e-12);
        assert!(fit.intercept_ms < FLOOR_VERDICT_THRESHOLD_MS);
        assert!(verdict(&fit).contains("per-token-compute dominated"));
    }

    /// Noisy points recover the underlying line approximately with high (but <1) R².
    #[test]
    fn fit_noisy_points_recovers_approx_line_high_r2() {
        let c = 16.0;
        let m = 0.05;
        // Small deterministic perturbations around the true line.
        let noise = [0.3, -0.4, 0.2, -0.1, 0.35, -0.25];
        let ns = [64.0, 128.0, 256.0, 512.0, 768.0, 1024.0];
        let points: Vec<(f64, f64)> = ns
            .iter()
            .zip(noise.iter())
            .map(|(&n, &e)| (n, c + m * n + e))
            .collect();
        let fit = least_squares_fit(&points).unwrap();
        assert!(
            (fit.intercept_ms - 16.0).abs() < 1.0,
            "c={}",
            fit.intercept_ms
        );
        assert!((fit.slope_ms_per_token - 0.05).abs() < 0.01);
        assert!(
            fit.r_squared > 0.99 && fit.r_squared <= 1.0,
            "R²={}",
            fit.r_squared
        );
        // 16ms intercept → floor verdict.
        assert!(fit.intercept_ms >= FLOOR_VERDICT_THRESHOLD_MS);
        assert!(verdict(&fit).contains("protocol/spawn overhead"));
    }

    /// A horizontal line (all y equal) is a valid fit: slope 0, R² defined as 1.0.
    #[test]
    fn fit_horizontal_line_defined_r_squared() {
        let points = [(1.0, 7.0), (2.0, 7.0), (3.0, 7.0)];
        let fit = least_squares_fit(&points).unwrap();
        assert!(fit.slope_ms_per_token.abs() < 1e-12);
        assert!((fit.intercept_ms - 7.0).abs() < 1e-12);
        assert!((fit.r_squared - 1.0).abs() < 1e-12);
    }

    /// Fit is undetermined with <2 points or all-equal x.
    #[test]
    fn fit_undetermined_cases_return_none() {
        assert!(least_squares_fit(&[]).is_none());
        assert!(least_squares_fit(&[(1.0, 2.0)]).is_none());
        // All x equal → vertical, no finite slope.
        assert!(least_squares_fit(&[(5.0, 1.0), (5.0, 2.0), (5.0, 9.0)]).is_none());
    }

    #[test]
    fn parse_defaults_when_optional_flags_omitted() {
        let a = parse_args(&[
            "--engine".into(),
            "eng".into(),
            "--weights".into(),
            "w".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(a.engine, "eng");
        assert_eq!(a.weights, PathBuf::from("w"));
        assert_eq!(a.sizes, vec![128, 256, 512, 1024]);
        assert_eq!(a.reps, DEFAULT_REPS);
    }

    #[test]
    fn parse_custom_sizes_and_reps() {
        let a = parse_args(&[
            "--engine".into(),
            "e".into(),
            "--weights".into(),
            "w".into(),
            "--sizes".into(),
            "64, 128 ,256".into(),
            "--reps".into(),
            "4".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(a.sizes, vec![64, 128, 256]);
        assert_eq!(a.reps, 4);
    }

    #[test]
    fn parse_requires_engine_and_weights() {
        assert!(parse_args(&["--engine".into(), "e".into()]).is_err());
        assert!(parse_args(&["--weights".into(), "w".into()]).is_err());
    }

    #[test]
    fn parse_rejects_bad_sizes_and_reps() {
        let base = ["--engine", "e", "--weights", "w"].map(String::from);
        let with = |extra: &[&str]| {
            let mut v = base.to_vec();
            v.extend(extra.iter().map(|s| s.to_string()));
            v
        };
        // Single size can't fit a line.
        assert!(parse_args(&with(&["--sizes", "128"])).is_err());
        // Two identical sizes are not distinct.
        assert!(parse_args(&with(&["--sizes", "128,128"])).is_err());
        // Zero / non-numeric.
        assert!(parse_args(&with(&["--sizes", "0,128"])).is_err());
        assert!(parse_args(&with(&["--sizes", "a,b"])).is_err());
        assert!(parse_args(&with(&["--reps", "0"])).is_err());
        assert!(parse_args(&with(&["--reps", "x"])).is_err());
    }

    #[test]
    fn parse_help_returns_none() {
        assert!(parse_args(&["--help".into()]).unwrap().is_none());
        assert!(parse_args(&["-h".into()]).unwrap().is_none());
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        assert!(parse_args(&["--bogus".into()]).is_err());
    }

    /// The rendered report carries the table header, the fit line with c/m/R², and a verdict.
    #[test]
    fn render_report_contains_table_fit_and_verdict() {
        let measurements = vec![
            SizeMeasurement {
                n: 128,
                reps: 8,
                min_ms: 16.5,
                mean_ms: 16.8,
            },
            SizeMeasurement {
                n: 1024,
                reps: 8,
                min_ms: 61.0,
                mean_ms: 61.6,
            },
        ];
        let fit = least_squares_fit(&[(128.0, 16.8), (1024.0, 61.6)]).unwrap();
        let report = render_report(&measurements, &fit);
        assert!(report.contains("mean_ms"));
        assert!(report.contains("128"));
        assert!(report.contains("1024"));
        assert!(report.contains("elapsed_ms = c + m*n"));
        assert!(report.contains("R²="));
        assert!(report.contains("VERDICT:"));
    }
}
