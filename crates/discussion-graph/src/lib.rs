//! In-memory discussion graph extraction and evaluation.

pub mod eval;
mod model;
mod patch;
mod prompt;

pub use model::*;
pub use patch::{
    apply_patch, parse_ops, AppliedOp, GroundRef, Op, ParseError, PatchResult, Rejection,
};
pub use prompt::{
    build_consolidate, build_incremental, ConsolidateStyle, Prompt, PromptError,
    DEFAULT_INPUT_LIMIT,
};
