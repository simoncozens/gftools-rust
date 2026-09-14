use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::Path,
    sync::{LazyLock, Mutex},
};
use toml::Table;

use gftools::GftoolsError;

use super::items::{Axis, Designer, Family, FamilyMeta, PushItem};

pub(crate) const PROD_FAMILY_DOWNLOAD: &str = "https://fonts.google.com/download?family={}";

#[derive(Default, Serialize, Deserialize)]
struct GfServer {
    name: String,
    url: String,
    dl_url: String,
    version_url: String,
    families: HashMap<String, Family>,
    designers: HashMap<String, Designer>,
    family_meta: HashMap<String, FamilyMeta>,
    axisregistry: HashMap<String, Axis>,
    #[serde(skip, default)]
    _family_versions_data: serde_json::Map<String, Value>,
    #[serde(skip, default)]
    _family_versions: HashMap<String, String>,
}

static CACHE: LazyLock<Mutex<HashMap<(String, String), Value>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cached_gf_server_family_metadata(name: &str, url: &str) -> Result<Value, GftoolsError> {
    let mut cache = CACHE.lock().unwrap();
    if let Some(cached_value) = cache.get(&(name.to_string(), url.to_string())) {
        return Ok(cached_value.clone());
    }
    let request =
        reqwest::blocking::Client::new().get(format!("{}/{}", url, name.replace(" ", "%20")));
    let response = request.send().and_then(|r| r.error_for_status())?;
    let text = response.text()?.replace(")]}'", "");
    let family_data: Value = serde_json::from_str(&text)
        .map_err(|e| GftoolsError::Misc(format!("Failed to parse JSON response: {}", e)))?;
    cache.insert((name.to_string(), url.to_string()), family_data.clone());
    Ok(family_data)
}

impl GfServer {
    pub fn new(
        name: String,
        url: String,
        dl_url: String,
        version_url: String,
    ) -> Result<Self, GftoolsError> {
        let mut server = GfServer {
            name,
            url,
            dl_url,
            version_url,
            ..Default::default()
        };
        let request = reqwest::blocking::Client::new().get(&server.version_url);
        let response = request.send().and_then(|r| r.error_for_status())?;
        let text = response.text()?;
        let json_text = text.chars().skip(5).collect::<String>();
        let family_data: serde_json::Value = serde_json::from_str(&json_text)
            .map_err(|e| GftoolsError::Misc(format!("Failed to parse JSON response: {}", e)))?;
        server._family_versions_data = family_data
            .as_object()
            .ok_or(GftoolsError::Misc(format!(
                "Failed to parse JSON response: {}",
                text
            )))?
            .clone();
        server._family_versions = server
            ._family_versions_data
            .get("familyVersions")
            .and_then(|x| x.as_array())
            .ok_or(GftoolsError::Misc(format!(
                "Failed to find familyVersions in manifest: {:?}",
                server._family_versions_data
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

        Ok(server)
    }

    fn is_online(&self) -> bool {
        let request = reqwest::blocking::Client::new().head(&self.url);
        request
            .send()
            .and_then(|response| response.error_for_status())
            .is_ok()
    }

    fn last_push(&self) -> Result<chrono::DateTime<Utc>, GftoolsError> {
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

    fn compare_push_item(&self, item: &PushItem) -> bool {
        self.find_item(item).as_ref() == Some(item)
    }

    fn find_item(&self, item: &PushItem) -> Option<PushItem> {
        // I don't like all the clone()s here, but I don't know how to do it better
        match item {
            PushItem::Family(family) => self
                .families
                .get(&family.name)
                .map(|x| PushItem::Family(x.clone())),
            PushItem::Designer(designer) => self
                .designers
                .get(&designer.name)
                .map(|x| PushItem::Designer(x.clone())),
            PushItem::FamilyMeta(family_meta) => self
                .family_meta
                .get(&family_meta.name)
                .map(|x| PushItem::FamilyMeta(x.clone())),
            PushItem::Axis(axis) => self
                .axisregistry
                .get(&axis.tag)
                .map(|x| PushItem::Axis(x.clone())),
            PushItem::AxisFallback(_) => None,
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

    fn update_family(&mut self, name: &str) -> Result<(), GftoolsError> {
        if let Some(version) = self._family_versions.get(name) {
            self.families.insert(
                name.to_string(),
                Family {
                    name: name.to_string(),
                    version: version.clone(),
                },
            );
        } else {
            let family = Family::from_googlefonts(name, &self.dl_url)?;
            self.families.insert(name.to_string(), family);
        }
        Ok(())
    }

    fn update_family_designers(&mut self, name: &str) -> Result<(), GftoolsError> {
        let meta: Value = cached_gf_server_family_metadata(name, &self.url)?;
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

    fn update_metadata(&mut self, name: &str) -> Result<(), GftoolsError> {
        let meta: Value = cached_gf_server_family_metadata(name, &self.url)?;
        let family_meta: FamilyMeta = serde_json_path_to_error::from_value(meta.clone())?;
        self.family_meta.insert(name.to_string(), family_meta);
        Ok(())
    }

    fn update_all(&mut self, last_checked: &chrono::NaiveDate) -> Result<(), GftoolsError> {
        let request = reqwest::blocking::Client::new().get(&self.url);
        let response = request.send().and_then(|r| r.error_for_status())?;
        let text = response.text()?;
        let parsed_meta = serde_json_path_to_error::from_str::<Value>(&text)?;
        let meta = parsed_meta.as_object().ok_or_else(|| {
            GftoolsError::Misc(format!("Failed to parse JSON response: {}", text))
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
            let last_modified_str = family_data
                .get("lastModified")
                .and_then(|x| x.as_str())
                .ok_or(GftoolsError::Misc(format!(
                    "Failed to parse lastModified: {:?}",
                    family_data
                )))?;
            let last_modified = chrono::NaiveDate::parse_from_str(last_modified_str, "%Y-%m-%d")
                .map_err(|e| GftoolsError::Misc(format!("Failed to parse lastModified: {}", e)))?;

            let cached_family_version = self._family_versions.get(name).map(|x| x.as_str());
            let existing_family_version = self.families.get(name).map(|x| x.version.as_str());
            if (cached_family_version != existing_family_version) || &last_modified > last_checked {
                self.update(name)?;
            }
        }

        Ok(())
    }

    fn update(&mut self, family_name: &str) -> Result<(), GftoolsError> {
        log::info!("Updating family: {}", family_name);
        self.update_family(family_name)?;
        self.update_family_designers(family_name)?;
        self.update_metadata(family_name)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct GfServers {
    dev: GfServer,
    sandbox: GfServer,
    production: GfServer,
    last_checked: NaiveDateTime,
}

impl GfServers {
    fn new(config_file: impl AsRef<Path>) -> Result<Self, GftoolsError> {
        let config_file: &Path = config_file.as_ref();
        let config = std::fs::read_to_string(config_file)?;
        let config: Table = toml::de::from_str(&config)?;
        let urls = config
            .get("urls")
            .and_then(|x| x.as_table())
            .ok_or(GftoolsError::Misc(
                "Failed to find urls in config file".to_string(),
            ))?;
        let dev = GfServer::new(
            "dev".to_string(),
            urls.get("dev_meta")
                .and_then(|x| x.as_str())
                .ok_or(GftoolsError::Misc(
                    "Failed to find dev_meta in config file".to_string(),
                ))?
                .to_string(),
            urls.get("dev_family_download")
                .and_then(|x| x.as_str())
                .ok_or(GftoolsError::Misc(
                    "Failed to find dev_family_download in config file".to_string(),
                ))?
                .to_string(),
            urls.get("dev_versions")
                .and_then(|x| x.as_str())
                .ok_or(GftoolsError::Misc(
                    "Failed to find dev_versions in config file".to_string(),
                ))?
                .to_string(),
        )?;
        let sandbox = GfServer::new(
            "sandbox".to_string(),
            urls.get("sandbox_meta")
                .and_then(|x| x.as_str())
                .ok_or(GftoolsError::Misc(
                    "Failed to find sandbox_meta in config file".to_string(),
                ))?
                .to_string(),
            urls.get("sandbox_family_download")
                .and_then(|x| x.as_str())
                .ok_or(GftoolsError::Misc(
                    "Failed to find sandbox_family_download in config file".to_string(),
                ))?
                .to_string(),
            urls.get("sandbox_versions")
                .and_then(|x| x.as_str())
                .ok_or(GftoolsError::Misc(
                    "Failed to find sandbox_versions in config file".to_string(),
                ))?
                .to_string(),
        )?;
        let production = GfServer::new(
            "production".to_string(),
            urls.get("production_meta")
                .and_then(|x| x.as_str())
                .ok_or(GftoolsError::Misc(
                    "Failed to find production_meta in config file".to_string(),
                ))?
                .to_string(),
            PROD_FAMILY_DOWNLOAD.to_string(),
            urls.get("production_versions")
                .and_then(|x| x.as_str())
                .ok_or(GftoolsError::Misc(
                    "Failed to find production_versions in config file".to_string(),
                ))?
                .to_string(),
        )?;
        Ok(GfServers {
            dev,
            sandbox,
            production,
            last_checked: chrono::Utc::now().naive_utc(),
        })
    }

    fn last_pushes(&self) -> Result<(), GftoolsError> {
        log::info!(
            "Last push: dev: {}, sandbox: {}, production: {}",
            self.dev.last_push()?,
            self.sandbox.last_push()?,
            self.production.last_push()?
        );
        Ok(())
    }

    fn iter(&self) -> impl Iterator<Item = &GfServer> {
        vec![&self.dev, &self.sandbox, &self.production].into_iter()
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = &mut GfServer> {
        vec![&mut self.dev, &mut self.sandbox, &mut self.production].into_iter()
    }

    fn servers_online(&self) -> Result<(), GftoolsError> {
        for server in self.iter() {
            if !server.is_online() {
                return Err(GftoolsError::Misc(format!(
                    "Server {} is offline",
                    server.name
                )));
            }
        }
        Ok(())
    }

    fn update_all(&mut self) -> Result<(), GftoolsError> {
        let last_checked = self.last_checked;
        for server in self.iter_mut() {
            server.update_all(&last_checked.into())?;
        }
        self.last_checked = Utc::now().naive_utc();
        Ok(())
    }

    fn save(&self, path: impl AsRef<Path>) -> Result<(), GftoolsError> {
        let path = path.as_ref();
        let json = serde_json_path_to_error::to_string_pretty(&self)?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expanduser::expanduser;

    #[test]
    fn test_name() {
        env_logger::init();

        let mut servers = GfServers::new(expanduser("~/.gf_push_config.ini").unwrap()).unwrap();
        servers.update_all().unwrap();
        servers.save("test.json").unwrap();
    }
}
