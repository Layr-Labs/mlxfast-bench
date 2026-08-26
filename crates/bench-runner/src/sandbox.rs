//! B-2 — official-run Seatbelt sandbox + fail-closed spawn policy.
//!
//! Port of the Swift official-run sandbox from
//! `mlxfast-challenge-dev/Sources/MLXFastCLI/main.swift`
//! (`runtimeWorkerOptions` :1143-1219, `writeRuntimeWorkerSandboxProfile` :1221-1258,
//! `seatbeltEscaped` :1270-1274) and `benchmark.sh enforce_official_sandbox` (:666-719).
//!
//! An official benchmark run executes the (untrusted, submitted) engine under
//! `/usr/bin/sandbox-exec -f <profile> <engine> runtime-worker …` with a Seatbelt profile
//! that denies network, fork, exec (except the engine itself), all file writes (except
//! `/dev/null`), and reads of the private golden (+ the private dir). The run FAILS CLOSED
//! — refuses to run at all rather than falling back to an unsandboxed/worker-less path — on
//! ANY of: the worker disabled, `MLXFAST_NO_SANDBOX=1`, no engine executable, no derivable
//! sandbox profile, or `/usr/bin/sandbox-exec` missing.
//!
//! This module is macOS-buildable and unit-tested against a STUB (the pure profile builder
//! and the fail-closed resolver take injected inputs, so no real `sandbox-exec` or GPU is
//! needed). The real sandboxed timed run is exercised on the GPU box in B-3.

use std::path::{Path, PathBuf};

/// The Seatbelt interpreter the official run wraps the engine with (Swift
/// `sandboxExecutable`, main.swift:1223).
pub const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// Escape a path for embedding inside a Seatbelt `(literal "...")` / `(subpath "...")`
/// string. Byte-for-byte port of Swift `seatbeltEscaped` (main.swift:1270-1274): backslash
/// first, then double-quote. Order matters — escaping the quote first would double-escape
/// the backslash it introduces.
pub fn seatbelt_escaped(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Build the Seatbelt profile source that guards the official runtime worker. Byte-shape
/// mirror of Swift `writeRuntimeWorkerSandboxProfile` (main.swift:1237-1256): a fixed rule
/// preamble, then the golden deny-read rule, then (when `private_dir` is non-empty) the
/// private-dir subpath deny-read rule, joined by `\n` with NO trailing newline (Swift
/// multiline string literal ends on the interpolation).
///
/// Callers pass ABSOLUTE, symlink-resolved paths ([`absolute_path`]) so the embedded
/// literals match what the kernel resolves the worker's accesses to.
pub fn build_seatbelt_profile(
    engine_path: &str,
    golden_path: &str,
    private_dir: Option<&str>,
) -> String {
    let mut denied_read_rules = vec![format!(
        "(deny file-read* (literal \"{}\"))",
        seatbelt_escaped(golden_path)
    )];
    if let Some(dir) = private_dir {
        if !dir.is_empty() {
            denied_read_rules.push(format!(
                "(deny file-read* (subpath \"{}\"))",
                seatbelt_escaped(dir)
            ));
        }
    }
    // The fixed preamble (main.swift:1247-1254), then the deny-read rules. No trailing
    // newline: the Swift `"""` literal closes right after `deniedReadRules.joined(...)`.
    let mut lines = vec![
        "(version 1)".to_string(),
        "(allow default)".to_string(),
        "(deny network*)".to_string(),
        "(deny process-fork)".to_string(),
        "(deny process-exec*)".to_string(),
        format!(
            "(allow process-exec (literal \"{}\"))",
            seatbelt_escaped(engine_path)
        ),
        "(deny file-write*)".to_string(),
        "(allow file-write* (literal \"/dev/null\"))".to_string(),
    ];
    lines.extend(denied_read_rules);
    lines.join("\n")
}

/// Resolve a path to absolute + symlink-resolved form, matching Swift `absolutePath`
/// (main.swift:1260-1268: relative-to-cwd, `standardizedFileURL.resolvingSymlinksInPath`).
/// A non-existent path cannot be symlink-resolved by the OS, so we fall back to a plain
/// absolutization (join to cwd) — the profile still names a stable absolute path.
pub fn absolute_path(path: &str) -> String {
    let p = Path::new(path);
    let absolute: PathBuf = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(p)
    };
    // canonicalize resolves symlinks but requires existence; fall back to the plain
    // absolute path (lexically normalized) when the target does not exist yet.
    std::fs::canonicalize(&absolute)
        .unwrap_or_else(|_| lexically_normalize(&absolute))
        .to_string_lossy()
        .to_string()
}

/// Lexical `.`/`..` normalization for a path that may not exist on disk.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        use std::path::Component::*;
        match comp {
            CurDir => {}
            ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The fail-closed rejection reasons for an official run, each carrying the EXACT Swift
/// error string (main.swift `runtimeWorkerOptions` / `writeRuntimeWorkerSandboxProfile`).
/// These must byte-match so an operator sees the same refusal on either side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficialSandboxError {
    /// `MLXFAST_USE_RUNTIME_WORKER` is `0`/`false` (main.swift:1157-1161).
    WorkerDisabled,
    /// `MLXFAST_NO_SANDBOX=1` (main.swift:1166-1170).
    SandboxDisabled,
    /// No engine executable configured or derivable (main.swift:1178-1183).
    NoExecutable,
    /// `/usr/bin/sandbox-exec` is not an executable file (main.swift:1224-1226).
    SandboxExecNotFound,
    /// No sandbox profile configured or derivable (main.swift:1206-1210).
    NoProfile,
}

impl OfficialSandboxError {
    /// The verbatim Swift message for this rejection.
    pub fn message(&self) -> &'static str {
        match self {
            OfficialSandboxError::WorkerDisabled => {
                "official benchmark runs require the runtime worker; unset MLXFAST_USE_RUNTIME_WORKER"
            }
            OfficialSandboxError::SandboxDisabled => {
                "official benchmark runs require the runtime worker sandbox; unset MLXFAST_NO_SANDBOX"
            }
            OfficialSandboxError::NoExecutable => {
                "official benchmark runs require a runtime worker executable; none was configured or derivable"
            }
            OfficialSandboxError::SandboxExecNotFound => {
                "sandbox-exec not found for runtime worker sandbox"
            }
            OfficialSandboxError::NoProfile => {
                "official benchmark runs require a runtime worker sandbox profile; none was configured or derivable"
            }
        }
    }
}

impl std::fmt::Display for OfficialSandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for OfficialSandboxError {}

/// The env/context inputs the official-sandbox resolver reads (the `MLXFAST_*` knobs Swift
/// `runtimeWorkerOptions` consults). `None` means the variable is unset/empty. Passing them
/// explicitly (rather than reading the process env inside the resolver) makes the
/// fail-closed matrix a pure, exhaustively-testable function.
#[derive(Debug, Clone, Default)]
pub struct OfficialSandboxInputs<'a> {
    /// `MLXFAST_USE_RUNTIME_WORKER` (fallback `"1"`).
    pub use_runtime_worker: Option<&'a str>,
    /// `MLXFAST_NO_SANDBOX` (fallback `"0"`).
    pub no_sandbox: Option<&'a str>,
    /// `MLXFAST_RUNTIME_WORKER_EXECUTABLE` (fallback: `fallback_executable`).
    pub executable_override: Option<&'a str>,
    /// `MLXFAST_RUNTIME_WORKER_SANDBOX_PROFILE` — a pre-built profile path.
    pub profile_override: Option<&'a str>,
    /// `MLXFAST_PRIVATE_DIR` — an extra subpath denied to the worker.
    pub private_dir: Option<&'a str>,
    /// The engine path benchctl was told to run (Swift's `CommandLine.arguments.first`
    /// fallback for the worker executable).
    pub fallback_executable: &'a str,
    /// The private golden the worker must not read (`blockedGoldenPath`).
    pub golden_path: &'a str,
    /// Whether `/usr/bin/sandbox-exec` exists + is executable (Swift
    /// `FileManager.isExecutableFile`). Injected so the matrix is testable off-box.
    pub sandbox_exec_available: bool,
}

/// Where the resolved profile comes from: an operator-supplied path or a generated source
/// string the caller must write to a temp `.sb` file before spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxProfile {
    /// `MLXFAST_RUNTIME_WORKER_SANDBOX_PROFILE` pointed at an existing profile file.
    Override(String),
    /// A freshly-built profile ([`build_seatbelt_profile`]) to be written out.
    Generated(String),
}

/// The resolved official-sandbox plan: the executable to run, the profile to enforce, and
/// whether worker stderr is forwarded (official forces this OFF, main.swift:1217).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialSandboxPlan {
    /// Absolute path of the engine executable to `(allow process-exec (literal …))`.
    pub executable_path: String,
    /// The profile to pass to `sandbox-exec -f`.
    pub profile: SandboxProfile,
    /// Whether live worker stderr is echoed to this process (always false on official).
    pub forward_worker_stderr: bool,
}

/// Resolve the official-run sandbox, FAIL-CLOSED on any missing prerequisite. Port of Swift
/// `runtimeWorkerOptions` (main.swift:1143-1219) restricted to `officialRun == true` (the
/// only mode benchctl calls this from): every "no worker / no sandbox" exit that Swift turns
/// into a throw on an official run is an `Err` here, never a silent unsandboxed fallback.
///
/// `forwards_worker_stderr` is the caller's requested value; the returned plan's
/// `forward_worker_stderr` is `forwards_worker_stderr && false` (official forces it off —
/// Swift `forwardsWorkerStderr && !officialRun`, main.swift:1217).
pub fn resolve_official_sandbox(
    inputs: &OfficialSandboxInputs<'_>,
    forwards_worker_stderr: bool,
) -> std::result::Result<OfficialSandboxPlan, OfficialSandboxError> {
    // 1. Worker must be enabled (main.swift:1155-1164).
    let enabled = inputs.use_runtime_worker.unwrap_or("1");
    if enabled == "0" || enabled.eq_ignore_ascii_case("false") {
        return Err(OfficialSandboxError::WorkerDisabled);
    }
    // 2. The sandbox must not be explicitly disabled (main.swift:1165-1170).
    if inputs.no_sandbox == Some("1") {
        return Err(OfficialSandboxError::SandboxDisabled);
    }
    // 3. An engine executable must be configured or derivable (main.swift:1171-1184).
    let executable = match inputs.executable_override {
        Some(e) if !e.is_empty() => e,
        _ => inputs.fallback_executable,
    };
    if executable.is_empty() {
        return Err(OfficialSandboxError::NoExecutable);
    }
    let executable_path = absolute_path(executable);

    // 4. Resolve the profile (main.swift:1195-1210). An operator override wins; else, when
    //    the sandbox is not disabled and a golden path is present, generate one — which
    //    requires `/usr/bin/sandbox-exec` to exist (Swift writeRuntimeWorkerSandboxProfile).
    let profile = match inputs.profile_override {
        Some(p) if !p.is_empty() => SandboxProfile::Override(p.to_string()),
        _ => {
            if inputs.no_sandbox != Some("1") && !inputs.golden_path.is_empty() {
                if !inputs.sandbox_exec_available {
                    return Err(OfficialSandboxError::SandboxExecNotFound);
                }
                let golden = absolute_path(inputs.golden_path);
                let private_dir = inputs
                    .private_dir
                    .filter(|d| !d.is_empty())
                    .map(absolute_path);
                SandboxProfile::Generated(build_seatbelt_profile(
                    &executable_path,
                    &golden,
                    private_dir.as_deref(),
                ))
            } else {
                // No override, and no way to generate one → fail closed below.
                return Err(OfficialSandboxError::NoProfile);
            }
        }
    };

    // Official forces worker-stderr forwarding OFF (Swift `forwardsWorkerStderr &&
    // !officialRun`, main.swift:1217; this resolver is the officialRun == true path, so the
    // requested `forwards_worker_stderr` is always suppressed). Named to keep the caller's
    // intent visible even though it can only resolve to false here.
    let _requested_forward = forwards_worker_stderr;
    Ok(OfficialSandboxPlan {
        executable_path,
        profile,
        forward_worker_stderr: false,
    })
}

/// Build the argv the official run spawns: `sandbox-exec -f <profile_path> <engine>
/// runtime-worker --weights <weights> [extra…]`. The engine argv is exactly the one the
/// unsandboxed [`crate::transport::ChildStdioTransport::build_args`] produces, wrapped by
/// the Seatbelt interpreter. Returned as `(program, args)`.
pub fn sandbox_exec_command(
    profile_path: &str,
    engine_path: &str,
    weights_path: &str,
    extra_args: &[String],
) -> (String, Vec<String>) {
    let mut args = vec![
        "-f".to_string(),
        profile_path.to_string(),
        engine_path.to_string(),
    ];
    args.extend(crate::transport::ChildStdioTransport::build_args(
        weights_path,
        extra_args,
    ));
    (SANDBOX_EXEC_PATH.to_string(), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_inputs<'a>() -> OfficialSandboxInputs<'a> {
        OfficialSandboxInputs {
            use_runtime_worker: None,
            no_sandbox: None,
            executable_override: None,
            profile_override: None,
            private_dir: None,
            fallback_executable: "/opt/mlxfast/engine",
            golden_path: "/private/golden.json",
            sandbox_exec_available: true,
        }
    }

    #[test]
    fn seatbelt_escape_backslash_then_quote() {
        // Backslash escaped first, then quote — matching Swift order.
        assert_eq!(seatbelt_escaped(r#"a\b"c"#), r#"a\\b\"c"#);
        assert_eq!(seatbelt_escaped("/plain/path"), "/plain/path");
    }

    #[test]
    fn profile_byte_shape_matches_swift() {
        // Byte-shape parity with the Swift multiline literal: exact rule order, the
        // engine literal, /dev/null write allowance, and the golden deny-read as the last
        // line with NO trailing newline.
        let profile = build_seatbelt_profile("/opt/engine", "/private/golden.json", None);
        let expected = "(version 1)\n\
             (allow default)\n\
             (deny network*)\n\
             (deny process-fork)\n\
             (deny process-exec*)\n\
             (allow process-exec (literal \"/opt/engine\"))\n\
             (deny file-write*)\n\
             (allow file-write* (literal \"/dev/null\"))\n\
             (deny file-read* (literal \"/private/golden.json\"))";
        assert_eq!(profile, expected);
        assert!(
            !profile.ends_with('\n'),
            "no trailing newline (Swift literal)"
        );
    }

    #[test]
    fn profile_appends_private_dir_subpath() {
        // With MLXFAST_PRIVATE_DIR set, a second deny-read subpath rule is appended.
        let profile =
            build_seatbelt_profile("/opt/engine", "/private/golden.json", Some("/private/dir"));
        assert!(profile.ends_with(
            "(deny file-read* (literal \"/private/golden.json\"))\n\
             (deny file-read* (subpath \"/private/dir\"))"
        ));
        // An empty private dir does NOT add a rule.
        let profile2 = build_seatbelt_profile("/opt/engine", "/private/golden.json", Some(""));
        assert_eq!(
            profile2,
            build_seatbelt_profile("/opt/engine", "/private/golden.json", None)
        );
    }

    #[test]
    fn profile_escapes_paths() {
        let profile = build_seatbelt_profile(r#"/opt/en"gine"#, r#"/priv/g"n.json"#, None);
        assert!(profile.contains(r#"(allow process-exec (literal "/opt/en\"gine"))"#));
        assert!(profile.contains(r#"(deny file-read* (literal "/priv/g\"n.json"))"#));
    }

    // ---- Fail-closed matrix (the 5 official refusals) ----

    #[test]
    fn official_happy_path_generates_profile_and_forces_stderr_off() {
        let plan = resolve_official_sandbox(&base_inputs(), true).unwrap();
        assert!(matches!(plan.profile, SandboxProfile::Generated(_)));
        // Even when the caller requests stderr forwarding, official forces it OFF.
        assert!(!plan.forward_worker_stderr);
        assert_eq!(plan.executable_path, absolute_path("/opt/mlxfast/engine"));
    }

    #[test]
    fn fail_closed_worker_disabled() {
        for v in ["0", "false", "False", "FALSE"] {
            let mut inp = base_inputs();
            inp.use_runtime_worker = Some(v);
            assert_eq!(
                resolve_official_sandbox(&inp, false).unwrap_err(),
                OfficialSandboxError::WorkerDisabled
            );
        }
    }

    #[test]
    fn fail_closed_sandbox_disabled() {
        let mut inp = base_inputs();
        inp.no_sandbox = Some("1");
        assert_eq!(
            resolve_official_sandbox(&inp, false).unwrap_err(),
            OfficialSandboxError::SandboxDisabled
        );
    }

    #[test]
    fn fail_closed_no_executable() {
        let mut inp = base_inputs();
        inp.fallback_executable = "";
        inp.executable_override = None;
        assert_eq!(
            resolve_official_sandbox(&inp, false).unwrap_err(),
            OfficialSandboxError::NoExecutable
        );
    }

    #[test]
    fn fail_closed_no_sandbox_exec() {
        let mut inp = base_inputs();
        inp.sandbox_exec_available = false;
        assert_eq!(
            resolve_official_sandbox(&inp, false).unwrap_err(),
            OfficialSandboxError::SandboxExecNotFound
        );
    }

    #[test]
    fn fail_closed_no_derivable_profile() {
        // No override, and no golden path to build one from → NoProfile (a generated
        // profile is impossible). sandbox_exec availability is irrelevant here.
        let mut inp = base_inputs();
        inp.golden_path = "";
        inp.profile_override = None;
        assert_eq!(
            resolve_official_sandbox(&inp, false).unwrap_err(),
            OfficialSandboxError::NoProfile
        );
    }

    #[test]
    fn profile_override_bypasses_generation() {
        // An operator-supplied profile path is used verbatim (Override), and it does NOT
        // require sandbox-exec to be present (Swift only touches sandbox-exec when it must
        // WRITE a profile).
        let mut inp = base_inputs();
        inp.profile_override = Some("/etc/mlxfast/worker.sb");
        inp.sandbox_exec_available = false;
        let plan = resolve_official_sandbox(&inp, false).unwrap();
        assert_eq!(
            plan.profile,
            SandboxProfile::Override("/etc/mlxfast/worker.sb".to_string())
        );
    }

    #[test]
    fn all_error_messages_are_swift_verbatim() {
        assert_eq!(
            OfficialSandboxError::WorkerDisabled.message(),
            "official benchmark runs require the runtime worker; unset MLXFAST_USE_RUNTIME_WORKER"
        );
        assert_eq!(
            OfficialSandboxError::SandboxDisabled.message(),
            "official benchmark runs require the runtime worker sandbox; unset MLXFAST_NO_SANDBOX"
        );
        assert_eq!(
            OfficialSandboxError::NoExecutable.message(),
            "official benchmark runs require a runtime worker executable; none was configured or derivable"
        );
        assert_eq!(
            OfficialSandboxError::SandboxExecNotFound.message(),
            "sandbox-exec not found for runtime worker sandbox"
        );
        assert_eq!(
            OfficialSandboxError::NoProfile.message(),
            "official benchmark runs require a runtime worker sandbox profile; none was configured or derivable"
        );
    }

    #[test]
    fn sandbox_exec_command_wraps_engine_argv() {
        let (program, args) =
            sandbox_exec_command("/tmp/x.sb", "/opt/engine", "/weights/qwen", &[]);
        assert_eq!(program, "/usr/bin/sandbox-exec");
        assert_eq!(
            args,
            vec![
                "-f",
                "/tmp/x.sb",
                "/opt/engine",
                "runtime-worker",
                "--weights",
                "/weights/qwen",
            ]
        );
    }
}
