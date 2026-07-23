//! Concrete evaluation and change-impact snapshots for LightNix.

mod error;
mod evaluator;
mod snapshot;
mod value;

pub use error::{EvaluationError, EvaluationErrorKind};
pub use evaluator::{EvaluationInputs, EvaluationResult, TunableValue, evaluate_module};
pub use snapshot::{EvaluationSnapshot, OutputChange, OutputEntry, OutputPath, SourceOrigin};
pub use value::{RuntimeRecord, RuntimeValue};
