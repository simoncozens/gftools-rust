use crate::{
    buildsystem::{DataKind, Operation, OperationOutput},
    error::ApplicationError,
};
use skeravar::{FontRef, GlyphId, IntSet, Plan, parse_instancing_spec, subset_font};
use std::process::Output;
use tracing::info_span;

#[derive(PartialEq, Debug)]
pub(crate) struct Subspace {
    args: Option<String>,
}

impl Subspace {
    pub fn new() -> Self {
        Subspace { args: None }
    }
}

impl Operation for Subspace {
    fn shortname(&self) -> &str {
        "subspace"
    }

    fn input_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::Bytes]
    }

    fn output_kinds(&self) -> Vec<DataKind> {
        vec![DataKind::BinaryFont]
    }

    fn execute(
        &self,
        inputs: &[OperationOutput],
        outputs: &[OperationOutput],
    ) -> Result<Output, ApplicationError> {
        let _span = info_span!("subspace").entered();
        let bytes = inputs[0].to_bytes()?;
        let spec = parse_instancing_spec(self.args.as_deref().unwrap_or("")).unwrap();

        let fontref = FontRef::new(&bytes)
            .map_err(|e| ApplicationError::Other(format!("Failed to create FontRef: {}", e)))?;
        let plan = Plan::new(
            &IntSet::<GlyphId>::all(),
            &IntSet::<u32>::all(),
            &fontref,
            skeravar::SubsetFlags::SUBSET_FLAGS_DEFAULT
                | skeravar::SubsetFlags::SUBSET_FLAGS_UPDATE_NAME_TABLE
                | skeravar::SubsetFlags::SUBSET_FLAGS_GLYPH_NAMES,
            &IntSet::empty(),
            &IntSet::all(),
            &IntSet::all(),
            &IntSet::all(),
            &IntSet::<u16>::all(),
            &Some(spec),
        );

        match subset_font(&fontref, &plan) {
            Ok(bytes) => {
                outputs[0].set_contents(bytes)?;
                Ok(Output {
                    status: std::process::ExitStatus::default(),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                })
            }
            Err(e) => Err(ApplicationError::Other(format!("subspace failed: {}", e))),
        }
    }

    fn description(&self) -> String {
        format!("Subspace to {}", self.args.as_deref().unwrap_or(""))
    }

    fn set_args(&mut self, args: Option<String>) {
        self.args = args;
    }

    fn identifier(&self) -> String {
        format!("subspace-{}", self.args.as_deref().unwrap_or(""))
    }
}
