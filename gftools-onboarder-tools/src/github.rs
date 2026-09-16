//! The shared GitHub surface: the authenticated octocrab client, the Traffic Jam
//! board mutations and the google/fonts project-board mutation.
//!
//! Python's `gftools.gfgithub.GitHubClient` is not ported — it is a thin
//! hand-rolled `requests` wrapper, and `octocrab` replaces it at the call sites.
//! What the onboarding tools do share is this client and these mutations, which
//! both the push tools and the packager need.

use gftools::GftoolsError;
use serde_json::{Value, json};

use crate::config::PushConfig;
use crate::push::trafficjam::{PushList, PushStatus};

pub(crate) struct OctocrabHelper<'a> {
    octocrab: octocrab::Octocrab,
    config: &'a PushConfig,
}

pub(crate) fn octocrab_with_auth() -> Result<octocrab::Octocrab, GftoolsError> {
    let token = std::env::var("GH_TOKEN")
        .map_err(|_| GftoolsError::Misc("GH_TOKEN is not set".to_string()))?;
    octocrab::Octocrab::builder()
        .personal_token(token)
        .build()
        .map_err(|e| GftoolsError::Misc(format!("Failed to create Octocrab instance: {}", e)))
}

/// The `updateProjectV2ItemFieldValue` payload.
///
/// The document goes in `query` even though it is a mutation: that is the key
/// GraphQL reads the operation from, and Python's client posts the same shape
/// (`gfgithub._run_graphql`).
fn update_item_payload(project_id: &str, item_id: &str, field_id: &str, option_id: &str) -> Value {
    json!({
        "query": include_str!("mutation.graphql"),
        "variables": {
            "projectId": project_id,
            "itemId": item_id,
            "fieldId": field_id,
            "singleSelectOptionId": option_id,
        }
    })
}

impl<'a> OctocrabHelper<'a> {
    pub fn new(config: &'a PushConfig) -> Result<Self, GftoolsError> {
        let octocrab = octocrab_with_auth()?;
        Ok(Self { octocrab, config })
    }

    pub(crate) async fn update_traffic_jam_status(
        &self,
        item_id: &str,
        server: &PushStatus,
    ) -> Result<(), GftoolsError> {
        let option_id = match server {
            PushStatus::PrGf => self.config.board_meta("pr_gf_id"),
            PushStatus::InDev => self.config.board_meta("in_dev_id"),
            PushStatus::InSandbox => self.config.board_meta("in_sandbox_id"),
            PushStatus::Live => self.config.board_meta("live_id"),
        }?;

        // The document goes in `query` even though it is a mutation: that is the
        // key GraphQL reads the operation from, and Python's client posts the
        // same shape (`gfgithub._run_graphql`).
        let _: Value = self
            .octocrab
            .graphql(&update_item_payload(
                self.config.board_meta("traffic_jam_id")?,
                item_id,
                self.config.board_meta("status_field_id")?,
                option_id,
            ))
            .await
            .map_err(|e| {
                GftoolsError::GitHub(format!("Failed to set the status of {item_id}: {e}"))
            })?;
        Ok(())
    }

    pub(crate) async fn update_traffic_jam_list(
        &self,
        item_id: &str,
        list: &PushList,
    ) -> Result<(), GftoolsError> {
        let list_id = match list {
            PushList::ToSandbox => self.config.board_meta("to_sandbox_id"),
            PushList::ToProduction => self.config.board_meta("to_production_id"),
            PushList::Blocked => self.config.board_meta("blocked_id"),
        }?;
        let _: Value = self
            .octocrab
            .graphql(&update_item_payload(
                self.config.board_meta("traffic_jam_id")?,
                item_id,
                self.config.board_meta("list_field_id")?,
                list_id,
            ))
            .await
            .map_err(|e| {
                GftoolsError::GitHub(format!("Failed to set the list of {item_id}: {e}"))
            })?;
        Ok(())
    }

    pub(crate) async fn update_gf_project(
        &self,
        project_item_id: &str,
        status: &PushStatus,
    ) -> Result<(), GftoolsError> {
        let option_id = match status {
            PushStatus::PrGf => self.config.gf_board_meta("pr_gf_id"),
            PushStatus::InDev => self.config.gf_board_meta("in_dev_id"),
            PushStatus::InSandbox => self.config.gf_board_meta("in_sandbox_id"),
            PushStatus::Live => self.config.gf_board_meta("live_id"),
        }?;
        let _: Value = self
            .octocrab
            .graphql(&update_item_payload(
                self.config.gf_board_meta("board_id")?,
                project_item_id,
                self.config.gf_board_meta("status_field_id")?,
                option_id,
            ))
            .await
            .map_err(|e| {
                GftoolsError::GitHub(format!(
                    "Failed to update google/fonts board item {project_item_id}: {e}"
                ))
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_item_payload() {
        let payload = update_item_payload("PROJECT", "ITEM", "FIELD", "OPTION");
        let document = payload["query"].as_str().unwrap();
        assert!(
            document.starts_with("mutation($projectId: ID!"),
            "{document}"
        );
        assert!(document.contains("updateProjectV2ItemFieldValue"));
        assert_eq!(payload["variables"]["projectId"], "PROJECT");
        assert_eq!(payload["variables"]["itemId"], "ITEM");
        assert_eq!(payload["variables"]["fieldId"], "FIELD");
        assert_eq!(payload["variables"]["singleSelectOptionId"], "OPTION");
    }
}
