use std::{os::unix::process::ExitStatusExt as _, process::Output};

use babelfont::{
    DesignCoord, DesignLocation, Tag,
    filters::{DropVariations, FontFilter, SetDefaultLocation},
};

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};

#[derive(PartialEq, Debug)]
pub(crate) struct InstantiateSource {
    args: Option<String>,
}

impl InstantiateSource {
    pub fn new() -> Self {
        InstantiateSource { args: None }
    }
}

impl Operation for InstantiateSource {
    fn shortname(&self) -> &str {
        "InstantiateSource"
    }

    fn input_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::SourceFont]
    }

    fn output_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::SourceFont]
    }

    fn execute(
        &self,
        inputs: &[OperationOutput],
        outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError> {
        // Assert that we have two inputs
        let input_font = inputs
            .first()
            .ok_or_else(|| ApplicationError::WrongInputs("No input".into()))?
            .to_font_source()?;
        let mut output_font = input_font.clone();
        // Parse location from args (e.g. "wght=400,wdth=100") and apply to output_font
        let location = self
            .args
            .as_deref()
            .unwrap_or("")
            .split(",")
            .filter_map(|pair| {
                let mut parts = pair.splitn(2, '=');
                let axis = parts.next()?.trim();
                let value = parts.next()?.trim().parse::<f64>().ok()?;
                let tag = Tag::new_checked(axis.as_bytes());
                tag.ok().map(|t| (t, DesignCoord::new(value)))
            })
            .collect::<DesignLocation>();
        SetDefaultLocation::new(location)
            .apply(&mut output_font)
            .map_err(|e| {
                ApplicationError::Other(format!("Failed to set default location: {}", e))
            })?;
        DropVariations
            .apply(&mut output_font)
            .map_err(|e| ApplicationError::Other(format!("Failed to drop variations: {}", e)))?;
        outputs[0].set_font_source(output_font)?;
        Ok(Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: vec![],
            stderr: vec![],
        })
    }

    fn description(&self) -> String {
        format!(
            "Instantiate font source at {}",
            self.args.as_deref().unwrap_or("")
        )
    }

    fn set_args(&mut self, args: Option<String>) {
        self.args = args;
    }

    fn identifier(&self) -> String {
        format!("instantiate-{}", self.args.as_deref().unwrap_or(""))
    }
}
