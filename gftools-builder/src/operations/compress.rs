use std::{
    os::unix::process::ExitStatusExt,
    process::{ExitStatus, Output},
};

use tracing::info_span;
use ttf2woff2::{BrotliQuality, encode};

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};

#[derive(PartialEq, Debug)]
pub(crate) struct Compress;

impl Operation for Compress {
    fn shortname(&self) -> &str {
        "Compress"
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
        let _span = info_span!("woff2compress").entered();
        let input_file = inputs
            .first()
            .ok_or_else(|| ApplicationError::WrongInputs("No input file provided".to_string()))?;
        let ttf_data = input_file.to_bytes()?;

        let compressed = encode(&ttf_data, BrotliQuality::default())?;
        outputs[0].set_contents(compressed)?;
        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: vec![],
            stderr: vec![],
        })
    }

    fn description(&self) -> String {
        "Convert to woff2".to_string()
    }
}
