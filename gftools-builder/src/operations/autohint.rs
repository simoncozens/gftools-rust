use std::{os::unix::process::ExitStatusExt, process::Output};

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};
use tilvisan::{Args, autohint};

#[derive(PartialEq, Debug, Default)]
pub(crate) struct Autohint {
    fail_ok: bool,
}

impl Autohint {
    pub fn new() -> Self {
        Autohint { fail_ok: false }
    }
}

impl Operation for Autohint {
    fn shortname(&self) -> &str {
        "Autohint"
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
        let args = Args {
            input: font_filename.clone(),
            ..Default::default()
        };
        match autohint(&args) {
            Ok(hinted_font) => {
                outputs[0].set_contents(hinted_font)?;
            }
            Err(e) if self.fail_ok => {
                log::info!("Autohinting failed but fail_ok is set, continuing: {}", e);
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

    fn set_args(&mut self, args: Option<String>) {
        if args.is_some_and(|a| a.contains("fail-ok")) {
            self.fail_ok = true;
        }
    }
}
