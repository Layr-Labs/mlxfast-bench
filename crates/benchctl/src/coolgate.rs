//! Local-mode GPU cool-down gate — a byte-for-byte port of benchmark.sh's
//! `run_local_cool_gate` (the thermal helper Swift `runLocalPhaseCoolGate` dispatches to,
//! `QwenRuntimeLocalIterate.swift:708` prefill / `:754` decode). Before EACH timed phase of
//! a local run, block until the GPU has cooled to the gate temperature, so back-to-back
//! timings are not measured on a hot (throttled) or sequencing-warmed GPU.
//!
//! Semantics match benchmark.sh exactly:
//! - gate temp 40 C; poll 10 s; abort floor 180 s; stall window 90 s; hard ceiling 900 s;
//!   progress epsilon 0.25 C (a new minimum must drop at least this much to count).
//! - reader: `MLXFAST_GPU_TEMP_CMD` (a shell command printing Celsius) else `macmon pipe -s1`
//!   → `.temp.gpu_temp_avg`; macmon discovered via `MLXFAST_MACMON_BIN`, then PATH, then the
//!   homebrew/`~/bin` candidates.
//! - missing reader / repeated unusable samples → SKIP (warn, never fail); a hot GPU that is
//!   NOT trending down (stall) or that never reaches the gate (ceiling) → ABORT (error), so a
//!   scripted loop stops instead of measuring a loaded GPU.
//! - `MLXFAST_LOCAL_COOL_GATE=0` disables the gate (with a not-comparable warning).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use bench_runner::RunnerError;

// Constants — identical to benchmark.sh COOL_GATE_* (lines 18-24).
const TEMP_C: f64 = 40.0;
const POLL_SECONDS: u64 = 10;
const ABORT_SECONDS: u64 = 180;
const STALL_SECONDS: u64 = 90;
const MAX_WAIT_SECONDS: u64 = 900;
const PROGRESS_EPSILON_C: f64 = 0.25;

/// Outcome of the poll loop (the pure core, independent of subprocess/sleep).
#[derive(Debug, PartialEq)]
pub enum CoolGateOutcome {
    /// GPU reached the gate temperature after `waited` seconds.
    Passed { waited: u64 },
    /// The gate was skipped (unusable/absent reader) — never a failure.
    Skipped(String),
}

/// The pure poll loop at the FIXED wrapper cool-gate constant [`TEMP_C`] (finding R21 — the live
/// thermal thresholds are READONLY wrapper constants `readonly GATE_TEMP=40`, NOT a contract/env
/// value): `read_temp` yields the current GPU temp in C (or `None` for an unusable sample);
/// `sleep` waits the given seconds. Returns `Err` on a stall/ceiling abort. Mirrors benchmark.sh
/// `run_local_cool_gate`'s loop 1:1 (waited is a logical counter incremented by the poll interval,
/// exactly as the shell tracks it). The gate temperature is FIXED, never parameterized — the live
/// track fixture carries no threshold fields and env/contract overrides are ignored.
pub fn cool_gate_loop<R, S>(mut read_temp: R, mut sleep: S) -> Result<CoolGateOutcome, String>
where
    R: FnMut() -> Option<f64>,
    S: FnMut(u64),
{
    let mut waited: u64 = 0;
    let mut min_temp: Option<f64> = None;
    let mut last_progress_waited: u64 = 0;
    let mut bad_samples: u32 = 0;

    loop {
        let temp = match read_temp() {
            Some(t) if t.is_finite() => t,
            _ => {
                bad_samples += 1;
                if bad_samples >= 3 {
                    return Ok(CoolGateOutcome::Skipped(
                        "temperature reader returned no usable sample".to_string(),
                    ));
                }
                sleep(2);
                continue;
            }
        };
        bad_samples = 0;

        if temp <= TEMP_C {
            return Ok(CoolGateOutcome::Passed { waited });
        }

        // Progress: only a new minimum at least EPSILON below the previous one counts, so
        // sensor jitter around a plateau does not look like cooling.
        if min_temp.is_none_or(|m| temp <= m - PROGRESS_EPSILON_C) {
            min_temp = Some(temp);
            last_progress_waited = waited;
        }

        // Abort: hot AND not trending down (external GPU load) — more waiting won't help.
        if waited >= ABORT_SECONDS && waited - last_progress_waited >= STALL_SECONDS {
            return Err(format!(
                "GPU is hot and not cooling down (current {:.1}C, min seen {:.1}C, target <={:.0}C, waited {}s); something else is loading the GPU",
                temp,
                min_temp.unwrap_or(temp),
                TEMP_C,
                waited
            ));
        }
        // Hard ceiling: do not stall the loop past the ranked runner's cool timeout.
        if waited >= MAX_WAIT_SECONDS {
            return Err(format!(
                "GPU did not reach {:.0}C within {}s (current {:.1}C); reduce GPU load or ambient heat",
                TEMP_C, MAX_WAIT_SECONDS, temp
            ));
        }

        sleep(POLL_SECONDS);
        waited += POLL_SECONDS;
    }
}

/// A resolved GPU temperature reader.
enum TempReader {
    /// `MLXFAST_GPU_TEMP_CMD`: a shell command whose stdout is a Celsius number.
    Cmd(String),
    /// A macmon binary; temperature read from `macmon pipe -s1` → `.temp.gpu_temp_avg`.
    Macmon(PathBuf),
}

/// Resolve the temperature reader, mirroring benchmark.sh `find_macmon` + the
/// `MLXFAST_GPU_TEMP_CMD` seam. `None` means no reader is available (→ skip the gate).
fn resolve_temp_reader() -> Option<TempReader> {
    if let Ok(cmd) = std::env::var("MLXFAST_GPU_TEMP_CMD") {
        if !cmd.is_empty() {
            return Some(TempReader::Cmd(cmd));
        }
    }
    // MLXFAST_MACMON_BIN (must be executable), then PATH, then known install locations.
    if let Ok(bin) = std::env::var("MLXFAST_MACMON_BIN") {
        if !bin.is_empty() {
            let p = PathBuf::from(&bin);
            if is_executable(&p) {
                return Some(TempReader::Macmon(p));
            }
            eprintln!("benchctl: MLXFAST_MACMON_BIN is set but not executable: {bin}");
            return None;
        }
    }
    if let Some(p) = which("macmon") {
        return Some(TempReader::Macmon(p));
    }
    for cand in [
        "/opt/homebrew/bin/macmon",
        "/usr/local/bin/macmon",
        &format!("{}/bin/macmon", std::env::var("HOME").unwrap_or_default()),
    ] {
        let p = PathBuf::from(cand);
        if is_executable(&p) {
            return Some(TempReader::Macmon(p));
        }
    }
    None
}

fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        let cand = PathBuf::from(dir).join(name);
        if is_executable(&cand) {
            return Some(cand);
        }
    }
    None
}

/// Read one GPU temperature sample (C) from the reader, or `None` on an unusable sample.
fn read_temp(reader: &TempReader) -> Option<f64> {
    let stdout = match reader {
        TempReader::Cmd(cmd) => {
            Command::new("bash")
                .arg("-c")
                .arg(cmd)
                .output()
                .ok()?
                .stdout
        }
        TempReader::Macmon(bin) => {
            Command::new(bin)
                .arg("pipe")
                .arg("-s1")
                .output()
                .ok()?
                .stdout
        }
    };
    let text = String::from_utf8_lossy(&stdout);
    match reader {
        TempReader::Cmd(_) => text.lines().next()?.trim().parse::<f64>().ok(),
        TempReader::Macmon(_) => {
            // macmon pipe emits one JSON object per line; take the first with a gpu temp.
            for line in text.lines() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(t) = v
                        .get("temp")
                        .and_then(|t| t.get("gpu_temp_avg"))
                        .and_then(|t| t.as_f64())
                    {
                        return Some(t);
                    }
                }
            }
            None
        }
    }
}

/// The recorded state of the per-phase cool-down gate (measure-job finding 1): whether the
/// gate `Fired` (GPU was already at/below the gate temp, passed with no wait), `Waited` (the
/// gate blocked until the GPU cooled), or was `SkippedNoReader` (no temperature reader
/// available — the documented skip; a HOT GPU that cannot cool is never silently skipped, it
/// aborts). Recorded verbatim in the measure-job `results.json` so the run carries provenance
/// of whether thermal enforcement actually happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    Fired,
    Waited,
    SkippedNoReader,
}

impl GateState {
    pub fn as_str(self) -> &'static str {
        match self {
            GateState::Fired => "fired",
            GateState::Waited => "waited",
            GateState::SkippedNoReader => "skipped-no-reader",
        }
    }
}
// R15 — the per-phase gate-state FOLD (`GateState::fold` / `severity`) was removed: a leg now runs
// ONE `mtp-timed` verb with ONE cool gate, so there is a single gate state per leg to record —
// there is no longer a prefill+decode pair of states to fold into one.

/// Run the cool gate before a timed `phase` and REPORT its resolved state (measure-job
/// finding 1). Uses the [`resolve_temp_reader`] discovery seam (never a hardcoded `/opt`
/// macmon path) and does NOT swallow an abort into a silent pass: a stall/ceiling abort
/// returns a TYPED [`RunnerError::GateRejected`] (the one-gated-retry class for the pair
/// loop), never `Ok`. The disabled / no-reader SKIP is the only path that returns without
/// enforcing, and it is recorded as `SkippedNoReader` rather than silently "passed".
pub fn cool_gate_report(phase: &str) -> Result<GateState, RunnerError> {
    if std::env::var("MLXFAST_LOCAL_COOL_GATE").ok().as_deref() == Some("0") {
        eprintln!(
            "benchctl: {phase} cool gate disabled (MLXFAST_LOCAL_COOL_GATE=0); hot-start timings are not comparable to gated runs"
        );
        return Ok(GateState::SkippedNoReader);
    }
    let reader = match resolve_temp_reader() {
        Some(r) => r,
        None => {
            eprintln!(
                "benchctl: skipping the {phase} GPU cool-down gate: no temperature reader (install macmon, set MLXFAST_MACMON_BIN, or set MLXFAST_GPU_TEMP_CMD)"
            );
            return Ok(GateState::SkippedNoReader);
        }
    };
    eprintln!("benchctl: {phase} cool gate: waiting for GPU <= {TEMP_C:.0}C before timing...");
    match cool_gate_loop(
        || read_temp(&reader),
        |secs| std::thread::sleep(Duration::from_secs(secs)),
    ) {
        Ok(CoolGateOutcome::Passed { waited }) => {
            eprintln!(
                "benchctl: {phase} cool gate passed (waited {waited}s, target <={TEMP_C:.0}C)"
            );
            // waited==0 ⇒ already cool (fired without blocking); waited>0 ⇒ blocked to cool.
            Ok(if waited == 0 {
                GateState::Fired
            } else {
                GateState::Waited
            })
        }
        Ok(CoolGateOutcome::Skipped(why)) => {
            eprintln!("benchctl: {phase} cool gate skipped: {why}");
            Ok(GateState::SkippedNoReader)
        }
        // A stall/ceiling abort is a TYPED gate rejection (the retry class), NOT swallowed.
        Err(e) => Err(RunnerError::GateRejected {
            phase: phase.to_string(),
            reason: e,
        }),
    }
}

/// Run the cool gate before a timed `phase` ("prefill"/"decode"). Returns `Ok(())` on pass
/// or skip; `Err` on a stall/ceiling abort (which fails the timed run, as benchmark.sh
/// `exit 1` aborts the benchmark). This is the closure benchctl threads into the local
/// fresh-per-phase timing path, and the body of the `--local-cool-gate-only` helper. Shares
/// [`cool_gate_report`]'s discovery + fail-closed abort, discarding the recorded state.
pub fn cool_gate(phase: &str) -> Result<(), RunnerError> {
    cool_gate_report(phase).map(|_state| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A temp reader driven by a fixed sequence; the last value repeats once exhausted.
    fn seq(values: Vec<Option<f64>>) -> impl FnMut() -> Option<f64> {
        let i = Rc::new(RefCell::new(0usize));
        move || {
            let mut idx = i.borrow_mut();
            let v = values.get(*idx).copied().unwrap_or(*values.last().unwrap());
            *idx += 1;
            v
        }
    }

    #[test]
    fn passes_immediately_when_already_cool() {
        let r = cool_gate_loop(seq(vec![Some(38.0)]), |_| {});
        assert_eq!(r, Ok(CoolGateOutcome::Passed { waited: 0 }));
    }

    #[test]
    fn waits_then_passes_as_gpu_cools() {
        // 50 -> 45 -> 41 (all hot) -> 39 (<=40): 3 polls of 10s = waited 30.
        let slept = Rc::new(RefCell::new(0u64));
        let s = slept.clone();
        let r = cool_gate_loop(
            seq(vec![Some(50.0), Some(45.0), Some(41.0), Some(39.0)]),
            move |secs| *s.borrow_mut() += secs,
        );
        assert_eq!(r, Ok(CoolGateOutcome::Passed { waited: 30 }));
        assert_eq!(*slept.borrow(), 30);
    }

    #[test]
    fn aborts_on_stall_hot_and_not_cooling() {
        // Always 50C: min set at waited 0; at waited>=180 and no progress for >=90 -> abort.
        let r = cool_gate_loop(seq(vec![Some(50.0)]), |_| {});
        assert!(matches!(r, Err(ref m) if m.contains("not cooling down")));
    }

    #[test]
    fn aborts_on_ceiling_even_while_slowly_cooling() {
        // Cools by >=EPSILON every poll (always progress, so the stall path never fires) but
        // never reaches 40 within 900s -> hard ceiling abort. Start high enough to stay >40.
        let mut t = 1000.0_f64;
        let reader = move || {
            t -= 0.3; // > EPSILON (0.25) each poll => always "progress", never stalls
            Some(t)
        };
        let r = cool_gate_loop(reader, |_| {});
        assert!(matches!(r, Err(ref m) if m.contains("did not reach")));
    }

    #[test]
    fn skips_after_three_unusable_samples() {
        let r = cool_gate_loop(seq(vec![None, None, None]), |_| {});
        assert!(matches!(r, Ok(CoolGateOutcome::Skipped(_))));
    }

    #[test]
    fn tolerates_one_bad_sample_then_passes() {
        // A single unusable read (sleep 2, no waited bump), then a cool read passes at waited 0.
        let r = cool_gate_loop(seq(vec![None, Some(35.0)]), |_| {});
        assert_eq!(r, Ok(CoolGateOutcome::Passed { waited: 0 }));
    }

    #[test]
    fn gate_enforces_the_fixed_40c_wrapper_constant_r21() {
        // finding R21 — the cool-gate temperature is the FIXED wrapper constant 40C, never a
        // contract/env value. A steady 41C (just above 40) stays hot and aborts against the fixed
        // gate; a steady 40C is at the boundary and passes immediately. There is no parameter to
        // raise the gate — the readonly GATE_TEMP=40 is the only threshold the loop enforces.
        assert_eq!(
            cool_gate_loop(seq(vec![Some(40.0)]), |_| {}),
            Ok(CoolGateOutcome::Passed { waited: 0 }),
            "40C is at the fixed 40C gate boundary → pass"
        );
        assert_eq!(
            cool_gate_loop(seq(vec![Some(41.0)]), |_| {}),
            Err("GPU is hot and not cooling down (current 41.0C, min seen 41.0C, target <=40C, waited 180s); something else is loading the GPU".to_string()),
            "41C stays hot against the FIXED 40C gate (no contract/env override can raise it)"
        );
    }
}
