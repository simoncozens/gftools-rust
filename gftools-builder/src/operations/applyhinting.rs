use std::process::Output;

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};
use gftools::{HintingFile, apply_hinting};

/// Give a compiled TrueType font the hinting of its hinting file (the args).
#[derive(PartialEq, Debug, Default)]
pub(crate) struct ApplyHinting {
    args: Option<String>,
}

impl ApplyHinting {
    pub fn new() -> Self {
        ApplyHinting { args: None }
    }
}

impl Operation for ApplyHinting {
    fn shortname(&self) -> &str {
        "ApplyHinting"
    }

    fn set_args(&mut self, args: Option<String>) {
        self.args = args;
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
        let path = self
            .args
            .as_deref()
            .ok_or_else(|| ApplicationError::Other("applyHinting needs a hinting file".into()))?;
        let text = std::fs::read_to_string(path)?;
        let file = HintingFile::from_json(&text)
            .map_err(|e| ApplicationError::Other(format!("{path}: {e}")))?;
        let font = inputs[0].to_bytes()?;
        let hinted = apply_hinting(&font, &file)
            .map_err(|e| ApplicationError::Other(format!("{path}: {e}")))?;
        outputs[0].set_contents(hinted)?;
        Ok(Output {
            status: std::process::ExitStatus::default(),
            stdout: vec![],
            stderr: vec![],
        })
    }

    fn description(&self) -> String {
        format!(
            "Apply the hinting file {}",
            self.args.as_deref().unwrap_or("")
        )
    }

    fn identifier(&self) -> String {
        format!("ApplyHinting-{}", self.args.as_deref().unwrap_or(""))
    }
}
