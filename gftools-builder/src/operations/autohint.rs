use std::{os::unix::process::ExitStatusExt, process::Output};

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};
use gftools::primary_script;
use skrifa::FontRef;
use tilvisan::{Args, ScriptClassIndex, autohint};

#[derive(PartialEq, Debug, Default)]
pub(crate) struct Autohint {
    args: Option<String>,
}

impl Autohint {
    pub fn new() -> Self {
        Autohint { args: None }
    }
}

impl Operation for Autohint {
    fn shortname(&self) -> &str {
        "Autohint"
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
        assert!(inputs.len() == outputs.len());
        let font_filename = inputs[0].to_filename(Some(".ttf"))?;
        let mut args = Args {
            input: font_filename.clone(),
            ..Default::default()
        };
        let our_args = self.args.as_ref().unwrap_or(&"".to_string()).to_string();
        if our_args.contains("--auto-script") {
            let font_bytes = inputs[0].to_bytes()?;
            let fontref = FontRef::new(&font_bytes)?;
            if let Some(script) = primary_script(&fontref, our_args.contains("--discount-latin")) {
                let maybe_script = ScriptClassIndex::from_tag(&script.to_ascii_lowercase());
                if maybe_script.is_err() && our_args.contains("--fail-ok") {
                    log::info!(
                        "Unknown script {} for autohinting, but fail-ok is set, continuing.",
                        script
                    );
                    // Get out now
                    outputs[0].set_contents(std::fs::read(&font_filename)?)?;
                    return Ok(Output {
                        status: std::process::ExitStatus::from_raw(0),
                        stdout: vec![],
                        stderr: vec![],
                    });
                }
                args.default_script = maybe_script.map_err(|e| {
                    ApplicationError::Other(format!("Unknown script for autohinting: {}", e))
                })?;
            }
        }
        match autohint(&args) {
            Ok(hinted_font) => {
                outputs[0].set_contents(hinted_font)?;
            }
            Err(e) if our_args.contains("fail-ok") => {
                log::info!("Autohinting failed but fail-ok is set, continuing: {}", e);
                outputs[0].set_contents(std::fs::read(&font_filename)?)?;
            }
            Err(e) => {
                return Err(ApplicationError::Other(format!(
                    "Autohinting failed: {}",
                    e
                )));
            }
        }
        Ok(Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: vec![],
            stderr: vec![],
        })
    }

    fn description(&self) -> String {
        "Autohint".to_string()
    }
}
