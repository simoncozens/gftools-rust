use std::process::Output;

use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};

#[derive(PartialEq, Debug)]
pub(crate) struct Glyphs2UFO;

impl Operation for Glyphs2UFO {
    fn shortname(&self) -> &str {
        "Glyphs2UFO"
    }

    fn input_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Path]
    }

    fn output_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Path]
    }

    fn execute(
        &self,
        inputs: &[OperationOutput],
        outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError> {
        let cmd = format!(
            "fontmake -o ufo -i --instance-dir instance_ufo -g {}",
            inputs[0].to_filename(Some(".glyphs"))?
        );
        self.run_shell_command(&cmd, outputs)
    }
    fn description(&self) -> String {
        "Convert glyphs file to UFO format".to_string()
    }
}
