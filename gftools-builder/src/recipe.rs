use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use tracing::info_span;

use crate::{
    buildsystem::{BuildGraph, BuildStep},
    error::ApplicationError,
    operations::OpStep,
    recipe_providers::{
        googlefonts::{GoogleFontsOptions, GoogleFontsProvider},
        noto::{NotoOptions, NotoProvider},
    },
};

/// Enum representing different recipe providers as stored in the config file
///
/// This is used during recipe deserialization to handle different provider-specific options.
/// We first determine which provider is requested, then parse its options separately
/// to provide clear error messages.

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
enum RecipeProviderTag {
    #[default]
    GoogleFonts,
    Noto,
}

pub(crate) trait Provider {
    fn generate_recipe(&self) -> Result<Recipe, ApplicationError>;
}

/// Parse provider-specific options with clear error reporting
fn parse_provider_options(
    tag: &RecipeProviderTag,
    raw_config: &serde_yaml_ng::Value,
) -> Result<Box<dyn Provider>, ApplicationError> {
    match tag {
        RecipeProviderTag::GoogleFonts => {
            let options: GoogleFontsOptions = serde_yaml_ng::from_value(raw_config.clone())
                .map_err(|e| {
                    ApplicationError::InvalidRecipe(format!(
                        "Failed to parse GoogleFonts provider options: {}",
                        e
                    ))
                })?;
            Ok(Box::new(GoogleFontsProvider::new(options)))
        }
        RecipeProviderTag::Noto => {
            let options: NotoOptions =
                serde_yaml_ng::from_value(raw_config.clone()).map_err(|e| {
                    ApplicationError::InvalidRecipe(format!(
                        "Failed to parse Noto provider options: {}",
                        e
                    ))
                })?;
            Ok(Box::new(NotoProvider::new(options)))
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
#[serde(untagged)]
pub(crate) enum Step {
    OperationStep {
        operation: OpStep,
        #[serde(default)]
        args: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_file: Option<String>,
        #[serde(flatten, skip_serializing_if = "HashMap::is_empty")]
        extra: HashMap<String, Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        needs: Vec<String>,
    },
    SourceStep {
        source: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },
}

impl Step {
    fn to_operation(&self) -> Result<(Option<String>, BuildStep, Vec<String>), ApplicationError> {
        match self {
            Step::OperationStep {
                operation,
                args,
                extra,
                input_file,
                needs,
            } => {
                let mut op = operation.operation();
                op.set_extra(extra.clone());
                op.set_args(args.clone());
                // Return the needs vector along with the operation
                Ok((input_file.clone(), Arc::new(op), needs.clone()))
            }
            Step::SourceStep { source, extra: _ } => {
                // Handle source step, possibly creating a Source operation
                Err(ApplicationError::InvalidRecipe(format!(
                    "Source step not implemented: {source}"
                )))
            }
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct ConfigOperation(pub(crate) Vec<Step>);

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Default)]
pub struct Recipe(pub HashMap<String, ConfigOperation>);

impl Recipe {
    pub fn new() -> Self {
        Recipe(HashMap::new())
    }
    pub fn extend(&mut self, other: Recipe) {
        for (key, value) in other.0 {
            self.0.insert(key, value);
        }
    }
    pub fn insert(&mut self, key: String, value: ConfigOperation) {
        self.0.insert(key, value);
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn to_graph(&self, debug_intermediates: bool) -> Result<BuildGraph, ApplicationError> {
        let _span = info_span!("generate_graph").entered();
        let mut graph = BuildGraph::new(debug_intermediates);

        // Track dependencies: (step_node, needs_targets)
        let mut dependencies: Vec<(petgraph::graph::NodeIndex, Vec<String>)> = Vec::new();
        let mut source_dependencies: Vec<(petgraph::graph::NodeIndex, String)> = Vec::new();

        for (target, operation) in self.0.iter() {
            // First operation must be a source step
            let source = operation.0.first().ok_or_else(|| {
                ApplicationError::InvalidRecipe(format!("No steps found for target '{target}'"))
            })?;
            let source_filename = if let Step::SourceStep { source, .. } = source {
                Ok(source)
            } else {
                Err(ApplicationError::InvalidRecipe(format!(
                    "First step for target '{target}' must be a source step"
                )))
            }?;

            let operations: Vec<(Option<String>, BuildStep, Vec<String>)> = operation
                .0
                .iter()
                .skip(1)
                .map(|step| step.to_operation())
                .collect::<Result<Vec<_>, ApplicationError>>()?;

            // Extract needs from operations and prepare for add_path
            let operations_for_path: Vec<(Option<String>, BuildStep)> = operations
                .iter()
                .map(|(input, op, _)| (input.clone(), op.clone()))
                .collect();

            // Add the path and get the nodes for each step
            let added_path = graph.add_path(source_filename, operations_for_path, target);

            source_dependencies.push((added_path.entry_node, source_filename.to_string()));

            // Record dependencies with their corresponding nodes
            for (step_idx, (_, _, needs)) in operations.iter().enumerate() {
                if !needs.is_empty() && step_idx < added_path.op_nodes.len() {
                    dependencies.push((added_path.op_nodes[step_idx], needs.clone()));
                }
            }
        }

        // Now add dependency edges
        for (target_node, needs) in dependencies {
            for (slot, need_target) in needs.iter().enumerate() {
                // Input slot starts at 1 because slot 0 is the primary input from the path
                graph.add_dependency(need_target, target_node, slot + 1)?;
            }
        }

        for (target_node, source_target) in source_dependencies {
            graph.add_source_dependency(&source_target, target_node)?;
        }

        Ok(graph)
    }
}

#[derive(Serialize)]
pub struct Config {
    #[serde(default)]
    recipe: Recipe,
    #[serde(skip)]
    provider: Option<Box<dyn Provider>>,
}

// We need to deserialize manually to:
// 1. Separate provider tag detection from options parsing (for better error messages)
// 2. Default to GoogleFonts when no provider and no recipe is specified
// 3. Ignore provider when an explicit recipe is given
impl<'de> Deserialize<'de> for Config {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ConfigHelper {
            #[serde(default)]
            recipe: Recipe,
            #[serde(rename = "recipeProvider", default)]
            recipe_provider_tag: Option<RecipeProviderTag>,
            #[serde(flatten)]
            raw_config: serde_yaml_ng::Value,
        }

        let helper = ConfigHelper::deserialize(deserializer)?;

        // If there's an explicit recipe, don't use a provider
        let provider = if !helper.recipe.is_empty() {
            None
        } else {
            // Determine which provider to use (default to GoogleFonts)
            let tag = helper.recipe_provider_tag.unwrap_or_default();

            // Parse provider-specific options with clear error messages
            Some(
                parse_provider_options(&tag, &helper.raw_config)
                    .map_err(serde::de::Error::custom)?,
            )
        };

        Ok(Config {
            recipe: helper.recipe,
            provider,
        })
    }
}

impl Config {
    pub fn recipe(&self) -> Result<Recipe, ApplicationError> {
        let _span = info_span!("generate_recipe").entered();

        let mut recipe = if let Some(provider) = &self.provider {
            provider.generate_recipe()?
        } else {
            Recipe::new()
        };
        // If the user provided a recipe in the config, overlay it on top.
        recipe.extend(self.recipe.clone());
        Ok(recipe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_untagged_googlefonts() {
        let config = r#"
sources:
    - "Nunito.glyphs"
"#;
        let deserialized: Config =
            serde_yaml_ng::from_str(config).expect("Failed to deserialize YAML");

        // Verify provider was created (we can't inspect its type directly)
        assert!(deserialized.provider.is_some());
        assert!(deserialized.recipe.is_empty());
    }

    #[test]
    fn test_deserialize_explicit_provider() {
        let config = r#"
recipeProvider: googlefonts
sources:
    - "Nunito.glyphs"
"#;
        let deserialized: Config =
            serde_yaml_ng::from_str(config).expect("Failed to deserialize YAML");

        // Verify provider was created
        assert!(deserialized.provider.is_some());
        assert!(deserialized.recipe.is_empty());
    }

    #[test]
    fn test_deserialize_explicit_recipe() {
        let config = r#"
recipe:
    Nunito.designspace:
        - source: "Nunito.glyphs"
        - operation: "glyphs2ufo"
"#;
        let deserialized: Config =
            serde_yaml_ng::from_str(config).expect("Failed to deserialize YAML");

        // When explicit recipe is provided, provider should be None
        assert!(deserialized.provider.is_none());
        assert_eq!(deserialized.recipe.len(), 1);
        assert!(deserialized.recipe.contains_key("Nunito.designspace"));
    }

    #[test]
    fn test_invalid_provider_options() {
        // Test with a field that has the wrong type (sources should be array, not string)
        let config = r#"
recipeProvider: googlefonts
sources: "NotAnArray"
"#;
        let result: Result<Config, _> = serde_yaml_ng::from_str(config);

        // Should fail with clear error message about invalid options
        match result {
            Err(e) => {
                let err_msg = format!("{}", e);
                assert!(
                    err_msg.contains("Failed to parse GoogleFonts provider options"),
                    "Expected error message to contain 'Failed to parse GoogleFonts provider options', got: {}",
                    err_msg
                );
            }
            Ok(_) => panic!("Expected deserialization to fail, but it succeeded"),
        }
    }
}
