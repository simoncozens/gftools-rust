// Public modules for library usage
pub mod buildsystem;
pub mod error;
pub mod operations;
pub mod recipe;
pub mod recipe_providers;

use error::ApplicationError;
use recipe::Config;
use std::path::{Path, PathBuf};

use crate::recipe::Recipe;

/// Configuration for building fonts
pub struct BuildConfig {
    /// Path to the config file
    pub config_path: String,
    /// Maximum number of parallel jobs
    pub job_limit: usize,
    /// Whether to only generate the recipe (don't build)
    pub generate_only: bool,
    /// Generate graphviz graph
    #[cfg(feature = "graphviz")]
    pub draw_graph: bool,
    /// Generate ASCII graph
    pub ascii_graph: bool,
    /// Materialize intermediate outputs as named files for debugging
    pub debug_intermediates: bool,
    // Verbosity level for logging
    pub verbosity: log::Level,
    /// Whether to show progress bars
    pub progress: bool,
}

impl Default for BuildConfig {
    fn default() -> Self {
        BuildConfig {
            config_path: String::new(),
            job_limit: num_cpus::get(),
            generate_only: false,
            #[cfg(feature = "graphviz")]
            draw_graph: false,
            ascii_graph: false,
            debug_intermediates: false,
            verbosity: log::Level::Info,
            progress: true,
        }
    }
}

/// Load and parse a config file
pub fn load_config(config_path: &str) -> Result<Config, ApplicationError> {
    let config_yaml = std::fs::read_to_string(config_path).map_err(|e| {
        ApplicationError::InvalidRecipe(format!(
            "Could not read config file {}: {}",
            config_path, e
        ))
    })?;

    serde_yaml_ng::from_str::<Config>(&config_yaml).map_err(|e| {
        ApplicationError::InvalidRecipe(format!(
            "Could not parse config file {}: {}",
            config_path, e
        ))
    })
}

/// Change to the config file's directory
pub fn change_to_config_dir(config_path: &str) -> Result<(), ApplicationError> {
    if let Some(config_dir) = Path::new(config_path).parent() {
        // If the config path has no parent (e.g. just "config.yaml"), stay in the current directory
        if !config_dir.as_os_str().is_empty() {
            std::env::set_current_dir(config_dir).map_err(|e| {
                ApplicationError::InvalidRecipe(format!(
                    "Could not change directory to config file's directory {}: {}",
                    config_dir.display(),
                    e
                ))
            })?;
        }
    }
    Ok(())
}

/// Generate a recipe from a config without building
pub fn generate_recipe(config: &Config) -> Result<String, ApplicationError> {
    let recipe = config.recipe()?;
    serde_yaml_ng::to_string(&recipe).map_err(|e| {
        ApplicationError::InvalidRecipe(format!("Could not serialize recipe to YAML: {}", e))
    })
}

/// Generate an ASCII graph of the build process
pub fn generate_ascii_graph(
    recipe: &Recipe,
    verbosity: log::Level,
    debug_intermediates: bool,
) -> Result<String, ApplicationError> {
    let graph = recipe.to_graph(debug_intermediates)?;
    graph.ascii(verbosity)
}

/// Generate an SVG graph of the build process
#[cfg(feature = "graphviz")]
pub fn generate_svg_graph(recipe: &Recipe) -> Result<String, ApplicationError> {
    let graph = recipe.to_graph(false)?;
    graph.draw()
}

/// Main build function that can be called from tests or the binary
pub async fn build(config: BuildConfig) -> Result<(), ApplicationError> {
    let config_yaml = load_config(&config.config_path)?;

    // Hold a guard to the current directory
    let _change_back = ChangeDirGuard::new()?;

    // Change to the config file's directory
    change_to_config_dir(&config.config_path)?;

    // Use block_in_place to run the blocking recipe generation.
    // This tells tokio to park the current task and use another thread from the pool.
    // This avoids the "Cannot drop a runtime" panic that occurs when reqwest::blocking
    // creates/drops a runtime inside an async context.
    let recipe = tokio::task::block_in_place(|| config_yaml.recipe())?;

    if config.generate_only {
        #[derive(serde::Serialize)]
        struct GeneratedRecipe {
            recipe: Recipe,
        }
        let serialized_recipe =
            serde_yaml_ng::to_string(&GeneratedRecipe { recipe }).map_err(|e| {
                ApplicationError::InvalidRecipe(format!(
                    "Could not serialize recipe to YAML: {}",
                    e
                ))
            })?;
        println!("{serialized_recipe}");
        return Ok(());
    }

    #[cfg(feature = "graphviz")]
    if config.draw_graph {
        let graph = generate_svg_graph(&recipe)?;
        std::fs::write("graph.svg", graph).map_err(|e| {
            ApplicationError::InvalidRecipe(format!("Could not write graph to file: {}", e))
        })?;
        println!(
            "Wrote build graph to {}/graph.svg",
            std::env::current_dir().unwrap().display()
        );
        return Ok(());
    }

    // We'll always generate an ASCII graph even if the user doesn't ask
    // for one, because generate_ascii_graph has a nice cycle detector.
    let graph = generate_ascii_graph(&recipe, config.verbosity, config.debug_intermediates)?;
    if config.ascii_graph || graph.starts_with("⚠️") {
        println!("{graph}");
        return Ok(());
    }

    // Use the config to create a build graph
    let graph = recipe.to_graph(config.debug_intermediates)?;
    graph.ensure_directories()?;

    // Run the build
    buildsystem::run(graph, config.job_limit, config.progress).await?;

    Ok(())
}

struct ChangeDirGuard {
    original_dir: PathBuf,
}

impl ChangeDirGuard {
    fn new() -> std::io::Result<Self> {
        let original_dir = std::env::current_dir()?;
        Ok(ChangeDirGuard { original_dir })
    }
}

impl Drop for ChangeDirGuard {
    fn drop(&mut self) {
        // Revert to original directory
        let _ = std::env::set_current_dir(&self.original_dir);
    }
}
