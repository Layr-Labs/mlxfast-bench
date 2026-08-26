//! Weight transform + staged-output validation; per-target quant emit.
//!
//! Port of the MLX-free Swift transform plus the gap-free safetensors tiling checks and
//! atomic publish currently in benchmark.sh. Emits per-platform quant: MLX int4 group-64
//! for Metal, NVFP4 for Blackwell (parity is at logits, not bytes). See §2, plan §4.

#![allow(dead_code)]
