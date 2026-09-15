//! Loading of `~/.gf_push_config.toml`.
//!
//! Python accepted a `.gf_push_config.ini` file and rewrote it as TOML on first
//! use, falling back to environment variables when neither file existed. That
//! migration is deliberately not ported: the TOML is assumed to exist already,
//! and the environment-variable fallback goes with it.
//!
//! Expected layout:
//!
//! ```toml
//! [urls]
//! sandbox_meta = "..."
//! sandbox_family_download = "..."
//! sandbox_versions = "..."
//! production_meta = "..."
//! production_versions = "..."
//!
//! [board_meta]              # the Traffic Jam board
//! traffic_jam_id = "..."
//! status_field_id = "..."
//! list_field_id = "..."
//! pr_gf_id = "..."
//! in_dev_id = "..."
//! in_sandbox_id = "..."
//! live_id = "..."
//! to_sandbox_id = "..."
//! to_production_id = "..."
//! blocked_id = "..."
//!
//! [gf_board_meta]           # the google/fonts board
//! board_id = "..."
//! status_field_id = "..."
//! pr_gf_id = "..."
//! in_dev_id = "..."
//! in_sandbox_id = "..."
//! live_id = "..."
//! ```

use std::path::{Path, PathBuf};

use gftools::GftoolsError;
use toml::Table;

/// Filename looked for in the user's home directory.
pub const CONFIG_FILENAME: &str = ".gf_push_config.toml";

/// Environment variable that overrides the config file location.
pub const CONFIG_ENV_VAR: &str = "GF_PUSH_CONFIG";

/// The parsed push configuration.
#[derive(Debug, Clone)]
pub struct PushConfig {
    table: Table,
    path: PathBuf,
}

impl PushConfig {
    /// Load the configuration from `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, GftoolsError> {
        let path = path.as_ref().to_path_buf();
        let contents = std::fs::read_to_string(&path).map_err(|e| {
            GftoolsError::Misc(format!(
                "Could not read push config {}: {}",
                path.display(),
                e
            ))
        })?;
        let table: Table = toml::de::from_str(&contents)?;
        Ok(Self { table, path })
    }

    /// Load from `$GF_PUSH_CONFIG`, or `~/.gf_push_config.toml`.
    pub fn load_default() -> Result<Self, GftoolsError> {
        Self::load(default_config_path()?)
    }

    /// Path this configuration was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A value from the `[urls]` table.
    pub fn url(&self, key: &str) -> Result<&str, GftoolsError> {
        Self::lookup(self.section("urls")?, key, &self.path)
    }

    /// A value from the `[board_meta]` table (the Traffic Jam board).
    pub fn board_meta(&self, key: &str) -> Result<&str, GftoolsError> {
        Self::lookup(self.section("board_meta")?, key, &self.path)
    }

    /// A value from the `[gf_board_meta]` table (the google/fonts board).
    pub fn gf_board_meta(&self, key: &str) -> Result<&str, GftoolsError> {
        Self::lookup(self.section("gf_board_meta")?, key, &self.path)
    }

    fn section(&self, name: &str) -> Result<&Table, GftoolsError> {
        self.table
            .get(name)
            .and_then(|section| section.as_table())
            .ok_or_else(|| {
                GftoolsError::Misc(format!(
                    "Push config {} has no [{}] table",
                    self.path.display(),
                    name
                ))
            })
    }

    fn lookup<'a>(table: &'a Table, key: &str, path: &Path) -> Result<&'a str, GftoolsError> {
        table
            .get(key)
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                GftoolsError::Misc(format!(
                    "Push config {} has no '{}' entry",
                    path.display(),
                    key
                ))
            })
    }
}

/// `$GF_PUSH_CONFIG`, else `~/.gf_push_config.toml`.
pub fn default_config_path() -> Result<PathBuf, GftoolsError> {
    if let Some(path) = std::env::var_os(CONFIG_ENV_VAR) {
        return Ok(PathBuf::from(path));
    }
    let home = dirs::home_dir()
        .ok_or_else(|| GftoolsError::Misc("Could not determine the home directory".to_string()))?;
    Ok(home.join(CONFIG_FILENAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[urls]
sandbox_meta = "https://sandbox.example/api"
sandbox_family_download = "https://sandbox.example/download?family={}"

[board_meta]
traffic_jam_id = "TJ"

[gf_board_meta]
board_id = "GF"
"#;

    fn config() -> PushConfig {
        PushConfig {
            table: toml::de::from_str(SAMPLE).unwrap(),
            path: PathBuf::from("test.toml"),
        }
    }

    #[test]
    fn test_lookup() {
        let config = config();
        assert_eq!(
            config.url("sandbox_meta").unwrap(),
            "https://sandbox.example/api"
        );
        assert_eq!(config.board_meta("traffic_jam_id").unwrap(), "TJ");
        assert_eq!(config.gf_board_meta("board_id").unwrap(), "GF");
    }

    #[test]
    fn test_missing_entries_are_errors() {
        let config = config();
        // Present table, absent key.
        assert!(config.url("production_meta").is_err());
        // Absent table.
        assert!(config.board_meta("nope").is_err());
    }
}
