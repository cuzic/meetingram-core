//! Discussion graph extraction, evaluation, and persistence.

mod cloud;
pub mod eval;
mod model;
mod patch;
mod prompt;
pub mod store;

pub use cloud::{
    build_conflict_cloud, build_conflict_cloud_default, conflict_candidates, conflict_subject,
    truncate_utf16, validate_cloud, AssumptionOn, Basis, CloudAnchor, CloudAssumption,
    CloudDiagram, CloudElement, CloudInputError, CloudInputStage, CloudParseError, CloudPrompt,
    ConflictCandidate, ValidatedCloud, CLOUD_SYSTEM, CLOUD_TEXT_UNITS,
};
pub use model::*;
pub use patch::{
    apply_patch, apply_patch_with_context, parse_ops, AppliedOp, GroundRef, Op, ParseError,
    PatchContext, PatchResult, Rejection,
};
pub use prompt::{
    build_consolidate, build_consolidate_groups, build_consolidate_issues, build_incremental,
    ConsolidateStyle, IssuePromptStage, Prompt, PromptError, DEFAULT_INPUT_LIMIT,
};
