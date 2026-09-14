use serde_inline_default::serde_inline_default;
use serde_json::Value;
use std::{
    collections::HashMap,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::{ExitStatus, Output},
};

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};
use fontc::{Flags, generate_font};
use serde::{Deserialize, Serialize};
use tracing::info_span;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde_inline_default]
#[serde(rename_all = "camelCase")]
pub struct FontcConfig {
    #[serde(default)]
    pub flatten_components: bool,

    #[serde(default)]
    pub decompose_transformed_components: bool,

    #[serde(default = "default_reverse_outline_direction")]
    pub reverse_outline_direction: bool,
}

fn default_reverse_outline_direction() -> bool {
    true
}

impl Default for FontcConfig {
    fn default() -> Self {
        Self {
            flatten_components: false,
            decompose_transformed_components: false,
            reverse_outline_direction: true,
        }
    }
}

#[derive(PartialEq, Debug)]
pub(crate) struct Fontc {
    config: FontcConfig,
}

impl Fontc {
    pub fn new() -> Self {
        Fontc {
            config: FontcConfig::default(),
        }
    }

    fn fontc_options(&self) -> fontc::Options {
        let mut options = fontc::Options::default();
        if self.config.decompose_transformed_components {
            options
                .flags
                .insert(Flags::DECOMPOSE_TRANSFORMED_COMPONENTS);
        }

        if self.config.flatten_components {
            options.flags.insert(Flags::FLATTEN_COMPONENTS);
        }

        if !self.config.reverse_outline_direction {
            options.flags.insert(Flags::KEEP_DIRECTION);
        }

        options
    }
}

impl Operation for Fontc {
    fn shortname(&self) -> &str {
        "Fontc"
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
        let _span = info_span!("fontc").entered();
        let input_font = inputs
            .first()
            .ok_or_else(|| ApplicationError::WrongInputs("No input file provided".to_string()))?
            .to_filename(Some(".glyphs"))?;

        let input = fontc::Input::new(&PathBuf::from(input_font))
            .map_err(|e| ApplicationError::Other(e.to_string()))?
            .create_source()
            .map_err(|e| ApplicationError::Other(e.to_string()))?;
        let font = generate_font(input, self.fontc_options())
            .map_err(|e| ApplicationError::Other(e.to_string()))?;
        outputs[0].set_contents(font)?;
        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: vec![],
            stderr: vec![],
        })
    }

    fn set_extra(&mut self, extra: HashMap<String, Value>) {
        // Deserialize the extra map into our typed config
        let value = Value::Object(extra.into_iter().collect());
        self.config = serde_json::from_value(value).unwrap_or_else(|e| {
            log::warn!("Failed to deserialize Fontc config: {}. Using defaults.", e);
            FontcConfig::default()
        });
    }

    fn description(&self) -> String {
        "Compile font".to_string()
    }
}
