# Paired Qwen decode-floor policy

The Qwen 3.8 125B A6B engine manifest declares `decodeSpeedupFloor: 0.90`. The paired official path must use that value for its floor verdict, failure reason, and sealed `decode_speedup_floor`/`passed_decode_speedup_floor` metrics. Generic stored-baseline and local paths retain their 0.95 default.

This change corrects the paired path's use of the generic floor. It does not alter the prefill floor (0.95), score weights, correctness checks, or acceptance bands. In particular, production candidate decode timing remains subject to the independent +2% slowdown band. A synthetic 0.925 decode speedup can clear the 0.90 floor under loose test bands while still failing the production band. Passing the floor alone does not imply an accepted score.

The original discrepancy is visible in engine run [34365765860](https://github.com/Layr-Labs/mlxfast-qwen38-125b-a6b-engine/actions/runs/34365765860): its receipt reports 0.95 despite the track manifest's 0.90. The candidate cleared both floors, so this repair does not request changing its historical score. Engine PR [64](https://github.com/Layr-Labs/mlxfast-qwen38-125b-a6b-engine/pull/64) provides a read-only audit of this receipt/manifest disagreement.

No distribution binary is changed by this source PR. After review and merge to the `qwen3.8-125b-a6b-v1` channel, maintainers must rebuild and publish the verified distribution before official machines execute the repair.
