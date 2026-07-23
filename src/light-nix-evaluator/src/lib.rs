//! Concrete evaluation and change-impact snapshots for LightNix.

mod error;
mod evaluator;
mod snapshot;
mod value;

pub use error::{EvaluationError, EvaluationErrorKind};
pub use evaluator::{EvaluationInputs, EvaluationResult, TunableValue, evaluate_module};
pub use light_nix_ir::{OutputPath, SourceOrigin};
pub use snapshot::{EvaluationSnapshot, OutputChange, OutputEntry};
pub use value::{RuntimeRecord, RuntimeValue};
