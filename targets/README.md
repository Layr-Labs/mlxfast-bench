# targets/

One signed `target.toml` per (model, platform) — model identity, checkpoint pins,
per-platform baselines, scoring floors/bands, gates, telemetry provider. Bundles
what is triplicated today (MLXFastConstants, benchmark.yml env, fixtures, R2 keys).

Planned: `qwen36-27b.m5.toml`, `qwen36-27b.dgx-spark.toml`.
