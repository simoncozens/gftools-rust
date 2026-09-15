use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::Path,
    sync::{LazyLock, Mutex},
};

use gftools::{GftoolsError, strip_json_guard};

use super::items::{Axis, Designer, Family, FamilyMeta, Item};
use crate::config::PushConfig;

/// Re-exported so the push crate has a single source of truth for this url.
pub use gftools::PROD_FAMILY_DOWNLOAD;

/// A Google Fonts push target: its metadata api, its family download endpoint
/// and its versions manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GfServer {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    dl_url: String,
    #[serde(default)]
    version_url: String,
    #[serde(default)]
    families: HashMap<String, Family>,
    #[serde(default)]
    designers: HashMap<String, Designer>,
    #[serde(default)]
    metadata: HashMap<String, FamilyMeta>,
    #[serde(default)]
    axisregistry: HashMap<String, Axis>,
    /// Loaded by `refresh_versions`; deliberately not persisted, so a saved
    /// cache can never hold a stale versions manifest.
    #[serde(skip, default)]
    _family_versions_data: serde_json::Map<String, Value>,
    #[serde(skip, default)]
    _family_versions: HashMap<String, String>,
}

/// Shared client, so connection pools are reused across calls.
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

/// Per-family metadata, fetched by both `update_family_designers` and
/// `update_metadata` for the same family.
static FAMILY_CACHE: LazyLock<Mutex<HashMap<(String, String), Value>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

async fn cached_gf_server_family_metadata(name: &str, url: &str) -> Result<Value, GftoolsError> {
    let key = (name.to_string(), url.to_string());
    {
        let cache = FAMILY_CACHE.lock().unwrap();
        if let Some(cached_value) = cache.get(&key) {
            return Ok(cached_value.clone());
        }
    }
    let text = HTTP
        .get(format!("{}/{}", url, name.replace(" ", "%20")))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let family_data: Value = serde_json::from_str(strip_json_guard(&text))
        .map_err(|e| GftoolsError::Misc(format!("Failed to parse JSON response: {}", e)))?;
    FAMILY_CACHE
        .lock()
        .unwrap()
        .insert(key, family_data.clone());
    Ok(family_data)
}

/// Every family on a server, keyed by family name, as returned by the
/// `familyMetadataList` endpoint. Cached, and unbounded like Python's
/// `lru_cache`. Used by `push_status` to separate new families from existing
/// ones.
pub async fn gf_server_metadata(url: &str) -> Result<HashMap<String, Value>, GftoolsError> {
    static CACHE: LazyLock<Mutex<HashMap<String, HashMap<String, Value>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    {
        let cache = CACHE.lock().unwrap();
        if let Some(cached) = cache.get(url) {
            return Ok(cached.clone());
        }
    }
    let info: Value = HTTP
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let families: HashMap<String, Value> = info
        .get("familyMetadataList")
        .and_then(|x| x.as_array())
        .ok_or_else(|| GftoolsError::Misc(format!("Failed to find familyMetadataList in {}", url)))?
        .iter()
        .filter_map(|entry| {
            entry
                .get("family")
                .and_then(|f| f.as_str())
                .map(|name| (name.to_string(), entry.clone()))
        })
        .collect();
    CACHE
        .lock()
        .unwrap()
        .insert(url.to_string(), families.clone());
    Ok(families)
}

impl GfServer {
    /// Build a server and load its versions manifest.
    pub async fn new(
        name: &str,
        url: &str,
        dl_url: &str,
        version_url: &str,
    ) -> Result<Self, GftoolsError> {
        let mut server = GfServer {
            name: name.to_string(),
            url: url.to_string(),
            dl_url: dl_url.to_string(),
            version_url: version_url.to_string(),
            ..Default::default()
        };
        server.refresh_versions().await?;
        Ok(server)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Re-read the `familyVersions` manifest. Called on construction, and again
    /// by `update_all` so that a server restored from a saved cache does not
    /// need the network just to be loaded.
    pub async fn refresh_versions(&mut self) -> Result<(), GftoolsError> {
        let text = HTTP
            .get(&self.version_url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let data: Value = serde_json::from_str(strip_json_guard(&text))
            .map_err(|e| GftoolsError::Misc(format!("Failed to parse versions response: {}", e)))?;
        self._family_versions_data = data.as_object().cloned().ok_or_else(|| {
            GftoolsError::Misc(format!("Failed to parse versions response: {}", text))
        })?;
        self._family_versions = self
            ._family_versions_data
            .get("familyVersions")
            .and_then(|x| x.as_array())
            .ok_or(GftoolsError::Misc(format!(
                "Failed to find familyVersions in manifest: {:?}",
                self._family_versions_data
            )))?
            .iter()
            .flat_map(|item| item.as_object())
            .flat_map(|item| {
                let family = item.get("name").and_then(|x| x.as_str());
                let version = item
                    .get("fontVersions")
                    .and_then(|x| x.as_array())
                    .and_then(|x| x.first())
                    .and_then(|x| x.as_object())
                    .and_then(|x| x.get("version"))
                    .and_then(|x| x.as_str());
                if let (Some(family), Some(version)) = (family, version) {
                    Some((family.to_string(), version.to_string()))
                } else {
                    None
                }
            })
            .collect(); // This is four lines of code in Python
        Ok(())
    }

    pub async fn is_online(&self) -> bool {
        HTTP.head(&self.url)
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .is_ok()
    }

    pub fn last_push(&self) -> Result<DateTime<Utc>, GftoolsError> {
        let timestamp = self
            ._family_versions_data
            .get("lastUpdate")
            .and_then(|x| x.as_object())
            .and_then(|x| x.get("seconds"))
            .and_then(|x| x.as_i64())
            .ok_or(GftoolsError::Misc(format!(
                "Failed to find lastUpdate in manifest: {:?}",
                self._family_versions_data
            )))?;
        let last_push = DateTime::from_timestamp(timestamp, 0).ok_or(GftoolsError::Misc(
            format!("Failed to parse lastUpdate timestamp: {}", timestamp),
        ))?;
        Ok(last_push)
    }

    pub fn compare_push_item(&self, item: &Item) -> bool {
        self.find_item(item).as_ref() == Some(item)
    }

    pub fn find_item(&self, item: &Item) -> Option<Item> {
        // I don't like all the clone()s here, but I don't know how to do it better
        match item {
            Item::Family(family) => self
                .families
                .get(&family.name)
                .map(|x| Item::Family(x.clone())),
            Item::Designer(designer) => self
                .designers
                .get(&designer.name)
                .map(|x| Item::Designer(x.clone())),
            Item::FamilyMeta(family_meta) => self
                .metadata
                .get(&family_meta.name)
                .map(|x| Item::FamilyMeta(x.clone())),
            Item::Axis(axis) => self
                .axisregistry
                .get(&axis.tag)
                .map(|x| Item::Axis(x.clone())),
            Item::AxisFallback(_) => None,
        }
    }

    fn update_axis_registry(&mut self, axis_data: &Value) -> Result<(), GftoolsError> {
        for axis in axis_data.as_array().ok_or_else(|| {
            GftoolsError::Misc(format!("Failed to parse axis registry data: {}", axis_data))
        })? {
            let axis_obj = serde_json::from_value::<Axis>(axis.clone()).map_err(|e| {
                GftoolsError::Misc(format!("Failed to parse axis data for {:#?}: {}", axis, e))
            })?;
            self.axisregistry.insert(axis_obj.tag.clone(), axis_obj);
        }
        Ok(())
    }

    /// Returns false when nothing could be recorded, which tells `update` to
    /// skip this family's designers and metadata. Python behaves the same way:
    /// `Family.from_gf` returns `None` rather than raising.
    async fn update_family(&mut self, name: &str) -> Result<bool, GftoolsError> {
        if let Some(version) = self._family_versions.get(name) {
            self.families.insert(
                name.to_string(),
                Family {
                    name: name.to_string(),
                    version: version.clone(),
                },
            );
            return Ok(true);
        }
        match Family::from_googlefonts(name, &self.dl_url).await {
            Ok(family) => {
                self.families.insert(name.to_string(), family);
                Ok(true)
            }
            Err(e) => {
                log::warn!("Could not fetch {} from {}: {}", name, self.dl_url, e);
                Ok(false)
            }
        }
    }

    async fn update_family_designers(&mut self, name: &str) -> Result<(), GftoolsError> {
        let meta: Value = cached_gf_server_family_metadata(name, &self.url).await?;
        for designer_value in
            meta.get("designers")
                .and_then(|x| x.as_array())
                .ok_or(GftoolsError::Misc(format!(
                    "Failed to parse designers data: {}",
                    meta
                )))?
        {
            let designer = serde_json_path_to_error::from_value::<Designer>(designer_value.clone())
                .map_err(|e| {
                    GftoolsError::Misc(format!(
                        "Failed to parse designer data for {:?}: {}",
                        designer_value.get("name"),
                        e
                    ))
                })?;
            self.designers.insert(designer.name.clone(), designer);
        }
        Ok(())
    }

    async fn update_metadata(&mut self, name: &str) -> Result<(), GftoolsError> {
        let meta: Value = cached_gf_server_family_metadata(name, &self.url).await?;
        let family_meta: FamilyMeta = serde_json_path_to_error::from_value(meta)?;
        // Key by the name the server reports, as Python does.
        self.metadata.insert(family_meta.name.clone(), family_meta);
        Ok(())
    }

    async fn fetch_metadata_root(&self) -> Result<Value, GftoolsError> {
        let text = HTTP
            .get(&self.url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        serde_json::from_str(strip_json_guard(&text))
            .map_err(|e| GftoolsError::Misc(format!("Failed to parse JSON response: {}", e)))
    }

    /// Refresh the axis registry, plus every family whose version changed or
    /// that was modified on or after `last_checked`.
    pub async fn update_all(&mut self, last_checked: &NaiveDate) -> Result<(), GftoolsError> {
        // The versions manifest may be missing, e.g. after `open`, and the
        // version comparison below depends on it.
        self.refresh_versions().await?;
        let parsed_meta = self.fetch_metadata_root().await?;
        let meta = parsed_meta.as_object().ok_or_else(|| {
            GftoolsError::Misc(format!("Failed to parse JSON response: {}", parsed_meta))
        })?;
        let axis_data = meta.get("axisRegistry").ok_or(GftoolsError::Misc(format!(
            "Failed to find axisRegistry in manifest: {:?}",
            meta
        )))?;
        self.update_axis_registry(axis_data)?;
        let families_data = meta
            .get("familyMetadataList")
            .ok_or(GftoolsError::Misc(format!(
                "Failed to find familyMetadataList in manifest: {:?}",
                meta
            )))?
            .as_array()
            .ok_or(GftoolsError::Misc(format!(
                "Failed to parse familyMetadataList data: {:?}",
                meta
            )))?;
        for family_data in families_data {
            let family_data = family_data.as_object().ok_or(GftoolsError::Misc(format!(
                "Failed to parse familyMetadataList data: {}",
                family_data
            )))?;
            let name =
                family_data
                    .get("family")
                    .and_then(|x| x.as_str())
                    .ok_or(GftoolsError::Misc(format!(
                        "Failed to parse family name: {:?}",
                        family_data
                    )))?;
            // A leftover test family which Google Fonts keeps in the list.
            if name == "Roboto_old" {
                continue;
            }
            let last_modified_str = family_data
                .get("lastModified")
                .and_then(|x| x.as_str())
                .ok_or(GftoolsError::Misc(format!(
                    "Failed to parse lastModified: {:?}",
                    family_data
                )))?;
            let last_modified = NaiveDate::parse_from_str(last_modified_str, "%Y-%m-%d")
                .map_err(|e| GftoolsError::Misc(format!("Failed to parse lastModified: {}", e)))?;

            let cached_family_version = self._family_versions.get(name);
            let existing_family_version = self.families.get(name).map(|f| &f.version);
            // When both versions are known, only a version change matters.
            // Otherwise fall back to the modification date.
            let should_update = match (cached_family_version, existing_family_version) {
                (Some(cached), Some(existing)) => cached != existing,
                _ => last_modified >= *last_checked,
            };
            if should_update {
                self.update(name).await?;
            }
        }
        Ok(())
    }

    pub async fn update(&mut self, family_name: &str) -> Result<(), GftoolsError> {
        log::info!("Updating family: {}", family_name);
        if self.update_family(family_name).await? {
            self.update_family_designers(family_name).await?;
            self.update_metadata(family_name).await?;
        }
        Ok(())
    }
}

/// The servers the push workflow targets, plus when they were last checked.
///
/// `dev` is deliberately absent: only `manage_traffic_jam` needs it, so that
/// script builds one from the config itself. (Python's `GFServers` has no `dev`
/// either, even though `manage_traffic_jam` tries to read one.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GfServers {
    pub sandbox: GfServer,
    pub production: GfServer,
    pub last_checked: NaiveDate,
}

impl GfServers {
    /// Build from `config`. Contacts both servers' versions endpoints.
    pub async fn new(config: &PushConfig) -> Result<Self, GftoolsError> {
        Ok(GfServers {
            sandbox: GfServer::new(
                "sandbox",
                config.url("sandbox_meta")?,
                config.url("sandbox_family_download")?,
                config.url("sandbox_versions")?,
            )
            .await?,
            production: GfServer::new(
                "production",
                config.url("production_meta")?,
                PROD_FAMILY_DOWNLOAD,
                config.url("production_versions")?,
            )
            .await?,
            last_checked: Utc::now().date_naive(),
        })
    }

    pub fn last_pushes(&self) -> Result<(), GftoolsError> {
        log::info!(
            "Last pushes for each server:\nSandbox: {}\nProduction: {}",
            self.sandbox.last_push()?,
            self.production.last_push()?
        );
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = &GfServer> {
        vec![&self.sandbox, &self.production].into_iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut GfServer> {
        vec![&mut self.sandbox, &mut self.production].into_iter()
    }

    pub async fn servers_online(&self) -> Result<(), GftoolsError> {
        for server in self.iter() {
            if !server.is_online().await {
                return Err(GftoolsError::Misc(format!(
                    "Server {} is offline",
                    server.name
                )));
            }
        }
        Ok(())
    }

    pub async fn update_all(&mut self) -> Result<(), GftoolsError> {
        let last_checked = self.last_checked;
        for server in self.iter_mut() {
            server.update_all(&last_checked).await?;
        }
        self.last_checked = Utc::now().date_naive();
        Ok(())
    }

    /// Update a single family on every server, logging rather than propagating
    /// per-server failures.
    pub async fn update(&mut self, family_name: &str) {
        for server in self.iter_mut() {
            if let Err(e) = server.update(family_name).await {
                log::error!("Error updating {} on {}: {}", family_name, server.name, e);
            }
        }
    }

    /// `item`'s JSON, plus whether each server already has it.
    pub fn compare_item(&self, item: &Item) -> serde_json::Map<String, Value> {
        let mut res = match item.to_json() {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        for server in self.iter() {
            res.insert(
                format!("In {}", server.name),
                Value::Bool(server.compare_push_item(item)),
            );
        }
        res
    }

    /// Read a saved cache. Never touches the network: the versions manifests are
    /// refreshed by `update_all`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, GftoolsError> {
        let contents = std::fs::read_to_string(path)?;
        Self::from_json(&contents)
    }

    pub fn from_json(json: &str) -> Result<Self, GftoolsError> {
        let mut servers: Self = serde_json_path_to_error::from_str(json)?;
        servers.stamp_server_names();
        Ok(servers)
    }

    pub fn from_dict(value: Value) -> Result<Self, GftoolsError> {
        let mut servers: Self = serde_json_path_to_error::from_value(value)?;
        servers.stamp_server_names();
        Ok(servers)
    }

    /// Server names belong to the struct rather than to the file, so a partial
    /// or hand-written document still yields usable servers. Python gets the
    /// same effect by overlaying the file onto an already-constructed object.
    fn stamp_server_names(&mut self) {
        self.sandbox.name = "sandbox".to_string();
        self.production.name = "production".to_string();
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), GftoolsError> {
        let path = path.as_ref();
        let json = serde_json_path_to_error::to_string_pretty(&self)?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Equivalent to the `DATA` fixture in `python/tests/push/test_servers.py`:
    /// a partial document, which is all `from_json` needs. No network access.
    const DATA: &str = r#"{
        "sandbox": {"families": {"Abel": {"name": "Abel", "version": "0.999"}}},
        "production": {"families": {"Abel": {"name": "Abel", "version": "0.999"}}},
        "last_checked": "2023-01-01"
    }"#;

    fn servers() -> GfServers {
        GfServers::from_json(DATA).unwrap()
    }

    fn abel(version: &str) -> Item {
        Item::Family(Family {
            name: "Abel".to_string(),
            version: version.to_string(),
        })
    }

    #[test]
    fn test_iter() {
        assert_eq!(
            servers().iter().map(|s| s.name()).collect::<Vec<_>>(),
            vec!["sandbox", "production"]
        );
    }

    #[test]
    fn test_compare_item() {
        let comparison = servers().compare_item(&abel("1.000"));
        assert_eq!(comparison["name"], Value::String("Abel".to_string()));
        assert_eq!(comparison["version"], Value::String("1.000".to_string()));
        assert_eq!(comparison["In sandbox"], Value::Bool(false));
        assert_eq!(comparison["In production"], Value::Bool(false));
    }

    #[test]
    fn test_compare_item_match() {
        let comparison = servers().compare_item(&abel("0.999"));
        assert_eq!(comparison["In sandbox"], Value::Bool(true));
        assert_eq!(comparison["In production"], Value::Bool(true));
    }

    #[test]
    fn test_save_and_open_round_trip() {
        let servers = servers();
        let json = serde_json_path_to_error::to_string_pretty(&servers).unwrap();
        let reopened = GfServers::from_json(&json).unwrap();
        assert_eq!(
            reopened.compare_item(&abel("0.999")),
            servers.compare_item(&abel("0.999"))
        );
    }

    /// Manual smoke test: needs `~/.gf_push_config.toml` (or `$GF_PUSH_CONFIG`)
    /// and network access, and writes `test.json` into the cwd.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires a local push config and network access"]
    async fn test_live_servers() {
        env_logger::init();

        let config = PushConfig::load_default().unwrap();
        let mut servers = GfServers::new(&config).await.unwrap();
        servers.update_all().await.unwrap();
        servers.save("test.json").unwrap();
    }
}
