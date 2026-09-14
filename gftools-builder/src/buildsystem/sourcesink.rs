use std::{
    os::unix::process::ExitStatusExt,
    process::{ExitStatus, Output},
};

use crate::{
    buildsystem::{Operation, OperationOutput},
    error::ApplicationError,
};

/// Source and Sink operations do nothing; they just serve as start and end points
/// in the operations graph.
pub(crate) enum SourceSink {
    Source,
    Sink,
}
impl Operation for SourceSink {
    fn execute(
        &self,
        _inputs: &[OperationOutput],
        _outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError> {
        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }

    fn description(&self) -> String {
        match self {
            SourceSink::Source => "Source Operation".to_string(),
            SourceSink::Sink => "Sink Operation".to_string(),
        }
    }

    fn shortname(&self) -> &str {
        match self {
            SourceSink::Source => "Source",
            SourceSink::Sink => "Sink",
        }
    }
}
