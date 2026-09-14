use std::{
    os::unix::process::ExitStatusExt,
    process::{ExitStatus, Output},
};

use gftools::remove_overlaps;
use tracing::info_span;

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};

#[derive(PartialEq, Debug)]
pub(crate) struct RemoveOverlaps;

impl Operation for RemoveOverlaps {
    fn shortname(&self) -> &str {
        "RemoveOverlaps"
    }

    fn input_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Bytes]
    }

    fn output_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Bytes]
    }

    fn execute(
        &self,
        inputs: &[OperationOutput],
        outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError> {
        let _span = info_span!("removeoverlaps").entered();
        let input_file = inputs
            .first()
            .ok_or_else(|| ApplicationError::WrongInputs("No input file provided".to_string()))?;
        let ttf_data = input_file.to_bytes()?;

        let processed = remove_overlaps(&ttf_data)
            .map_err(|e| ApplicationError::RemoveOverlapsError(e.to_string()))?;
        outputs[0].set_contents(processed)?;
        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: vec![],
            stderr: vec![],
        })
    }

    fn description(&self) -> String {
        "Remove overlaps".to_string()
    }
}
