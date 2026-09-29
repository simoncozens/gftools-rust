use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tracing::info_span;

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};
use gftools::{FixFvarTable, IncludeSourceFixes, Interactive, fix_font};
use std::process::Output;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FixConfig {
    #[serde(default)]
    pub include_source_fixes: bool,
    // #[serde(default)]
    // pub fvar_instance_axis_dflts: HashMap<String, f32>,
}

#[derive(PartialEq, Debug)]
pub(crate) struct Fix {
    args: Option<String>,
    config: FixConfig,
}

impl Fix {
    pub fn new() -> Self {
        Fix {
            args: None,
            config: FixConfig::default(),
        }
    }
}

impl Operation for Fix {
    fn shortname(&self) -> &str {
        "Fix"
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
        let _span = info_span!("gftools-fix-font").entered();
        match fix_font(
            &inputs[0].to_filename(Some(".ttf"))?,
            &outputs[0].to_filename(Some(".ttf"))?,
            if self.config.include_source_fixes {
                IncludeSourceFixes::Yes
            } else {
                IncludeSourceFixes::No
            },
            Interactive::No,
            FixFvarTable::Yes,
        ) {
            Ok(()) => Ok(Output {
                status: std::process::ExitStatus::default(),
                stdout: Vec::new(),
                stderr: Vec::new(),
            }),
            Err(e) => Err(ApplicationError::Other(format!(
                "gftools-fix-font failed: {}",
                e
            ))),
        }
    }

    fn description(&self) -> String {
        "Apply gftools-fix-font".to_string()
    }

    fn set_args(&mut self, args: Option<String>) {
        self.args = args;
    }

    fn set_extra(&mut self, extra: HashMap<String, Value>) {
        // Deserialize the extra map into our typed config
        let value = Value::Object(extra.into_iter().collect());
        self.config = serde_json::from_value(value).unwrap_or_else(|e| {
            log::warn!("Failed to deserialize Fix config: {}. Using defaults.", e);
            FixConfig::default()
        });
    }

    fn identifier(&self) -> String {
        format!(
            "Fix-{}-{:?}",
            self.args.as_deref().unwrap_or(""),
            self.config
        )
    }
}
