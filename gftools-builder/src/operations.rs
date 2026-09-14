use std::collections::HashMap;

use crate::{
    buildsystem::Operation,
    operations::{addsubset::AddSubsetConfig, fix::FixConfig, fontc::FontcConfig},
    recipe::{ConfigOperation, Step},
};
use babelfont::{DesignLocation, UserLocation};
use itertools::Itertools;
use serde::{Deserialize, Serialize};

pub mod addsubset;
pub mod autohint;
pub mod buildstat;
pub mod compress;
pub mod convert;
pub mod fix;
pub mod fontc;
pub mod glyphs2ufo;
pub mod instantiate_source;
pub mod removeoverlaps;
pub mod subspace;

/// Enum representing the different operation steps available
///
/// This is used during recipe deserialization to map step names to operation implementations.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub(crate) enum OpStep {
    #[serde(rename = "glyphs2ufo")]
    Glyphs2UFO,
    #[serde(rename = "fontc")]
    Fontc,
    #[serde(rename = "fix")]
    Fix,
    #[serde(rename = "buildStat")]
    BuildStat,
    #[serde(rename = "compress")]
    Compress,
    #[serde(rename = "addSubset")]
    AddSubset,
    #[serde(rename = "subspace")]
    Subspace,
    #[serde(rename = "autohint")]
    Autohint,
    #[serde(rename = "instantiateSource")]
    InstantiateSource,
    #[serde(rename = "removeOverlaps")]
    RemoveOverlaps,
}

impl OpStep {
    /// Convert the OpStep enum variant to its corresponding Operation implementation
    pub fn operation(&self) -> Box<dyn Operation> {
        match self {
            OpStep::Fix => Box::new(fix::Fix::new()),
            OpStep::Fontc => Box::new(fontc::Fontc::new()),
            OpStep::Glyphs2UFO => Box::new(glyphs2ufo::Glyphs2UFO),
            OpStep::BuildStat => Box::new(buildstat::BuildStat),
            OpStep::Compress => Box::new(compress::Compress),
            OpStep::AddSubset => Box::new(addsubset::AddSubset::new()),
            OpStep::Subspace => Box::new(subspace::Subspace::new()),
            OpStep::Autohint => Box::new(autohint::Autohint::new()),
            OpStep::InstantiateSource => Box::new(instantiate_source::InstantiateSource::new()),
            OpStep::RemoveOverlaps => Box::new(removeoverlaps::RemoveOverlaps),
        }
    }
}

#[derive(PartialEq, Debug, Clone)]
pub struct ConfigOperationBuilder {
    steps: Vec<Step>,
}
impl ConfigOperationBuilder {
    pub fn new() -> Self {
        ConfigOperationBuilder { steps: vec![] }
    }

    // pub(crate) fn new_from_steps(steps: Vec<Step>) -> Self {
    //     ConfigOperationBuilder { steps }
    // }

    pub fn build(self) -> ConfigOperation {
        ConfigOperation(self.steps)
    }

    pub fn source(mut self, s: String) -> Self {
        self.steps.push(Step::SourceStep {
            source: s,
            extra: HashMap::new(),
        });
        self
    }

    pub fn fix(mut self, config: &FixConfig) -> Self {
        // Serialize FixConfig to a HashMap<String, serde_json::Value>
        let extra = serde_json::to_value(config)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<String, serde_json::Value>>();
        self.steps.push(Step::OperationStep {
            operation: OpStep::Fix,
            extra,
            args: None,
            input_file: None,
            needs: vec![],
        });
        self
    }

    fn to_extra<T>(config: &T) -> HashMap<String, serde_json::Value>
    where
        T: Serialize,
    {
        serde_json::to_value(config)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<String, serde_json::Value>>()
    }

    pub fn compile(mut self, config: &FontcConfig) -> Self {
        let extra = Self::to_extra(config);
        self.steps.push(Step::OperationStep {
            operation: OpStep::Fontc,
            extra,
            args: None,
            input_file: None,
            needs: vec![],
        });
        self
    }

    pub fn compress(mut self) -> Self {
        self.steps.push(Step::OperationStep {
            operation: OpStep::Compress,
            extra: HashMap::new(),
            args: None,
            input_file: None,
            needs: vec![],
        });
        self
    }

    pub fn buildstat(mut self, others: &[String]) -> Self {
        self.steps.push(Step::OperationStep {
            operation: OpStep::BuildStat,
            extra: HashMap::new(),
            args: None,
            input_file: None,
            needs: others.to_vec(),
        });
        self
    }

    pub fn add_subset(mut self, config: &AddSubsetConfig, donor: &str) -> Self {
        let extra = Self::to_extra(config);
        self.steps.push(Step::OperationStep {
            operation: OpStep::AddSubset,
            extra,
            args: None,
            input_file: None,
            needs: vec![donor.to_string()],
        });
        self
    }

    pub fn instance(mut self, location: &UserLocation) -> Self {
        self.steps.push(Step::OperationStep {
            operation: OpStep::Subspace,
            args: Some(
                location
                    .iter()
                    .map(|(axis, value)| format!("{}={}", axis, value.to_f64()))
                    .join(","),
            ),
            input_file: None,
            extra: HashMap::new(),
            needs: vec![],
        });
        self
    }

    pub fn autohint(mut self, args: Option<String>) -> Self {
        self.steps.push(Step::OperationStep {
            operation: OpStep::Autohint,
            extra: HashMap::new(),
            args,
            input_file: None,
            needs: vec![],
        });
        self
    }

    pub fn instantiate_source(mut self, location: &DesignLocation) -> Self {
        self.steps.push(Step::OperationStep {
            operation: OpStep::InstantiateSource,
            extra: HashMap::new(),
            args: Some(
                location
                    .iter()
                    .map(|(axis, value)| format!("{}={}", axis, value.to_f64()))
                    .join(","),
            ),
            input_file: None,
            needs: vec![],
        });
        self
    }

    pub fn remove_overlaps(mut self) -> Self {
        self.steps.push(Step::OperationStep {
            operation: OpStep::RemoveOverlaps,
            extra: HashMap::new(),
            args: None,
            input_file: None,
            needs: vec![],
        });
        self
    }
}

impl Default for ConfigOperationBuilder {
    fn default() -> Self {
        Self::new()
    }
}
