//! Local-mode GPU cool-down gate — a byte-for-byte port of benchmark.sh's
//! `run_local_cool_gate` (the thermal helper Swift `runLocalPhaseCoolGate` dispatches to,
//! `QwenRuntimeLocalIterate.swift:708` prefill / `:754` decode). Before EACH timed phase of
//! a local run, block until the GPU has cooled to the gate temperature, so back-to-back
//! timings are not measured on a hot (throttled) or sequencing-warmed GPU.
//!
//! Semantics match benchmark.sh exactly, except the gate temperature (see below):
//! - gate temp: the run platform's cool-gate temperature
//!   ([`bench_core::constants::Platform::cool_gate_temp_c`]) — Mac/MLX 40 C, GB10/CUDA 50 C.
//!   Finding R21 originally froze this at a single non-parameterizable 40 C constant; David
//!   2026-08-30 ("no adversarial hardening") ruled that rigidity out of scope, because the GB10
//!   GPU idles at 40–43 C and a 40 C gate would refuse forever. The threshold is now a trusted
//!   per-platform value; it is NOT a contract/candidate input.
//! - poll 10 s; abort floor 180 s; stall window 90 s; hard ceiling 900 s;
//!   progress epsilon 0.25 C (a new minimum must drop at least this much to count).
//! - reader: resolved NATIVELY per host OS, so NO env var is required on either platform —
//!   on Linux (GB10) `nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits`
//!   (first line parsed as Celsius); on macOS `macmon pipe -s1` → `.temp.gpu_temp_avg`.
//!   `MLXFAST_GPU_TEMP_CMD` (a shell command printing Celsius) and `MLXFAST_MACMON_BIN` remain
//!   OPTIONAL overrides that WIN when set; they are never required. macmon is discovered via
//!   `MLXFAST_MACMON_BIN`, then PATH, then the homebrew/`~/bin` candidates; nvidia-smi via PATH
//!   then the usual `/usr/bin` install locations. The reader identity actually used is reported by
//!   [`cool_gate_report`] (`GateState`) alongside the platform threshold source.
//! - missing reader / repeated unusable samples → SKIP (warn, never fail); a hot GPU that is
//!   NOT trending down (stall) or that never reaches the gate (ceiling) → ABORT (error), so a
//!   scripted loop stops instead of measuring a loaded GPU.
//! - `MLXFAST_LOCAL_COOL_GATE=0` disables the gate (with a not-comparable warning).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use bench_core::constants::Platform;
use bench_runner::RunnerError;

// Constants — identical to benchmark.sh COOL_GATE_* (lines 18-24), EXCEPT the gate temperature,
// which is the run platform's trusted per-platform value ([`Platform::cool_gate_temp_c`]) rather
// than a fixed 40 C — R21 lift, David 2026-08-30 (see the module doc).
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

/// The pure poll loop at the cool-gate temperature `gate_temp` (C): `read_temp` yields the current
/// GPU temp in C (or `None` for an unusable sample); `sleep` waits the given seconds. Returns `Err`
/// on a stall/ceiling abort. Mirrors benchmark.sh `run_local_cool_gate`'s loop 1:1 (waited is a
/// logical counter incremented by the poll interval, exactly as the shell tracks it).
///
/// `gate_temp` is the run platform's trusted per-platform threshold
/// ([`Platform::cool_gate_temp_c`]) — Mac/MLX 40 C, GB10/CUDA 50 C. Finding R21 froze this at a
/// non-parameterizable 40 C; David 2026-08-30 ("no adversarial hardening") lifted that rigidity so
/// the GB10 idle (40–43 C) does not refuse forever against a 40 C gate. It is a per-platform
/// value, NOT a contract/candidate input: the loop still ABORTS a genuinely hot GPU above
/// `gate_temp` — re-siting the threshold does not defang the gate.
#[cfg(test)]
pub fn cool_gate_loop<R, S>(
    gate_temp: f64,
    read_temp: R,
    sleep: S,
) -> Result<CoolGateOutcome, String>
where
    R: FnMut() -> Option<f64>,
    S: FnMut(u64),
{
    cool_gate_loop_with_progress(gate_temp, read_temp, sleep, |_, _, _| {})
}

/// The same gate with an observation-only callback, called on every usable sample.
fn cool_gate_loop_with_progress<R, S, P>(
    gate_temp: f64,
    mut read_temp: R,
    mut sleep: S,
    mut progress: P,
) -> Result<CoolGateOutcome, String>
where
    R: FnMut() -> Option<f64>,
    S: FnMut(u64),
    P: FnMut(u64, f64, f64),
{
    let mut waited: u64 = 0;
    let mut min_temp: Option<f64> = None;
    let mut observed_min: Option<f64> = None;
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
        let min_seen = observed_min.map_or(temp, |min| min.min(temp));
        observed_min = Some(min_seen);
        progress(waited, temp, min_seen);

        if temp <= gate_temp {
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
                gate_temp,
                waited
            ));
        }
        // Hard ceiling: do not stall the loop past the ranked runner's cool timeout.
        if waited >= MAX_WAIT_SECONDS {
            return Err(format!(
                "GPU did not reach {:.0}C within {}s (current {:.1}C); reduce GPU load or ambient heat",
                gate_temp, MAX_WAIT_SECONDS, temp
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
    /// A macmon binary (macOS native); temperature read from `macmon pipe -s1` →
    /// `.temp.gpu_temp_avg`.
    Macmon(PathBuf),
    /// An `nvidia-smi` binary (Linux/GB10 native); temperature read from
    /// `nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits` (first line, C).
    NvidiaSmi(PathBuf),
}

impl TempReader {
    /// The stable provenance label for the reader ACTUALLY resolved — recorded in the run seal so
    /// the artifact names the real reader, not a generic one.
    fn source_label(&self) -> &'static str {
        match self {
            TempReader::Cmd(_) => "gpu-temp-cmd",
            TempReader::Macmon(_) => "macmon",
            TempReader::NvidiaSmi(_) => "nvidia-smi",
        }
    }
}

/// The host OS families whose NATIVE cool-gate reader differs. Kept as an explicit value (not a
/// bare `cfg!`) so the per-platform reader dispatch ([`resolve_temp_reader_for`]) is unit-testable
/// for BOTH platforms regardless of the host the tests run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostOs {
    Linux,
    MacOs,
    Other,
}

/// The compile-time host OS.
fn host_os() -> HostOs {
    if cfg!(target_os = "linux") {
        HostOs::Linux
    } else if cfg!(target_os = "macos") {
        HostOs::MacOs
    } else {
        HostOs::Other
    }
}

/// Resolve the temperature reader NATIVELY for the host OS: Linux (GB10) → `nvidia-smi`, macOS →
/// `macmon` — so NO env var is required on either platform. The `MLXFAST_GPU_TEMP_CMD` and
/// `MLXFAST_MACMON_BIN` seams remain OPTIONAL overrides that WIN when set. `None` means no reader
/// is available (→ skip the gate, fail-closed).
fn resolve_temp_reader() -> Option<TempReader> {
    resolve_temp_reader_for(host_os())
}

/// The pure OS→reader dispatch behind [`resolve_temp_reader`] (`os` passed in so both platform
/// branches are testable on any host). Optional overrides win first; then the platform default.
fn resolve_temp_reader_for(os: HostOs) -> Option<TempReader> {
    // 1. Explicit command override wins on EVERY platform (never required).
    if let Ok(cmd) = std::env::var("MLXFAST_GPU_TEMP_CMD") {
        if !cmd.is_empty() {
            return Some(TempReader::Cmd(cmd));
        }
    }
    // 2. Explicit macmon override wins next — it names macmon, so honor it on any host.
    if let Ok(bin) = std::env::var("MLXFAST_MACMON_BIN") {
        if !bin.is_empty() {
            let p = PathBuf::from(&bin);
            if is_executable(&p) {
                return Some(TempReader::Macmon(p));
            }
            eprintln!("benchd: MLXFAST_MACMON_BIN is set but not executable: {bin}");
            return None;
        }
    }
    // 3. Native per-platform default — no env var needed.
    match os {
        HostOs::Linux => resolve_nvidia_smi(),
        HostOs::MacOs => resolve_macmon(),
        // Unknown host: try both natives rather than refusing outright.
        HostOs::Other => resolve_macmon().or_else(resolve_nvidia_smi),
    }
}

/// Discover a macmon binary: PATH, then the homebrew / `~/bin` install locations.
fn resolve_macmon() -> Option<TempReader> {
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

/// Discover an nvidia-smi binary: PATH, then the usual `/usr/bin` install locations.
fn resolve_nvidia_smi() -> Option<TempReader> {
    if let Some(p) = which("nvidia-smi") {
        return Some(TempReader::NvidiaSmi(p));
    }
    for cand in [
        "/usr/bin/nvidia-smi",
        "/usr/local/bin/nvidia-smi",
        "/opt/bin/nvidia-smi",
    ] {
        let p = PathBuf::from(cand);
        if is_executable(&p) {
            return Some(TempReader::NvidiaSmi(p));
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
        TempReader::NvidiaSmi(bin) => {
            Command::new(bin)
                .arg("--query-gpu=temperature.gpu")
                .arg("--format=csv,noheader,nounits")
                .output()
                .ok()?
                .stdout
        }
    };
    let text = String::from_utf8_lossy(&stdout);
    match reader {
        // A plain-number reader: the first line is the Celsius value. nvidia-smi prints one line
        // per GPU with `noheader,nounits`, so the first line is GPU 0's temperature.
        TempReader::Cmd(_) | TempReader::NvidiaSmi(_) => {
            text.lines().next()?.trim().parse::<f64>().ok()
        }
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
    /// The sealed spelling of the state, as the measure-job per-pair record carries it.
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
///
/// `platform` keys the gate temperature ([`Platform::cool_gate_temp_c`]): Mac/MLX 40 C,
/// GB10/CUDA 50 C (R21 lift, David 2026-08-30). It is the run's resolved platform, never a
/// contract/candidate value.
pub fn cool_gate_report(phase: &str, platform: Platform) -> Result<GateState, RunnerError> {
    let gate_temp = platform.cool_gate_temp_c();
    if std::env::var("MLXFAST_LOCAL_COOL_GATE").ok().as_deref() == Some("0") {
        eprintln!(
            "benchd: {phase} cool gate disabled (MLXFAST_LOCAL_COOL_GATE=0); hot-start timings are not comparable to gated runs"
        );
        return Ok(GateState::SkippedNoReader);
    }
    let reader = match resolve_temp_reader() {
        Some(r) => r,
        None => {
            eprintln!(
                "benchd: skipping the {phase} GPU cool-down gate: no temperature reader (install macmon, set MLXFAST_MACMON_BIN, or set MLXFAST_GPU_TEMP_CMD)"
            );
            return Ok(GateState::SkippedNoReader);
        }
    };
    eprintln!(
        "benchd: {phase} cool gate ({} platform, reader {}): waiting for GPU <= {gate_temp:.0}C before timing...",
        platform.key(),
        reader.source_label()
    );
    let started = std::time::Instant::now();
    match cool_gate_loop_with_progress(
        gate_temp,
        || read_temp(&reader),
        |secs| std::thread::sleep(Duration::from_secs(secs)),
        |waited, temp, min_temp| {
            eprintln!(
                "benchd: {phase} cooling: GPU {temp:.1}C, min {min_temp:.1}C, target <={gate_temp:.0}C, elapsed {:.0}s (gate wait {waited}s)",
                started.elapsed().as_secs_f64()
            );
        },
    ) {
        Ok(CoolGateOutcome::Passed { waited }) => {
            eprintln!(
                "benchd: {phase} cool gate passed (waited {waited}s, target <={gate_temp:.0}C)"
            );
            // waited==0 ⇒ already cool (fired without blocking); waited>0 ⇒ blocked to cool.
            Ok(if waited == 0 {
                GateState::Fired
            } else {
                GateState::Waited
            })
        }
        Ok(CoolGateOutcome::Skipped(why)) => {
            eprintln!("benchd: {phase} cool gate skipped: {why}");
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
/// `exit 1` aborts the benchmark). This is the closure benchd threads into the local
/// fresh-per-phase timing path, and the body of the `--local-cool-gate-only` helper. Shares
/// [`cool_gate_report`]'s discovery + fail-closed abort, discarding the recorded state. `platform`
/// keys the gate temperature ([`Platform::cool_gate_temp_c`]).
pub fn cool_gate(phase: &str, platform: Platform) -> Result<(), RunnerError> {
    cool_gate_report(phase, platform).map(|_state| ())
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

    // The two platform gate temperatures, so the tests bind to the real per-platform constants.
    fn mlx() -> f64 {
        Platform::Mlx.cool_gate_temp_c() // 40 C
    }
    fn cuda() -> f64 {
        Platform::Cuda.cool_gate_temp_c() // 50 C
    }

    #[test]
    fn progress_reports_samples_without_changing_the_gate() {
        let mut samples = Vec::new();
        let temperatures = vec![Some(50.0), Some(45.0), Some(46.0), Some(40.0)];
        let result = cool_gate_loop_with_progress(
            mlx(),
            seq(temperatures.clone()),
            |_| {},
            |waited, temp, min| samples.push((waited, temp, min)),
        );
        assert_eq!(result, cool_gate_loop(mlx(), seq(temperatures), |_| {}));
        assert_eq!(
            samples,
            vec![(0, 50.0, 50.0), (10, 45.0, 45.0), (20, 46.0, 45.0), (30, 40.0, 40.0)],
        );
    }

    #[test]
    fn progress_reports_the_final_hot_sample_before_stall_abort() {
        let mut samples = Vec::new();
        let result = cool_gate_loop_with_progress(
            mlx(),
            seq(vec![Some(50.0)]),
            |_| {},
            |waited, temp, _| samples.push((waited, temp)),
        );
        assert!(result.unwrap_err().contains("not cooling down"));
        assert_eq!(samples.last(), Some(&(180, 50.0)));
    }

    #[test]
    fn passes_immediately_when_already_cool() {
        let r = cool_gate_loop(mlx(), seq(vec![Some(38.0)]), |_| {});
        assert_eq!(r, Ok(CoolGateOutcome::Passed { waited: 0 }));
    }

    #[test]
    fn waits_then_passes_as_gpu_cools() {
        // 50 -> 45 -> 41 (all hot) -> 39 (<=40): 3 polls of 10s = waited 30.
        let slept = Rc::new(RefCell::new(0u64));
        let s = slept.clone();
        let r = cool_gate_loop(
            mlx(),
            seq(vec![Some(50.0), Some(45.0), Some(41.0), Some(39.0)]),
            move |secs| *s.borrow_mut() += secs,
        );
        assert_eq!(r, Ok(CoolGateOutcome::Passed { waited: 30 }));
        assert_eq!(*slept.borrow(), 30);
    }

    #[test]
    fn aborts_on_stall_hot_and_not_cooling() {
        // Always 50C: min set at waited 0; at waited>=180 and no progress for >=90 -> abort.
        let r = cool_gate_loop(mlx(), seq(vec![Some(50.0)]), |_| {});
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
        let r = cool_gate_loop(mlx(), reader, |_| {});
        assert!(matches!(r, Err(ref m) if m.contains("did not reach")));
    }

    #[test]
    fn skips_after_three_unusable_samples() {
        let r = cool_gate_loop(mlx(), seq(vec![None, None, None]), |_| {});
        assert!(matches!(r, Ok(CoolGateOutcome::Skipped(_))));
    }

    #[test]
    fn tolerates_one_bad_sample_then_passes() {
        // A single unusable read (sleep 2, no waited bump), then a cool read passes at waited 0.
        let r = cool_gate_loop(mlx(), seq(vec![None, Some(35.0)]), |_| {});
        assert_eq!(r, Ok(CoolGateOutcome::Passed { waited: 0 }));
    }

    // ---- R21 LIFT (David 2026-08-30 "no adversarial hardening") ---------------------------------
    // Finding R21 previously FROZE the gate at a single, non-parameterizable 40 C constant and a
    // test asserted "no contract/env override can raise it". David ruled that rigidity out of
    // scope: on GB10 the GPU IDLES at 40–43 C (throttle T.Limit 55 C), so a 40 C gate refuses
    // forever. The threshold is now a trusted PER-PLATFORM value (Mac/MLX 40 C, GB10/CUDA 50 C).
    // The tests below prove the change only RE-SITES the threshold (40→50 for GB10) — it does NOT
    // defang the gate: a genuinely hot box above the platform gate still aborts, and an unreadable
    // reader still fails closed (never a silent pass).

    #[test]
    fn gate_temp_is_per_platform_mac_40_gb10_50_r21_lift() {
        // 41 C: HOT for Mac (40 gate) → aborts; COOL for GB10 (50 gate) → passes immediately.
        // This is exactly the case R21's frozen 40 C gate made unusable on GB10.
        assert_eq!(
            cool_gate_loop(mlx(), seq(vec![Some(41.0)]), |_| {}),
            Err("GPU is hot and not cooling down (current 41.0C, min seen 41.0C, target <=40C, waited 180s); something else is loading the GPU".to_string()),
            "41C is above the 40C Mac gate → still aborts (Mac behavior unchanged)"
        );
        assert_eq!(
            cool_gate_loop(cuda(), seq(vec![Some(41.0)]), |_| {}),
            Ok(CoolGateOutcome::Passed { waited: 0 }),
            "41C is below the 50C GB10 gate → passes (the R21 lift: GB10 idle no longer refuses)"
        );
        // Boundary: each platform passes AT its own gate temperature (inclusive).
        assert_eq!(
            cool_gate_loop(mlx(), seq(vec![Some(40.0)]), |_| {}),
            Ok(CoolGateOutcome::Passed { waited: 0 })
        );
        assert_eq!(
            cool_gate_loop(cuda(), seq(vec![Some(50.0)]), |_| {}),
            Ok(CoolGateOutcome::Passed { waited: 0 })
        );
    }

    #[test]
    fn gb10_gate_still_refuses_a_genuinely_hot_box_negative_control() {
        // NEGATIVE CONTROL: raising the GB10 gate to 50 must NOT let a truly hot box time. At a
        // steady 52–54 C (above the 50 gate, near the 55 throttle limit) the gate stays hot and
        // aborts — the re-sited gate is not a disabled gate.
        for hot in [Some(52.0), Some(53.5), Some(54.0)] {
            let r = cool_gate_loop(cuda(), seq(vec![hot]), |_| {});
            assert!(
                matches!(r, Err(ref m) if m.contains("not cooling down")),
                "GB10 at {hot:?}C (>50) must refuse, got {r:?}"
            );
        }
    }

    #[test]
    fn gb10_gate_passes_a_cool_box_positive_control() {
        // POSITIVE CONTROL: below the 50 C GB10 gate (e.g. 45 C) the gate passes.
        assert_eq!(
            cool_gate_loop(cuda(), seq(vec![Some(45.0)]), |_| {}),
            Ok(CoolGateOutcome::Passed { waited: 0 })
        );
    }

    #[test]
    fn gb10_gate_fails_closed_when_temp_unreadable() {
        // FAIL-CLOSED: an unreadable reader (three unusable samples) is a SKIP, never a silent
        // pass of a possibly-hot box — the caller records it as SkippedNoReader, and a hot box
        // that never yields a usable sample never reaches the "Passed" branch. The gate does not
        // fabricate a cool reading to let timing proceed.
        let r = cool_gate_loop(cuda(), seq(vec![None, None, None]), |_| {});
        assert!(
            matches!(r, Ok(CoolGateOutcome::Skipped(_))),
            "unreadable reader → Skipped (fail-closed: never Passed), got {r:?}"
        );
        assert!(
            !matches!(r, Ok(CoolGateOutcome::Passed { .. })),
            "an unreadable reader must never silently pass the gate"
        );
    }

    // ---- NATIVE per-platform reader selection (bench-side companion to cudafast#22) -------------
    // No env var is needed on either platform: Linux (GB10) resolves nvidia-smi, macOS resolves
    // macmon. MLXFAST_GPU_TEMP_CMD / MLXFAST_MACMON_BIN remain OPTIONAL overrides that WIN. An
    // unreadable native reader still fails closed (skip, never a silent pass).

    /// Serialize the env-mutating reader tests (PATH / MLXFAST_* are process-global). Sets each
    /// named var (None = remove) for the closure, then restores the prior value.
    fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev: Vec<(String, Option<std::ffi::OsString>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var_os(k)))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        let out = f();
        for (k, v) in prev {
            match v {
                Some(val) => std::env::set_var(&k, val),
                None => std::env::remove_var(&k),
            }
        }
        out
    }

    /// Create a fresh temp dir holding an executable shell script `name` whose body is `body`
    /// (a stand-in nvidia-smi / macmon). Returns the dir (put it on PATH; the caller cleans up).
    fn mock_reader_dir(name: &str, body: &str) -> PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "benchd-coolgate-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        write!(f, "#!/bin/sh\n{body}\n").unwrap();
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&p, perm).unwrap();
        dir
    }

    /// The mock dir PREPENDED to the current PATH — so `which` finds the mock first while every
    /// other binary (git, /bin/sh) stays resolvable for tests running concurrently.
    fn path_with(dir: &std::path::Path) -> String {
        format!(
            "{}:{}",
            dir.display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    #[test]
    fn linux_selects_nvidia_smi_natively_no_env() {
        // GB10/Linux with NO env var: the reader is nvidia-smi (native) and reads a plausible C.
        let dir = mock_reader_dir("nvidia-smi", "echo 47");
        let path = path_with(&dir);
        with_env(
            &[
                ("MLXFAST_GPU_TEMP_CMD", None),
                ("MLXFAST_MACMON_BIN", None),
                ("PATH", Some(path.as_str())),
            ],
            || {
                let r = resolve_temp_reader_for(HostOs::Linux)
                    .expect("Linux resolves nvidia-smi natively, no env var");
                assert_eq!(r.source_label(), "nvidia-smi");
                assert_eq!(read_temp(&r), Some(47.0), "reads Celsius from nvidia-smi");
            },
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn macos_selects_macmon_natively_no_env() {
        // macOS with NO env var: the reader is macmon (unchanged from today).
        let dir = mock_reader_dir("macmon", "echo '{\"temp\":{\"gpu_temp_avg\":41.5}}'");
        let path = path_with(&dir);
        with_env(
            &[
                ("MLXFAST_GPU_TEMP_CMD", None),
                ("MLXFAST_MACMON_BIN", None),
                ("PATH", Some(path.as_str())),
            ],
            || {
                let r = resolve_temp_reader_for(HostOs::MacOs)
                    .expect("macOS resolves macmon natively, no env var");
                assert_eq!(r.source_label(), "macmon");
                assert_eq!(read_temp(&r), Some(41.5), "reads Celsius from macmon JSON");
            },
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_gpu_temp_cmd_override_wins_over_native() {
        // MLXFAST_GPU_TEMP_CMD wins even on Linux, where the native default is nvidia-smi. PATH is
        // left intact so the Cmd's `bash -c` resolves.
        with_env(
            &[
                ("MLXFAST_GPU_TEMP_CMD", Some("echo 33")),
                ("MLXFAST_MACMON_BIN", None),
            ],
            || {
                let r = resolve_temp_reader_for(HostOs::Linux)
                    .expect("the command override resolves a reader");
                assert_eq!(r.source_label(), "gpu-temp-cmd");
                assert_eq!(read_temp(&r), Some(33.0));
            },
        );
    }

    #[test]
    fn explicit_macmon_bin_override_wins_over_native() {
        // MLXFAST_MACMON_BIN wins even on Linux (native would be nvidia-smi). Resolved by absolute
        // path, so PATH need not carry it.
        let dir = mock_reader_dir("my-macmon", "echo '{\"temp\":{\"gpu_temp_avg\":30.0}}'");
        let bin = dir.join("my-macmon");
        with_env(
            &[
                ("MLXFAST_GPU_TEMP_CMD", None),
                ("MLXFAST_MACMON_BIN", bin.to_str()),
            ],
            || {
                let r = resolve_temp_reader_for(HostOs::Linux)
                    .expect("the MLXFAST_MACMON_BIN override resolves a reader");
                assert_eq!(r.source_label(), "macmon");
                assert_eq!(read_temp(&r), Some(30.0));
            },
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_reader_unreadable_fails_closed_never_passes() {
        // FAIL-CLOSED: an nvidia-smi that emits a non-numeric line is an unusable sample → the gate
        // SKIPS (never a silent Passed of a possibly-hot GB10). No fabricated cool reading.
        let dir = mock_reader_dir("nvidia-smi", "echo N/A");
        let path = path_with(&dir);
        with_env(
            &[
                ("MLXFAST_GPU_TEMP_CMD", None),
                ("MLXFAST_MACMON_BIN", None),
                ("PATH", Some(path.as_str())),
            ],
            || {
                let r = resolve_temp_reader_for(HostOs::Linux)
                    .expect("nvidia-smi resolves even though its output is unusable");
                assert_eq!(r.source_label(), "nvidia-smi");
                assert_eq!(
                    read_temp(&r),
                    None,
                    "a non-numeric nvidia-smi line is an unusable sample"
                );
                let outcome = cool_gate_loop(cuda(), || read_temp(&r), |_| {});
                assert!(
                    matches!(outcome, Ok(CoolGateOutcome::Skipped(_))),
                    "unreadable native reader → Skipped (fail-closed), got {outcome:?}"
                );
                assert!(
                    !matches!(outcome, Ok(CoolGateOutcome::Passed { .. })),
                    "an unreadable native reader must never silently pass the gate"
                );
            },
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
