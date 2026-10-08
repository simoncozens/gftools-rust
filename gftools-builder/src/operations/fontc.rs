use serde_inline_default::serde_inline_default;
use serde_json::Value;
use std::{
    collections::HashMap,
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
    // For some reason, serde_inline_default inside a nested (flattened)
    // struct doesn't provide defaults and you get missing field errors
    #[serde(default = "default_to_true")]
    pub flatten_components: bool,

    #[serde(default = "default_to_true")]
    pub decompose_transformed_components: bool,

    #[serde(default = "default_to_true")]
    pub reverse_outline_direction: bool,

    #[serde(default = "default_to_true")]
    pub production_names: bool,
}

fn default_to_true() -> bool {
    true
}

impl Default for FontcConfig {
    fn default() -> Self {
        Self {
            flatten_components: false,
            decompose_transformed_components: true,
            reverse_outline_direction: true,
            production_names: true,
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
        if !self.config.production_names {
            options.flags.remove(Flags::PRODUCTION_NAMES);
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
            status: ExitStatus::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(config: &str) -> fontc::Flags {
        let config: FontcConfig = serde_json::from_str(config).unwrap();
        Fontc { config }.fontc_options().flags
    }

    #[test]
    fn production_names_default_on_and_can_be_turned_off() {
        assert!(flags("{}").contains(Flags::PRODUCTION_NAMES));
        assert!(!flags(r#"{"productionNames": false}"#).contains(Flags::PRODUCTION_NAMES));
    }
}
