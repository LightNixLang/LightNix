use std::{collections::HashMap, time::Duration};

use light_nix_ir::{Constant, OutputPath, VariableId};

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum OutputGoal {
    Equals { path: OutputPath, value: Constant },
    Absent { path: OutputPath },
}

#[derive(Debug, Clone, Default)]
pub struct SolveRequest {
    variable_values: HashMap<VariableId, Constant>,
    output_values: HashMap<OutputPath, Option<Constant>>,
    goals: Vec<OutputGoal>,
    timeout: Option<Duration>,
}

impl SolveRequest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_variable_value(&mut self, variable: VariableId, value: Constant) -> &mut Self {
        self.variable_values.insert(variable, value);
        self
    }

    pub fn set_output_value(&mut self, path: OutputPath, value: Option<Constant>) -> &mut Self {
        self.output_values.insert(path, value);
        self
    }

    pub fn require_output(&mut self, path: OutputPath, value: Constant) -> &mut Self {
        self.goals.push(OutputGoal::Equals { path, value });
        self
    }

    pub fn require_output_absent(&mut self, path: OutputPath) -> &mut Self {
        self.goals.push(OutputGoal::Absent { path });
        self
    }

    pub fn set_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn variable_values(&self) -> &HashMap<VariableId, Constant> {
        &self.variable_values
    }

    pub fn output_values(&self) -> &HashMap<OutputPath, Option<Constant>> {
        &self.output_values
    }

    pub fn goals(&self) -> &[OutputGoal] {
        &self.goals
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }
}
