use std::{
    os::unix::process::ExitStatusExt as _,
    process::{ExitStatus, Output},
};

use async_trait::async_trait;

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};

#[derive(PartialEq, Debug)]
pub struct FileToBytes;

#[async_trait]
impl Operation for FileToBytes {
    fn shortname(&self) -> &str {
        "ToBytes"
    }
    fn description(&self) -> String {
        "Convert file path to bytes".to_string()
    }
    fn input_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Path]
    }
    fn output_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Bytes]
    }
    fn execute(
        &self,
        inputs: &[OperationOutput],
        outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError> {
        let input = inputs
            .first()
            .ok_or_else(|| ApplicationError::WrongInputs("No input".into()))?;
        let bytes = input.to_bytes()?;
        outputs
            .first()
            .ok_or_else(|| ApplicationError::WrongOutputs("Missing output slot 0".into()))?
            .set_bytes(bytes)?;
        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: vec![],
            stderr: vec![],
        })
    }
    fn hidden(&self) -> bool {
        true
    }
}

#[derive(PartialEq, Debug)]
pub struct BytesToTempFile;

#[async_trait]
impl Operation for BytesToTempFile {
    fn shortname(&self) -> &str {
        "ToTempFile"
    }
    fn description(&self) -> String {
        "Convert bytes to temporary file".to_string()
    }
    fn input_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Bytes]
    }
    fn output_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Path]
    }
    fn execute(
        &self,
        inputs: &[OperationOutput],
        outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError> {
        let input = inputs
            .first()
            .ok_or_else(|| ApplicationError::WrongInputs("No input".into()))?;
        let bytes = input.to_bytes()?;
        let out = outputs
            .first()
            .ok_or_else(|| ApplicationError::WrongOutputs("Missing output slot 0".into()))?;

        // Force the output to create/get a temp file and write the bytes to it
        let temp_filename = out.to_filename(Some(".glyphs"))?;
        std::fs::write(&temp_filename, &bytes)
            .map_err(|e| ApplicationError::Other(format!("Failed to write temp file: {}", e)))?;

        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: vec![],
            stderr: vec![],
        })
    }
    fn hidden(&self) -> bool {
        true
    }
}

#[derive(PartialEq, Debug)]
pub struct PathToSourceFont;

#[async_trait]
impl Operation for PathToSourceFont {
    fn shortname(&self) -> &str {
        "LoadSource"
    }
    fn description(&self) -> String {
        "Load font source file".to_string()
    }
    fn input_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Path]
    }
    fn output_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::SourceFont]
    }
    fn execute(
        &self,
        inputs: &[OperationOutput],
        outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError> {
        let input = inputs
            .first()
            .ok_or_else(|| ApplicationError::WrongInputs("No input".into()))?;
        let font = input.to_font_source()?;
        outputs
            .first()
            .ok_or_else(|| ApplicationError::WrongOutputs("Missing output slot 0".into()))?
            .set_font_source(font)?;
        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: vec![],
            stderr: vec![],
        })
    }
    fn hidden(&self) -> bool {
        true
    }
}
