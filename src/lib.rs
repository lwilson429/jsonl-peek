//! Streaming primitives for scanning JSONL files without loading the whole
//! file into memory. The binary in `src/main.rs` is a thin shell over this.

pub mod hist;
pub mod json;
pub mod lines;
pub mod path;
