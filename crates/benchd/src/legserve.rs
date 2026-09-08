//! PER-LEG RESIDENT ENGINES for the paired ranked path (coordinator ruling, CUDA engine PR #130).
//!
//! On a platform whose worker HOLDS the model (MLX) benchd already owns the whole residency: it
//! spawns the worker, the worker loads, and the leg's tree is the tree the worker binary came
//! from. Nothing here runs.
//!
//! On a platform whose worker is a thin ADAPTER over a resident engine (CUDA / ds4) the model is
//! owned by a separate process, and a paired run needs TWO of them — one per leg, from that leg's
//! own tree, one at a time. The measure-and-score script used to wrap the WHOLE benchd invocation
//! in `tools/serve-up.sh`, which boots one resident for the whole window; that shape cannot serve
//! two legs, so benchd boots and tears down each leg's resident itself.
//!
//! ## The convention benchd drives, per leg
//!
//! Both commands run with the working directory set to the LEG's own workspace:
//!
//! ```text
//! <workspace>/tools/serve-up.sh --boot --spec <serial|mtp> --draft-len <N> --socket-out <FILE>
//! <workspace>/tools/serve-up.sh --stop --socket <PATH>
//! ```
//!
//! * `--boot` boots exactly ONE resident from that tree, waits until it answers a healthy hello,
//!   writes the resident's Unix-socket path as the first line of `<FILE>`, and exits 0 leaving the
//!   resident running. It owns its own health timeout; benchd waits for it.
//! * `--spec serial --draft-len 0` is what LEG 1 always gets: the control leg is serial regardless
//!   of what the submission declares. Leg 2 gets the run's declared spec.
//! * `--stop` tears that resident down. benchd runs it when the leg's [`LegServe`] is dropped —
//!   after the leg, on success and on failure alike — so a resident never outlives its leg and the
//!   two legs never hold GPU memory at the same time.
//!
//! benchd then puts the socket path into that leg's WORKER SPAWNS ONLY, as
//! [`DS4_RESIDENT_SOCKET_ENV`] and [`BENCH_WORKER_RESIDENT_SOCKET_ENV`] in the child environment.
//! benchd's own environment is never mutated, so the two legs cannot bleed into each other.

use bench_core::constants::Platform;
use bench_protocol::SpecConfig;
use std::path::{Path, PathBuf};

/// The per-leg boot/teardown script, relative to a leg's workspace root.
pub const LEG_SERVE_SCRIPT: &str = "tools/serve-up.sh";
/// The socket name the ds4 adapter connects by.
pub const DS4_RESIDENT_SOCKET_ENV: &str = "DS4_RESIDENT_SOCKET";
/// The socket name the generic bench worker connects by.
pub const BENCH_WORKER_RESIDENT_SOCKET_ENV: &str = "BENCH_WORKER_RESIDENT_SOCKET";

/// EXACT-MATCH refusal names.
pub const LEG_SERVE_SCRIPT_MISSING: &str = "LEG-SERVE-SCRIPT-MISSING";
/// The boot command failed, or produced no socket.
pub const LEG_SERVE_BOOT_FAILED: &str = "LEG-SERVE-BOOT-FAILED";
/// A resident socket was inherited from the environment on a path that boots its own, per leg.
pub const LEG_SERVE_INHERITED_SOCKET: &str = "LEG-SERVE-INHERITED-SOCKET";

/// One leg's resident engine: booted from that leg's tree, torn down when this value is dropped.
#[derive(Debug)]
pub struct LegServe {
    workspace: PathBuf,
    socket: String,
    label: &'static str,
}

impl LegServe {
    /// The child-environment entries this leg's worker spawns must carry.
    pub fn spawn_env(&self) -> Vec<(String, String)> {
        vec![
            (DS4_RESIDENT_SOCKET_ENV.to_string(), self.socket.clone()),
            (
                BENCH_WORKER_RESIDENT_SOCKET_ENV.to_string(),
                self.socket.clone(),
            ),
        ]
    }
}

impl Drop for LegServe {
    fn drop(&mut self) {
        let script = self.workspace.join(LEG_SERVE_SCRIPT);
        let outcome = std::process::Command::new(&script)
            .current_dir(&self.workspace)
            .args(["--stop", "--socket", &self.socket])
            .output();
        match outcome {
            Ok(out) if out.status.success() => {}
            Ok(out) => eprintln!(
                "benchd: the {} leg's resident did not stop cleanly ({}): {}",
                self.label,
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            // Teardown runs on the failure path too, so it REPORTS rather than panics: a leg that
            // already failed must not be replaced in the log by its own cleanup.
            Err(e) => eprintln!(
                "benchd: the {} leg's resident teardown could not run ({}): {e}",
                self.label,
                script.display()
            ),
        }
    }
}

/// Whether this platform's paired legs need benchd to boot a resident per leg: a platform whose
/// worker does NOT hold the model has the weights in a separate process, and that process belongs
/// to one leg's tree.
pub fn leg_serve_required(platform: Platform) -> bool {
    !platform.worker_holds_model()
}

/// The `--spec` / `--draft-len` pair for a leg, from the spec that leg requests. The CONTROL leg
/// passes `None` and is booted serial; a `serial` spec is serial too.
pub fn spec_arguments(spec: Option<&SpecConfig>) -> (String, String) {
    match spec.and_then(|s| s.mtp.as_ref()).and_then(|m| m.depth) {
        Some(depth) if depth >= 1 => ("mtp".to_string(), depth.to_string()),
        _ => ("serial".to_string(), "0".to_string()),
    }
}

/// BOOT one leg's resident from `workspace`, at the spec that leg runs. `label` names the leg in
/// every message ("serial-control" / "candidate").
///
/// Fails CLOSED and BY NAME: a tree with no `tools/serve-up.sh`, a boot that exits non-zero, and a
/// boot that leaves no socket behind are three different refusals with three different names.
pub fn boot_leg(
    workspace: &Path,
    spec: Option<&SpecConfig>,
    label: &'static str,
) -> Result<LegServe, String> {
    let script = workspace.join(LEG_SERVE_SCRIPT);
    if !script.is_file() {
        return Err(format!(
            "{LEG_SERVE_SCRIPT_MISSING}: the {label} leg's tree {} holds no {LEG_SERVE_SCRIPT}, \
             and this platform's worker is an adapter over a resident engine that benchd boots \
             per leg",
            workspace.display()
        ));
    }
    let (spec_mode, draft_len) = spec_arguments(spec);
    let socket_out = std::env::temp_dir().join(format!(
        "benchd-leg-socket.{}.{label}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&socket_out);
    let out = std::process::Command::new(&script)
        .current_dir(workspace)
        .args([
            "--boot",
            "--spec",
            &spec_mode,
            "--draft-len",
            &draft_len,
            "--socket-out",
        ])
        .arg(&socket_out)
        .output()
        .map_err(|e| {
            format!(
                "{LEG_SERVE_BOOT_FAILED}: the {label} leg's boot command {} could not run: {e}",
                script.display()
            )
        })?;
    if !out.status.success() {
        return Err(format!(
            "{LEG_SERVE_BOOT_FAILED}: the {label} leg's resident did not boot ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let socket = std::fs::read_to_string(&socket_out)
        .map_err(|e| {
            format!(
                "{LEG_SERVE_BOOT_FAILED}: the {label} leg's boot reported success but wrote no \
                 socket to {}: {e}",
                socket_out.display()
            )
        })?
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let _ = std::fs::remove_file(&socket_out);
    if socket.is_empty() {
        return Err(format!(
            "{LEG_SERVE_BOOT_FAILED}: the {label} leg's boot wrote an empty socket path"
        ));
    }
    eprintln!(
        "benchd: the {label} leg's resident is up from {} at {socket} ({spec_mode}, draft-len \
         {draft_len})",
        workspace.display()
    );
    Ok(LegServe {
        workspace: workspace.to_path_buf(),
        socket,
        label,
    })
}

/// REFUSE an inherited resident socket on a path that boots one PER LEG. One socket in benchd's
/// own environment means one resident for both legs, which prices the candidate against itself.
pub fn refuse_inherited_socket(
    ds4: Option<&str>,
    bench_worker: Option<&str>,
) -> Result<(), String> {
    for (name, value) in [
        (DS4_RESIDENT_SOCKET_ENV, ds4),
        (BENCH_WORKER_RESIDENT_SOCKET_ENV, bench_worker),
    ] {
        if value.map(str::trim).is_some_and(|v| !v.is_empty()) {
            return Err(format!(
                "{LEG_SERVE_INHERITED_SOCKET}: {name} is set in benchd's environment, and a paired \
                 run boots ONE resident PER LEG from that leg's own tree; do not wrap benchd in \
                 {LEG_SERVE_SCRIPT} — benchd runs it itself, twice"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_adapter_platform_boots_a_resident_per_leg() {
        // MLX's worker holds the model, so benchd's own spawn IS the residency.
        assert!(!leg_serve_required(Platform::Mlx));
        // CUDA's worker is an adapter over a resident that belongs to one leg's tree.
        assert!(leg_serve_required(Platform::Cuda));
    }

    #[test]
    fn the_control_leg_boots_serial_and_the_candidate_boots_its_declared_depth() {
        assert_eq!(
            spec_arguments(None),
            ("serial".to_string(), "0".to_string())
        );
        assert_eq!(
            spec_arguments(Some(&SpecConfig::serial())),
            ("serial".to_string(), "0".to_string())
        );
        assert_eq!(
            spec_arguments(Some(&SpecConfig::mtp(1))),
            ("mtp".to_string(), "1".to_string())
        );
        assert_eq!(
            spec_arguments(Some(&SpecConfig::mtp(4))),
            ("mtp".to_string(), "4".to_string())
        );
    }

    #[test]
    fn a_tree_without_the_boot_script_refuses_by_name() {
        let dir = std::env::temp_dir().join(format!("benchd-legserve-test.{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = boot_leg(&dir, None, "serial-control").unwrap_err();
        assert!(err.contains(LEG_SERVE_SCRIPT_MISSING), "{err}");
        assert!(err.contains(LEG_SERVE_SCRIPT), "{err}");
        assert!(err.contains("serial-control"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_inherited_resident_socket_refuses_by_name() {
        assert!(refuse_inherited_socket(None, None).is_ok());
        assert!(refuse_inherited_socket(Some("  "), Some("")).is_ok());
        let err = refuse_inherited_socket(Some("/tmp/ds4.sock"), None).unwrap_err();
        assert!(err.contains(LEG_SERVE_INHERITED_SOCKET), "{err}");
        assert!(err.contains(DS4_RESIDENT_SOCKET_ENV), "{err}");
        let err = refuse_inherited_socket(None, Some("/tmp/bw.sock")).unwrap_err();
        assert!(err.contains(BENCH_WORKER_RESIDENT_SOCKET_ENV), "{err}");
    }

    /// The spawn environment names BOTH socket variables, because the ds4 adapter reads one and
    /// the generic bench worker reads the other, and one paired run may meet either.
    #[test]
    fn the_leg_environment_names_both_socket_variables() {
        let leg = LegServe {
            workspace: PathBuf::from("/ref/tree"),
            socket: "/tmp/ds4-resident.run.sock".to_string(),
            label: "candidate",
        };
        let env = leg.spawn_env();
        assert_eq!(
            env,
            vec![
                (
                    DS4_RESIDENT_SOCKET_ENV.to_string(),
                    "/tmp/ds4-resident.run.sock".to_string()
                ),
                (
                    BENCH_WORKER_RESIDENT_SOCKET_ENV.to_string(),
                    "/tmp/ds4-resident.run.sock".to_string()
                ),
            ]
        );
        // Dropping it runs the teardown; the tree does not exist here, so the teardown reports and
        // does not panic. That property is the whole reason `Drop` is where teardown lives.
        drop(leg);
    }
}
