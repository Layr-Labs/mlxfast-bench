//! `--engine-resource NAME=PATH` — the benchd→worker RESOURCE PASSTHROUGH (Darkbloom runner
//! contract §8.1 / §13b).
//!
//! A runner may need an input that is NOT in the checkpoint: the Qwen 3.8 Flash-Next
//! `Qwen4ExpRunner` builds its disk-resident n-gram row source from the resource
//! `qwen4exp.ngramRowSource`, whose value is the DIRECTORY the offline transform wrote. Without it
//! the runner refuses at load, by name, before any verb runs.
//!
//! WHERE THE VALUE COMES FROM. Each resource is read from benchd's OWN command line, as the engine
//! repository's wrapper invokes it. benchd NEVER reads a resource out of a manifest, a fixture or
//! any other file inside the submission: a submission-editable file must not be able to point the
//! engine at bytes of its own choosing. The wrapper resolves the path from the track fixture and
//! puts it on benchd's argv; benchd only carries it through.
//!
//! WHAT BENCHD CHECKS. The NAME and the shape only. benchd does not open the path, does not test
//! that it exists and does not look at what is behind it: the worker validates existence and shape
//! and refuses by name (contract §8.1). Every resource becomes exactly one `--resource NAME=PATH`
//! token pair on the worker argv, in command-line order.

/// The worker-side flag one `--engine-resource` becomes. The engine's `runtime-worker` verb accepts
/// it repeatably (contract §8.1); it is part of
/// [`crate::measure_job::RUNTIME_WORKER_ACCEPTED_FLAGS`], so benchd's own argv fence admits it.
pub const RESOURCE_FLAG: &str = "--resource";

/// The benchd-side CLI flag, repeatable, `NAME=PATH`.
pub const ENGINE_RESOURCE_FLAG: &str = "--engine-resource";

/// One resource name bound to one path, exactly as the command line gave them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineResource {
    /// The runner-private resource key, for example `qwen4exp.ngramRowSource`.
    pub name: String,
    /// The value, passed through to the worker VERBATIM. benchd does not resolve, canonicalize or
    /// open it.
    pub path: String,
}

/// The characters a resource NAME may use: ASCII letters, digits, `.`, `_` and `-`.
///
/// The runner manifest (`bench_core::runner_manifest`) declares no identifier character set of its
/// own — its fields are free strings — so this set is stated here. It admits every name the
/// contract uses (`qwen4exp.ngramRowSource`) and refuses whitespace, `=`, and shell metacharacters,
/// so a name can never split into a second argv token or hide a second assignment.
fn name_char_is_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'
}

/// Parse one `NAME=PATH` value. The split is at the FIRST `=`, so a path may contain `=`.
///
/// Refuses, by name: no `=`, an empty NAME, a NAME with a character outside
/// [`name_char_is_allowed`], and an empty PATH.
pub fn parse_engine_resource(raw: &str) -> Result<EngineResource, String> {
    let (name, path) = raw.split_once('=').ok_or_else(|| {
        format!(
            "{ENGINE_RESOURCE_FLAG} {raw:?} has no '=': the value is NAME=PATH \
             (for example qwen4exp.ngramRowSource=/data/ngram)"
        )
    })?;
    if name.is_empty() {
        return Err(format!(
            "{ENGINE_RESOURCE_FLAG} {raw:?} has an empty NAME: the value is NAME=PATH"
        ));
    }
    if let Some(bad) = name.chars().find(|c| !name_char_is_allowed(*c)) {
        return Err(format!(
            "{ENGINE_RESOURCE_FLAG} NAME {name:?} contains {bad:?}: a resource name uses only \
             ASCII letters, digits, '.', '_' and '-'"
        ));
    }
    if path.is_empty() {
        return Err(format!(
            "{ENGINE_RESOURCE_FLAG} {raw:?} has an empty PATH: give the directory or file the \
             runner loads the resource from"
        ));
    }
    Ok(EngineResource {
        name: name.to_string(),
        path: path.to_string(),
    })
}

/// Parse one `NAME=PATH` and append it to `declared`. A NAME already declared refuses: two paths
/// for one resource have no defined winner, and the worker refuses a duplicate as well (§8.1).
pub fn push_engine_resource(declared: &mut Vec<EngineResource>, raw: &str) -> Result<(), String> {
    let resource = parse_engine_resource(raw)?;
    if declared.iter().any(|d| d.name == resource.name) {
        return Err(format!(
            "{ENGINE_RESOURCE_FLAG} NAME {:?} is declared twice: give each resource exactly once",
            resource.name
        ));
    }
    declared.push(resource);
    Ok(())
}

/// The worker argv tokens for `declared`: `--resource NAME=PATH` per resource, in the order the
/// command line gave them.
///
/// These ride with `--weights`, NOT behind the `--speculative-protocol` gate: the runner needs the
/// resource to LOAD the model, so every spawn carries it, including the teacher-forced and
/// correctness spawns that speak strict v1.
pub fn spawn_args(declared: &[EngineResource]) -> Vec<String> {
    let mut args = Vec::with_capacity(declared.len() * 2);
    for resource in declared {
        args.push(RESOURCE_FLAG.to_string());
        args.push(format!("{}={}", resource.name, resource.path));
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure_job::{validate_spawn_argv, RUNTIME_WORKER_ACCEPTED_FLAGS};

    fn resource(name: &str, path: &str) -> EngineResource {
        EngineResource {
            name: name.to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn parses_the_contract_resource() {
        assert_eq!(
            parse_engine_resource("qwen4exp.ngramRowSource=/data/ngram").unwrap(),
            resource("qwen4exp.ngramRowSource", "/data/ngram")
        );
    }

    #[test]
    fn splits_at_the_first_equals_so_a_path_may_carry_one() {
        assert_eq!(
            parse_engine_resource("rows=/data/a=b").unwrap(),
            resource("rows", "/data/a=b")
        );
    }

    #[test]
    fn a_value_without_an_equals_refuses() {
        let e = parse_engine_resource("qwen4exp.ngramRowSource").unwrap_err();
        assert!(e.contains("no '='"), "{e}");
    }

    #[test]
    fn an_empty_name_refuses() {
        let e = parse_engine_resource("=/data/ngram").unwrap_err();
        assert!(e.contains("empty NAME"), "{e}");
    }

    #[test]
    fn an_empty_path_refuses() {
        let e = parse_engine_resource("rows=").unwrap_err();
        assert!(e.contains("empty PATH"), "{e}");
    }

    #[test]
    fn a_name_outside_the_character_set_refuses() {
        for bad in ["row s=/d", "row/s=/d", "row$s=/d", "röws=/d"] {
            let e = parse_engine_resource(bad).unwrap_err();
            assert!(e.contains("resource name uses only"), "{bad}: {e}");
        }
    }

    #[test]
    fn repeated_names_accumulate_in_command_line_order() {
        let mut declared = Vec::new();
        push_engine_resource(&mut declared, "qwen4exp.ngramRowSource=/data/ngram").unwrap();
        push_engine_resource(&mut declared, "rows-2=/data/other").unwrap();
        assert_eq!(
            declared,
            vec![
                resource("qwen4exp.ngramRowSource", "/data/ngram"),
                resource("rows-2", "/data/other"),
            ]
        );
    }

    #[test]
    fn a_duplicate_name_refuses() {
        let mut declared = Vec::new();
        push_engine_resource(&mut declared, "rows=/data/one").unwrap();
        let e = push_engine_resource(&mut declared, "rows=/data/two").unwrap_err();
        assert!(e.contains("declared twice"), "{e}");
        assert_eq!(declared, vec![resource("rows", "/data/one")]);
    }

    #[test]
    fn each_resource_becomes_one_resource_flag_pair_in_order() {
        let declared = vec![
            resource("qwen4exp.ngramRowSource", "/data/ngram"),
            resource("rows-2", "/data/other"),
        ];
        assert_eq!(
            spawn_args(&declared),
            vec![
                "--resource",
                "qwen4exp.ngramRowSource=/data/ngram",
                "--resource",
                "rows-2=/data/other",
            ]
        );
    }

    #[test]
    fn no_resources_add_no_tokens() {
        assert!(spawn_args(&[]).is_empty());
    }

    /// MOCK-ENGINE ACCEPTANCE — the resources reach the SPAWNED PROCESS's argv, through the real
    /// [`bench_runner::ChildStdioTransport`] spawn the timed legs use. The fake worker writes its
    /// own argv and exits; the assertion is the argv the kernel handed it, not a benchd-side
    /// reconstruction of it.
    ///
    /// Unix-only: the fake worker is a `/bin/sh` script.
    #[test]
    #[cfg(unix)]
    fn two_resources_reach_the_spawned_worker_argv() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("benchd-resource-argv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let argv_out = dir.join("argv.txt");
        let script = dir.join("fake_worker.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > {}\nexit 0\n",
                argv_out.display()
            ),
        )
        .expect("write script");
        let mut perms = std::fs::metadata(&script).expect("stat").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("chmod");

        let declared = vec![
            resource("qwen4exp.ngramRowSource", "/data/ngram"),
            resource("rows-2", "/data/other"),
        ];
        // The SAME builder the timed legs use: the resources ride in front of the v1.1 gate.
        let extra = crate::free_run_spawn_args(&declared);
        let transport = bench_runner::ChildStdioTransport::spawn(
            &script.to_string_lossy(),
            "/weights/qwen",
            &extra,
        )
        .expect("spawn fake worker");
        // Wait for the script to land its argv (spawn is asynchronous).
        let mut observed = String::new();
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(&argv_out) {
                if text.contains("v1.1") {
                    observed = text;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        drop(transport);
        std::fs::remove_dir_all(&dir).ok();
        let argv: Vec<&str> = observed.lines().collect();
        assert_eq!(
            argv,
            vec![
                "runtime-worker",
                "--weights",
                "/weights/qwen",
                "--resource",
                "qwen4exp.ngramRowSource=/data/ngram",
                "--resource",
                "rows-2=/data/other",
                "--speculative-protocol",
                "v1.1",
            ],
            "the spawned worker's own argv"
        );
    }

    /// The argv fence ADMITS the flag: a spawn that carries a resource is not refused pre-GPU.
    #[test]
    fn the_argv_fence_admits_the_resource_flag() {
        assert!(RUNTIME_WORKER_ACCEPTED_FLAGS.contains(&RESOURCE_FLAG));
        let declared = vec![resource("qwen4exp.ngramRowSource", "/data/ngram")];
        validate_spawn_argv(&spawn_args(&declared)).unwrap();
    }
}
