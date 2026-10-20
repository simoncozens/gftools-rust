use std::{collections::HashMap, process::Output};

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};
use gftools::set_font_values;

#[derive(PartialEq, Debug, Default)]

pub(crate) struct FontSetter(Vec<(String, serde_json::Value)>);
impl Operation for FontSetter {
    fn shortname(&self) -> &str {
        "FontSetter"
    }

    fn input_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Bytes]
    }

    fn output_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Bytes]
    }

    fn set_extra(&mut self, extra: HashMap<String, serde_json::Value>) {
        if let Some(config) = extra.get("config") {
            self.0 = serde_json::from_value(config.clone()).unwrap_or_default();
        }
    }

    fn execute(
        &self,
        inputs: &[OperationOutput],
        outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError> {
        assert!(inputs.len() == outputs.len());
        let input_file = inputs
            .first()
            .ok_or_else(|| ApplicationError::WrongInputs("No input file provided".to_string()))?;
        let ttf_data = input_file.to_bytes()?;
        let output = set_font_values(&ttf_data, &self.0)
            .map_err(|e| ApplicationError::Other(e.to_string()))?;
        if self.0.is_empty() {
            log::warn!("FontSetter called with empty configuration");
        }
        outputs[0].set_contents(output)?;
        Ok(Output {
            status: std::process::ExitStatus::default(),
            stdout: vec![],
            stderr: vec![],
        })
    }

    fn description(&self) -> String {
        "Change a font's binary values".to_string()
    }
}
