use crate::{buildsystem::OperationOutput, error::ApplicationError};
use async_trait::async_trait;
use serde_json::Value;
use std::{collections::HashMap, process::Output};

/// Logical data kind that operations consume/produce
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataKind {
    Any,
    /// Any filesystem path (named or temporary)
    Path,
    /// Raw in-memory bytes
    Bytes,
    /// Babelfont source font object
    SourceFont,
    /// Binary TrueType font (e.g., via skrifa::FontRef)
    BinaryFont,
}

/// Trait representing a build operation
///
/// An operation is a node in the build graph that takes some inputs and produces some outputs.
/// The implementation of the Operation trait determines the behavior of the operation.
#[async_trait]
pub trait Operation: Send + Sync {
    fn execute(
        &self,
        inputs: &[OperationOutput],
        outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError>;
    fn description(&self) -> String;
    fn shortname(&self) -> &str;
    /// Return any machine-readable identifer for this operation and any parameters.
    ///
    /// This function should return an identifier with enough information to
    /// tell you whether this operation
    /// can be reused between targets. For example, we might want to use
    /// the "addSubset" operation to generate fonts B and C from font A.
    /// We can't just check the short name in that case because the parameters
    /// might be different.
    fn identifier(&self) -> String {
        self.shortname().to_string()
    }

    fn set_args(&mut self, _args: Option<String>) {
        // Default implementation does nothing.
    }
    fn set_extra(&mut self, _extra: HashMap<String, Value>) {
        // Default implementation does nothing.
    }

    /// Whether this operation should be hidden from user-facing output.
    fn hidden(&self) -> bool {
        false
    }

    fn run_shell_command(
        &self,
        cmd: &str,
        _outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError> {
        log::debug!("Running shell command: {}", cmd);
        let process_output = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .map_err(|e| ApplicationError::Other(e.to_string()))?;
        Ok(process_output)
    }

    /// Declare the input kinds for this operation (one per input slot).
    /// Defaults to a single `Any` input, meaning no constraints.
    fn input_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Any]
    }
    /// Declare the output kinds for this operation (one per output slot).
    /// Defaults to a single `Any` output, meaning unspecified.
    fn output_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Any]
    }
    // You might want this later

    // #[allow(dead_code)]
    // async fn run_cross_platform(command: &str) -> Result<Output, std::io::Error> {
    //     if cfg!(target_os = "windows") {
    //         let components = command.split_whitespace().collect::<Vec<_>>();
    //         Command::new(components[0])
    //             .args(&components[1..])
    //             .output()
    //             .await
    //     } else {
    //         Command::new("sh").arg("-ec").arg(command).output().await
    //     }
    // }
}

impl std::fmt::Debug for dyn Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.shortname())
    }
}

impl std::fmt::Display for dyn Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.shortname())
    }
}
impl std::cmp::PartialEq for dyn Operation {
    fn eq(&self, other: &Self) -> bool {
        self.identifier() == other.identifier()
    }
}
