# bench-adapter

The Engine Protocol v1 adapter SDK.

A track engine repository writes one thing: an implementation of the `Engine`
trait. This crate gives it the protocol.

## What you implement

The `Engine` trait (`src/engine.rs`). It has the generation calls and nothing
else: `drain_to_zero`, `prefill`, `decode_begin`, `step`, `correctness_begin`,
`correctness_freerun`, `free_decode_begin`, `free_decode_run`, `expert_stats`
and `peak_ram_gb`.

An `EngineFactory` makes one engine for each phase. The adapter makes a new
engine at each phase opener and drops it at the phase close. No warm cache stays
between phases.

The engine reports raw counts only. It reports no rate, no ratio, no time and no
speedup. benchd measures the time and does the division.

## What you get

The `Adapter` (`src/adapter.rs`): the NDJSON loop over stdin and stdout. It
does:

- the unsolicited `hello` (`id = 0`), which carries the protocol version, the
  backend and device names, the runnable spec modes, the capabilities, and the
  optional `runner` and `head_provenance` audit blocks;
- the session nonce on every response, and the request `id` echo;
- the phase state machine: one open phase, matched openers and steps, and the
  `phase_diagnostics` barrier;
- the `completed_work` counter, which counts rounds on a free-run phase
  (`R + 1`), not tokens;
- fail-closed behaviour: a bad line, a protocol error, an engine error or an
  early EOF discards the phase session;
- the free-run consistency checks before any counter goes on the wire.

The wire types come from `bench-protocol`. This crate adds no wire type.

## Use

```rust
use bench_adapter::{Adapter, EngineFactory};

fn serve(factory: impl EngineFactory) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    Adapter::new(factory, "cuda", "cuda").run(stdin.lock(), stdout.lock())
}
```

Give `Adapter::new` the factory plus the backend and device names. Add
`.advertising(modes)` when the engine runs fewer than both spec modes, and
`.with_runner(...)` / `.with_head_provenance(...)` when the worker declares
them.

## Test backend

`src/mock.rs` holds a deterministic engine with no GPU. The crate's tests drive
the real loop against it. Your repository can use the same mock to test its own
wiring.
