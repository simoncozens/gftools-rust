//! Port of `gftools.push.trafficjam`.
//!
//! A [`PushItem`] is one row of the Traffic Jam: a path in google/fonts, a
//! category, a status, and the pull request it came from. [`PushItems`] is the
//! collection, with the path-normalisation rules that turn a list of changed
//! files into a list of things to push, plus the board mutations which move an
//! item's status and list along.

use std::collections::HashSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};

use futures_util::TryStreamExt;
use gftools::GftoolsError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::pin;

use crate::config::PushConfig;
use crate::github::{OctocrabHelper, octocrab_with_auth};
use crate::push::items::{Axis, Designer, Family, FamilyMeta, Item};
use crate::push::utils::{google_path_to_repo_path, repo_path_to_google_path};

struct BoardItemsFromGithub {
    items: Vec<Value>,
    last_update: String,
    total_count: usize,
    octocrab: octocrab::Octocrab,
}

impl BoardItemsFromGithub {
    fn new() -> Result<Self, GftoolsError> {
        Ok(Self {
            items: Vec::new(),
            last_update: String::new(),
            total_count: 0,
            octocrab: octocrab_with_auth()?,
        })
    }

    /// Fetch one page of the board.
    ///
    /// `Octocrab::graphql` unwraps the GraphQL envelope and returns the contents
    /// of the response's `data` field, so the pointers below start at
    /// `organization`, not at `data`.
    async fn fetch_more_board_items(
        &mut self,
        last_item: &str,
    ) -> Result<serde_json::Value, GftoolsError> {
        let data: serde_json::Value = self
            .octocrab
            .graphql(&json!({
                "query": include_str!("../trafficjam.graphql"),
                "variables": {
                    "after": last_item,
                }
            }))
            .await
            .map_err(|e| GftoolsError::GitHub(format!("{}", e)))?;
        self.last_update = data
            .pointer("/organization/projectV2/updatedAt")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        Ok(data)
    }

    fn fill_from_cache(&mut self, cache: Value) {
        if let Some(items) = cache.pointer("/board_items").and_then(|v| v.as_array()) {
            self.items.extend(items.iter().cloned());
        }
    }
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    // Extends the board items from the given data and returns the cursor of the last item, if any.
    fn extend_from_data(&mut self, data: Value) -> Option<String> {
        let incoming_items = data
            .pointer("/organization/projectV2/items/nodes")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        self.total_count = data
            .pointer("/organization/projectV2/items/totalCount")
            .and_then(|x| x.as_i64().map(|v| v as usize))
            .unwrap_or_default();
        let last_item = data
            .pointer("/organization/projectV2/items/edges")
            .and_then(|x| x.as_array())
            .and_then(|x| x.last())
            .and_then(|x| x.as_object())
            .and_then(|x| x.get("cursor"))
            .and_then(|x| x.as_str());
        let updated_at = data
            .pointer("/organization/projectV2/updatedAt")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        self.last_update = updated_at.to_string();
        self.items.extend(incoming_items);
        last_item.map(|s| s.to_string())
    }

    fn more_needed(&self) -> bool {
        self.items.len() < self.total_count
    }

    /// Check every board item is a pull request, sort them, then fill in the
    /// changed files for PRs which have more than the 100 the query returns.
    async fn sanitize(&mut self) -> Result<(), GftoolsError> {
        if self
            .items
            .iter()
            .any(|item| item.pointer("/type").and_then(|v| v.as_str()) != Some("PULL_REQUEST"))
        {
            return Err(GftoolsError::Misc(
                "Traffic Jam contains issues! All items must be pull requests. \
                Please remove the issues and rerun the tool, \
                https://github.com/orgs/google/projects/74/views/1"
                    .to_string(),
            ));
        }

        // Python sorts by the pull request url.
        self.items.sort_by(|a, b| {
            a.pointer("/content/url")
                .and_then(|v| v.as_str())
                .cmp(&b.pointer("/content/url").and_then(|v| v.as_str()))
        });

        // Fetching the file lists is async, so work out what needs filling in
        // before borrowing the items mutably.
        let backfills: Vec<(usize, u64, String, u64)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let changed_files = item
                    .pointer("/content/files/totalCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if changed_files <= 100 {
                    return None;
                }
                let pr_number = item.pointer("/content/number").and_then(|v| v.as_u64())?;
                let pr_url = item
                    .pointer("/content/url")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                Some((index, pr_number, pr_url, changed_files))
            })
            .collect();

        for (index, pr_number, pr_url, changed_files) in backfills {
            log::warn!(
                "{} has {} changed files. Attempting to fetch them.",
                pr_url,
                changed_files
            );
            let files = self.pr_files(pr_number).await?;
            if let Some(nodes) = self.items[index].pointer_mut("/content/files/nodes") {
                *nodes = files
                    .into_iter()
                    .map(|path| json!({ "path": path }))
                    .collect();
            }
        }
        Ok(())
    }

    fn save_cache(&self, cache_path: &Path) -> Result<(), GftoolsError> {
        std::fs::write(
            cache_path,
            serde_json::to_string_pretty(&serde_json::json!({
                "updatedAt": self.last_update,
                "board_items": self.items,
            }))
            .map_err(|e| GftoolsError::Misc(format!("Failed to serialize cache: {}", e)))?,
        )?;
        Ok(())
    }

    async fn pr_files(&self, pr_number: u64) -> Result<Vec<String>, GftoolsError> {
        let stream = self
            .octocrab
            .pulls("google", "fonts")
            .list_files(pr_number)
            .await
            .map_err(|e| GftoolsError::GitHub(format!("Failed to list PR files: {}", e)))?
            .into_stream(&self.octocrab);
        pin!(stream);
        let mut files = Vec::new();
        while let Some(diffentry) = stream
            .try_next()
            .await
            .map_err(|e| GftoolsError::GitHub(format!("Failed to list PR files: {}", e)))?
        {
            files.push(diffentry.filename);
        }
        Ok(files)
    }
}
/// Suffixes which identify a file inside a family directory. Python's
/// `FAMILY_FILE_SUFFIXES`, minus the leading dots.
const FAMILY_FILE_SUFFIXES: &[&str] = &["ttf", "otf", "html", "pb", "txt", "yaml", "png"];

/// The category a pull request falls into, taken from its labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PushCategory {
    New,
    Upgrade,
    DesignerProfile,
    AxisRegistry,
    Knowledge,
    Metadata,
    SampleTexts,
    Other,
    Blocked,
    Deleted,
}

impl PushCategory {
    /// The string used on the Traffic Jam board and in server files.
    pub fn as_str(&self) -> &'static str {
        match self {
            PushCategory::New => "New",
            PushCategory::Upgrade => "Upgrade",
            PushCategory::DesignerProfile => "Designer profile",
            PushCategory::AxisRegistry => "Axis Registry",
            PushCategory::Knowledge => "Knowledge",
            PushCategory::Metadata => "Metadata / Description / License",
            PushCategory::SampleTexts => "Sample texts",
            PushCategory::Other => "Other",
            PushCategory::Blocked => "Blocked",
            PushCategory::Deleted => "Deleted",
        }
    }

    /// Every category, in the order server files list them.
    pub fn values() -> [PushCategory; 10] {
        [
            PushCategory::New,
            PushCategory::Upgrade,
            PushCategory::DesignerProfile,
            PushCategory::AxisRegistry,
            PushCategory::Knowledge,
            PushCategory::Metadata,
            PushCategory::SampleTexts,
            PushCategory::Other,
            PushCategory::Blocked,
            PushCategory::Deleted,
        ]
    }

    /// `None` when the string is not a known category.
    pub fn from_string(string: &str) -> Option<Self> {
        Self::values().into_iter().find(|c| c.as_str() == string)
    }

    pub fn from_labels(labels: &[&str]) -> Self {
        if labels.contains(&"--- blocked") {
            PushCategory::Blocked
        } else if labels.contains(&"I Font Upgrade") || labels.contains(&"I Small Fix") {
            PushCategory::Upgrade
        } else if labels.contains(&"I New Font") {
            PushCategory::New
        } else if labels.contains(&"I Article/Description") || labels.contains(&"I Metadata/OFL") {
            PushCategory::Metadata
        } else if labels.contains(&"I Designer profile") {
            PushCategory::DesignerProfile
        } else if labels.contains(&"I Knowledge") {
            PushCategory::Knowledge
        } else if labels.contains(&"I Axis Registry") {
            PushCategory::AxisRegistry
        } else if labels.contains(&"I Lang") {
            PushCategory::SampleTexts
        } else {
            PushCategory::Other
        }
    }
}

impl Display for PushCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a pull request is in the release process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PushStatus {
    PrGf,
    InDev,
    InSandbox,
    Live,
}

impl PushStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PushStatus::PrGf => "PR GF",
            PushStatus::InDev => "In Dev / PR Merged",
            PushStatus::InSandbox => "In Sandbox",
            PushStatus::Live => "Live",
        }
    }

    pub fn from_string(string: &str) -> Option<Self> {
        [
            PushStatus::PrGf,
            PushStatus::InDev,
            PushStatus::InSandbox,
            PushStatus::Live,
        ]
        .into_iter()
        .find(|s| s.as_str() == string)
    }
}

impl Display for PushStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which push list an item is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PushList {
    ToSandbox,
    ToProduction,
    Blocked,
}

impl PushList {
    pub fn as_str(&self) -> &'static str {
        match self {
            PushList::ToSandbox => "to_sandbox",
            PushList::ToProduction => "to_production",
            PushList::Blocked => "blocked",
        }
    }

    pub fn from_string(string: &str) -> Option<Self> {
        [
            PushList::ToSandbox,
            PushList::ToProduction,
            PushList::Blocked,
        ]
        .into_iter()
        .find(|l| l.as_str() == string)
    }
}

impl Display for PushList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn has_component(path: &Path, name: &str) -> bool {
    path.components().any(|c| c.as_os_str() == name)
}

fn extension_is(path: &Path, suffix: &str) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some(suffix)
}

/// One line of a push list: a path in google/fonts plus the metadata the Traffic
/// Jam knows about it.
#[derive(Debug, Clone)]
pub struct PushItem {
    pub path: PathBuf,
    pub category: Option<PushCategory>,
    pub status: Option<PushStatus>,
    pub url: Option<String>,
    pub push_list: Option<PushList>,
    pub merged: Option<bool>,
    pub id: Option<String>,
    pub linked_issues: Vec<Value>,
}

impl PushItem {
    /// Items are compared on path and url only, which is what makes the set
    /// arithmetic in `gen_push_lists` work: the same path from two pull requests
    /// is one item.
    fn key(&self) -> (&Path, Option<&str>) {
        (self.path.as_path(), self.url.as_deref())
    }

    /// The url, treating an empty string as absent (as Python's truthiness does).
    pub fn url_string(&self) -> Option<&str> {
        self.url.as_deref().filter(|url| !url.is_empty())
    }

    /// Is the path present in a google/fonts checkout rooted at `root`?
    ///
    /// Python checks relative to the current directory; taking the root
    /// explicitly keeps this testable and avoids depending on process state.
    pub fn exists(&self, root: &Path) -> bool {
        root.join(google_path_to_repo_path(&self.path)).exists()
    }

    /// The tracked item this row refers to, if its category has one.
    ///
    /// `root` is the google/fonts checkout the item's path is relative to;
    /// Python resolves it against the current directory instead. A category
    /// which has no tracked item, such as `Knowledge`, yields `None` quietly;
    /// one which should have an item but cannot be read is logged, because
    /// silently returning `None` makes the caller report "N/A" for everything.
    ///
    /// Python memoises this with `cached_property`; here it is recomputed.
    pub fn item(&self, root: &Path) -> Option<Item> {
        let category = self.category?;
        let path = root.join(&self.path);
        let item = match category {
            PushCategory::New | PushCategory::Upgrade => {
                Family::from_path(&path).ok().map(Item::Family)
            }
            PushCategory::DesignerProfile => Designer::from_path(&path).ok().map(Item::Designer),
            PushCategory::Metadata => FamilyMeta::from_path(&path).ok().map(Item::FamilyMeta),
            PushCategory::AxisRegistry => Axis::from_path(&path).ok().map(Item::Axis),
            _ => return None,
        };
        if item.is_none() {
            log::warn!("Could not read a {} item from {}", category, path.display());
        }
        item
    }

    /// The four fields the server files and `push_stats` care about.
    pub fn to_json(&self) -> serde_json::Map<String, Value> {
        let mut json = serde_json::Map::new();
        json.insert(
            "path".to_string(),
            Value::String(self.path.to_string_lossy().to_string()),
        );
        json.insert(
            "category".to_string(),
            self.category
                .map(|c| Value::String(c.as_str().to_string()))
                .unwrap_or(Value::Null),
        );
        json.insert(
            "status".to_string(),
            self.status
                .map(|s| Value::String(s.as_str().to_string()))
                .unwrap_or(Value::Null),
        );
        json.insert(
            "url".to_string(),
            self.url_string()
                .map(|u| Value::String(u.to_string()))
                .unwrap_or(Value::Null),
        );
        json
    }

    pub async fn set_server(
        &mut self,
        server: PushStatus,
        config: &PushConfig,
    ) -> Result<(), GftoolsError> {
        let client = OctocrabHelper::new(config)?;
        match &self.id {
            Some(id) => client.update_traffic_jam_status(id, &server).await?,
            // Python interpolates the missing id into the mutation, which sends
            // the string "None"; say so instead.
            None => log::warn!(
                "{} is not on the board, so its status was not set",
                self.path.display()
            ),
        }
        self.status = Some(server);
        // Update the projects board as well
        for linked_issue in self.linked_issues.iter() {
            let Some(project_items) = linked_issue
                .pointer("/projectItems/nodes")
                .and_then(Value::as_array)
            else {
                continue;
            };
            for project_item in project_items {
                if let Some(id) = project_item["id"].as_str() {
                    client.update_gf_project(id, &server).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn set_pushlist(
        &mut self,
        pushlist: PushList,
        config: &PushConfig,
    ) -> Result<(), GftoolsError> {
        let client = OctocrabHelper::new(config)?;
        match &self.id {
            Some(id) => client.update_traffic_jam_list(id, &pushlist).await?,
            None => log::warn!(
                "{} is not on the board, so its list was not set",
                self.path.display()
            ),
        }
        self.push_list = Some(pushlist);
        Ok(())
    }

    pub async fn block(&mut self, config: &PushConfig) -> Result<(), GftoolsError> {
        log::info!("Blocking {}", self.path.display());
        self.set_pushlist(PushList::Blocked, config).await
    }

    pub async fn bump_pushlist(&mut self, config: &PushConfig) -> Result<(), GftoolsError> {
        match &self.push_list {
            None => self.set_pushlist(PushList::ToSandbox, config).await,
            Some(PushList::ToSandbox) => self.set_pushlist(PushList::ToProduction, config).await,
            Some(PushList::ToProduction) => {
                log::warn!(
                    "No push list beyond to_production, keeping {} in to_production",
                    self.path.display()
                );
                Ok(())
            }
            // Only `Blocked` can reach this, which Python raises for as well.
            Some(list) => Err(GftoolsError::Misc(format!("{list} is not supported"))),
        }
    }
}

impl PartialEq for PushItem {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for PushItem {}

impl std::hash::Hash for PushItem {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.url.hash(state);
    }
}

/// A collection of push items, with the path-normalisation rules that turn a
/// list of changed files into a list of things to push.
///
/// Construct with [`PushItems::default`] and [`PushItems::add`] to get the
/// normalisation, or with `PushItems(vec![...])` to bypass it (as Python does
/// when it builds a bare list).
#[derive(Debug, Clone, Default)]
pub struct PushItems(pub Vec<PushItem>);

impl PushItems {
    /// Normalise `item`'s path and add it, or drop it if it is not something we
    /// track. Mirrors `PushItems.add`.
    pub fn add(&mut self, mut item: PushItem) {
        let original = item.path.clone();

        if item.category == Some(PushCategory::DesignerProfile)
            && ["ofl", "apache", "ufl"]
                .iter()
                .any(|dir| has_component(&original, dir))
        {
            return;
        }

        if has_component(&original, "article") || has_component(&original, "static") {
            // A path under `article/`/`static/` belongs to the family above it.
            // Note this consults the filesystem, as Python does.
            let parent = if item.path.is_dir() {
                item.path.parent()
            } else {
                item.path.parent().and_then(|p| p.parent())
            };
            match parent {
                Some(parent) => item.path = parent.to_path_buf(),
                None => return,
            }
        } else if ["ofl", "ufl", "apache", "designers"]
            .iter()
            .any(|dir| has_component(&original, dir))
            && FAMILY_FILE_SUFFIXES
                .iter()
                .any(|suffix| extension_is(&item.path, suffix))
        {
            // A file inside a family directory belongs to that directory.
            match item.path.parent() {
                Some(parent) => item.path = parent.to_path_buf(),
                None => return,
            }
        } else if ["lang", "axisregistry"]
            .iter()
            .any(|dir| has_component(&original, dir))
            && extension_is(&item.path, "textproto")
        {
            item.path = repo_path_to_google_path(&item.path);
        } else if ["lang", "axisregistry"]
            .iter()
            .any(|dir| has_component(&original, dir))
        {
            // Only the `.textproto` files in these directories are tracked.
            return;
        }

        // Skip the directories themselves, e.g. `ofl/`.
        if item.path.components().count() <= 1 {
            return;
        }
        self.0.push(item);
    }

    /// `self + other`: a copy of `self` with every item of `other` added.
    pub fn added(&self, other: &PushItems) -> PushItems {
        let mut new = self.clone();
        for item in &other.0 {
            new.add(item.clone());
        }
        new
    }

    /// `self - other`: the items of `self` which are not in `other`.
    pub fn subtracted(&self, other: &PushItems) -> PushItems {
        let mut new = PushItems::default();
        for item in self.0.iter().filter(|item| !other.0.contains(item)) {
            new.add(item.clone());
        }
        new
    }

    fn filter(&self, predicate: impl Fn(&PushItem) -> bool) -> PushItems {
        PushItems(self.0.iter().filter(|i| predicate(i)).cloned().collect())
    }

    pub fn to_sandbox(&self) -> PushItems {
        self.filter(|i| i.push_list == Some(PushList::ToSandbox))
    }

    pub fn in_sandbox(&self) -> PushItems {
        self.filter(|i| i.status == Some(PushStatus::InSandbox))
    }

    pub fn in_dev(&self) -> PushItems {
        self.filter(|i| i.status == Some(PushStatus::InDev))
    }

    pub fn to_production(&self) -> PushItems {
        self.filter(|i| i.push_list == Some(PushList::ToProduction))
    }

    pub fn live(&self) -> PushItems {
        self.filter(|i| i.status == Some(PushStatus::Live))
    }

    /// The paths of items which are not present in the checkout at `root`,
    /// excluding deleted ones.
    pub fn missing_paths(&self, root: &Path) -> Vec<PathBuf> {
        let mut res = Vec::new();
        for item in &self.0 {
            if item.category == Some(PushCategory::Deleted) {
                continue;
            }
            let path =
                if has_component(&item.path, "lang") || has_component(&item.path, "axisregistry") {
                    google_path_to_repo_path(&item.path)
                } else {
                    item.path.clone()
                };
            if !root.join(&path).exists() {
                res.push(path);
            }
        }
        res
    }

    /// Render the contents of a `to_sandbox.txt`/`to_production.txt` file.
    ///
    /// Existence is checked against `root` rather than the current directory.
    pub fn to_server_file(&self, root: &Path) -> String {
        let mut seen: HashSet<&Path> = HashSet::new();
        let mut bins: Vec<(PushCategory, Vec<&PushItem>)> = Vec::new();
        for item in &self.0 {
            if item.category == Some(PushCategory::Blocked) || !seen.insert(item.path.as_path()) {
                continue;
            }
            // `tags/all/families.csv` has its own bin in Python that the output
            // loop never reaches; `gen_push_lists` appends it separately.
            if item.path == Path::new("tags/all/families.csv") {
                continue;
            }
            let Some(category) = item.category else {
                continue;
            };
            match bins.iter_mut().find(|(c, _)| *c == category) {
                Some((_, items)) => items.push(item),
                None => bins.push((category, vec![item])),
            }
        }

        let mut res: Vec<String> = Vec::new();
        for category in PushCategory::values() {
            let Some((_, items)) = bins.iter().find(|(c, _)| *c == category) else {
                continue;
            };
            res.push(format!("# {category}"));
            let mut items = items.clone();
            items.sort_by(|a, b| a.path.cmp(&b.path));
            for item in items {
                let path = item.path.to_string_lossy();
                if item.exists(root) {
                    res.push(format!("{path} # {}", item.url_string().unwrap_or("")));
                } else if let Some(url) = item.url_string() {
                    res.push(format!("# Deleted: {path} # {url}"));
                } else {
                    res.push(format!("# Deleted: {path}"));
                }
            }
            res.push(String::new());
        }
        res.join("\n")
    }

    /// Parse a `to_sandbox.txt`/`to_production.txt` file.
    pub fn from_server_file(
        content: &str,
        status: Option<PushStatus>,
        push_list: Option<PushList>,
    ) -> PushItems {
        let mut results = PushItems::default();
        let mut category = Some(PushCategory::Other);
        for line in content.split('\n') {
            if line.is_empty() {
                continue;
            }
            let mut line = line;
            let mut deleted = false;
            if let Some(rest) = line.strip_prefix("# Deleted") {
                line = rest.strip_prefix(':').unwrap_or(rest).trim_start();
                deleted = true;
            }
            if let Some(header) = line.strip_prefix('#') {
                category = PushCategory::from_string(header.trim());
            } else {
                let (path, url) = match line.split_once('#') {
                    Some((path, url)) => (path, Some(url.trim().to_string())),
                    None => (line, None),
                };
                results.add(PushItem {
                    path: PathBuf::from(path.trim()),
                    category: if deleted {
                        Some(PushCategory::Deleted)
                    } else {
                        category
                    },
                    status,
                    url: url.filter(|url| !url.is_empty()),
                    push_list,
                    merged: None,
                    id: None,
                    linked_issues: Vec::new(),
                });
            }
        }
        results
    }

    /// Fetch the push items from the Traffic Jam board, optionally using and
    /// refreshing an on-disk cache of the board.
    pub async fn from_traffic_jam(cache: Option<&Path>) -> Result<PushItems, GftoolsError> {
        log::info!("Getting push items from traffic jam board");
        let mut board = BoardItemsFromGithub::new()?;

        // Poll first, to learn when the board was last updated.
        let data = board.fetch_more_board_items("").await?;

        // Use the cache when the board has not changed since it was written.
        let mut used_cache = false;
        if let Some(cache_path) = cache.filter(|path| path.exists()) {
            let existing: Value = serde_json::from_reader(std::fs::File::open(cache_path)?)
                .map_err(|e| {
                    GftoolsError::Misc(format!("Couldn't parse cached traffic jam data: {}", e))
                })?;
            let last_update = existing.get("updatedAt").and_then(|v| v.as_str());
            let current_update = data
                .pointer("/organization/projectV2/updatedAt")
                .and_then(|v| v.as_str());
            if last_update == current_update {
                board.fill_from_cache(existing);
                used_cache = !board.is_empty();
            }
        }

        if !used_cache {
            let mut last_item = board.extend_from_data(data);
            while board.more_needed() {
                let Some(cursor) = last_item else {
                    break;
                };
                match board.fetch_more_board_items(&cursor).await {
                    Ok(data) => last_item = board.extend_from_data(data),
                    Err(e) => {
                        // Python retries until it succeeds. Truncating the board
                        // silently would be dangerous, so at least say so.
                        log::error!("Failed to fetch more board items: {}", e);
                        break;
                    }
                }
            }
            board.sanitize().await?;
            if let Some(cache_path) = cache {
                board.save_cache(cache_path)?;
            }
        }

        Ok(board.into())
    }
}

impl From<BoardItemsFromGithub> for PushItems {
    /// Mirrors the tail of Python's `from_traffic_jam`: one [`PushItem`] per
    /// changed file, normalised through [`PushItems::add`].
    fn from(board: BoardItemsFromGithub) -> Self {
        let mut results = PushItems::default();
        for item in board.items.iter() {
            // Don't let closed PRs affect the status.
            let closed = item
                .pointer("/content/closed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let merged = item
                .pointer("/content/merged")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if closed && !merged {
                continue;
            }

            // Both of these are optional on the board; an unknown name is also
            // treated as absent, as Python's truthiness check does.
            let status = item
                .pointer("/status/name")
                .and_then(|v| v.as_str())
                .and_then(PushStatus::from_string);
            let push_list = item
                .pointer("/list/name")
                .and_then(|v| v.as_str())
                .and_then(PushList::from_string);

            // Python skips a PR whose `content` has no `labels` at all.
            let Some(labels) = item
                .pointer("/content/labels/nodes")
                .and_then(|v| v.as_array())
            else {
                log::warn!("PR missing labels. Skipping");
                continue;
            };
            let labels: Vec<&str> = labels
                .iter()
                .filter_map(|label| label.pointer("/name").and_then(|v| v.as_str()))
                .collect();
            let category = PushCategory::from_labels(&labels);

            let linked_issues = item
                .pointer("/content/closingIssuesReferences/nodes")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let url = item
                .pointer("/content/url")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let id = item
                .pointer("/id")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let files = item
                .pointer("/content/files/nodes")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            for file in files {
                let Some(path) = file.pointer("/path").and_then(|v| v.as_str()) else {
                    continue;
                };
                // A Python script in a PR is not something to push.
                if Path::new(path).extension().and_then(|e| e.to_str()) == Some("py") {
                    continue;
                }
                results.add(PushItem {
                    path: PathBuf::from(path),
                    category: Some(category),
                    status,
                    url: url.clone(),
                    push_list,
                    merged: Some(merged),
                    id: id.clone(),
                    linked_issues: linked_issues.clone(),
                });
            }
        }
        results
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, category: PushCategory, url: &str) -> PushItem {
        PushItem {
            path: PathBuf::from(path),
            category: Some(category),
            status: Some(PushStatus::InDev),
            url: Some(url.to_string()),
            push_list: None,
            merged: None,
            id: None,
            linked_issues: Vec::new(),
        }
    }

    /// The path a single item normalises to, or `None` if it is dropped.
    fn add_one(path: &str, category: PushCategory) -> Option<PathBuf> {
        let mut items = PushItems::default();
        items.add(item(path, category, "1"));
        items.0.first().map(|i| i.path.clone())
    }

    #[test]
    fn test_push_item_eq() {
        let ofl = |path: &str, url: &str, status| PushItem {
            path: PathBuf::from(path),
            category: Some(PushCategory::Upgrade),
            status: Some(status),
            url: Some(url.to_string()),
            push_list: None,
            merged: None,
            id: None,
            linked_issues: Vec::new(),
        };
        // Same path and url.
        assert_eq!(
            ofl("ofl/mavenpro", "45", PushStatus::InDev),
            ofl("ofl/mavenpro", "45", PushStatus::InDev)
        );
        // Different url.
        assert_ne!(
            ofl("ofl/mavenpro", "45", PushStatus::InDev),
            ofl("ofl/mavenpro", "46", PushStatus::InDev)
        );
        // Different path.
        assert_ne!(
            ofl("ofl/mavenpro", "45", PushStatus::InDev),
            ofl("ofl/mavenpro2", "45", PushStatus::InDev)
        );
        // Status is not part of the comparison.
        assert_eq!(
            ofl("ofl/mavenpro", "45", PushStatus::InDev),
            ofl("ofl/mavenpro", "45", PushStatus::InSandbox)
        );
    }

    #[test]
    fn test_push_item_set() {
        let items = PushItems(vec![
            item("1", PushCategory::Other, "1"),
            item("1", PushCategory::Other, "1"),
            item("1", PushCategory::Other, "1"),
        ]);
        assert_eq!(items.0.iter().collect::<HashSet<_>>().len(), 1);

        let items = PushItems(vec![
            item("1", PushCategory::Other, "1"),
            item("1", PushCategory::Other, "1"),
            item("2", PushCategory::Other, "1"),
        ]);
        assert_eq!(items.0.iter().collect::<HashSet<_>>().len(), 2);
    }

    #[test]
    fn test_push_items_operators() {
        let left = PushItems(vec![item("ofl/mavenpro", PushCategory::New, "1")]);
        let right = PushItems(vec![
            {
                let mut i = item("ofl/mavenpro", PushCategory::New, "1");
                i.status = Some(PushStatus::InSandbox);
                i
            },
            item("ofl/amatic", PushCategory::New, "1"),
        ]);

        let sum = left.added(&right);
        assert_eq!(sum.0.len(), 3);
        assert_eq!(sum.0[0].path, PathBuf::from("ofl/mavenpro"));
        assert_eq!(sum.0[1].path, PathBuf::from("ofl/mavenpro"));
        assert_eq!(sum.0[2].path, PathBuf::from("ofl/amatic"));

        let difference = right.subtracted(&PushItems(vec![{
            let mut i = item("ofl/mavenpro", PushCategory::New, "1");
            i.status = Some(PushStatus::InSandbox);
            i
        }]));
        assert_eq!(difference.0.len(), 1);
        assert_eq!(difference.0[0].path, PathBuf::from("ofl/amatic"));
    }

    #[test]
    fn test_push_items_add() {
        // Font file names collapse to the family directory.
        assert_eq!(
            add_one("ofl/mavenpro/MavenPro[wght].ttf", PushCategory::Upgrade),
            Some(PathBuf::from("ofl/mavenpro"))
        );
        // ... and so do several of them.
        let mut items = PushItems::default();
        items.add(item(
            "ofl/mavenpro/MavenPro[wght].ttf",
            PushCategory::Upgrade,
            "1",
        ));
        items.add(item(
            "ofl/mavenpro/MavenPro-Italic[wght].ttf",
            PushCategory::Upgrade,
            "1",
        ));
        assert_eq!(items.0.len(), 2); // Python does not dedupe here either
        // axisregistry paths are rewritten.
        assert_eq!(
            add_one(
                "axisregistry/Lib/axisregistry/data/bounce.textproto",
                PushCategory::New
            ),
            Some(PathBuf::from("axisregistry/bounce.textproto"))
        );
        // ... as are lang paths.
        assert_eq!(
            add_one(
                "lang/Lib/gflanguages/data/languages/aa_Latn.textproto",
                PushCategory::New
            ),
            Some(PathBuf::from("lang/languages/aa_Latn.textproto"))
        );
        // Parent directories are skipped.
        assert_eq!(add_one("ofl", PushCategory::New), None);
        assert_eq!(add_one("apache", PushCategory::New), None);
        // Non-textproto files in lang/ are skipped.
        assert_eq!(add_one("lang/authors.txt", PushCategory::New), None);
        // A file under article/ collapses to the family.
        assert_eq!(
            add_one("ofl/notosans/article/index.html", PushCategory::New),
            Some(PathBuf::from("ofl/notosans"))
        );
        // Designer profiles inside a family directory are dropped outright.
        assert_eq!(
            add_one("ofl/notosans/bio.html", PushCategory::DesignerProfile),
            None
        );
        assert_eq!(
            add_one("ofl/colophonfoundry/info.pb", PushCategory::DesignerProfile),
            None
        );
        // A designer profile in the catalog keeps its directory.
        assert_eq!(
            add_one(
                "catalog/designers/colophonfoundry/info.pb",
                PushCategory::DesignerProfile
            ),
            Some(PathBuf::from("catalog/designers/colophonfoundry"))
        );
        // METADATA.pb collapses to the family directory.
        assert_eq!(
            add_one("ofl/notosanspsalterpahlavi/METADATA.pb", PushCategory::New),
            Some(PathBuf::from("ofl/notosanspsalterpahlavi"))
        );
    }

    #[test]
    fn test_push_items_from_server_file() {
        let items = PushItems::from_server_file(
            "ofl/noto # 2",
            Some(PushStatus::InDev),
            Some(PushList::ToSandbox),
        );
        assert_eq!(items.0.len(), 1);

        let items = PushItems::from_server_file(
            "# New\nofl/noto # 2\nofl/foobar # 3\n\n# Upgrade\nofl/mavenPro # 4",
            Some(PushStatus::InDev),
            Some(PushList::ToSandbox),
        );
        assert_eq!(items.0.len(), 3);
        assert_eq!(items.0[0].category, Some(PushCategory::New));
        assert_eq!(items.0[2].category, Some(PushCategory::Upgrade));
        assert_eq!(items.0[2].path, PathBuf::from("ofl/mavenPro"));
        assert_eq!(items.0[2].url.as_deref(), Some("4"));

        let items = PushItems::from_server_file(
            "# New\nofl/noto\n# Deleted: lang/languages/wsg_Gong.textproto # 5",
            Some(PushStatus::InDev),
            Some(PushList::ToSandbox),
        );
        assert_eq!(items.0.len(), 2);
        assert_eq!(items.0[1].category, Some(PushCategory::Deleted));
        assert_eq!(items.0[1].url.as_deref(), Some("5"));
    }

    #[test]
    fn test_push_items_to_server_file() {
        // Nothing exists relative to the root, so everything is "Deleted".
        let root = Path::new("/nonexistent-root");
        let items = PushItems(vec![
            item("ofl/mavenpro", PushCategory::Upgrade, "45"),
            item("ofl/amatic", PushCategory::New, "46"),
        ]);
        assert_eq!(
            items.to_server_file(root),
            "# New\n# Deleted: ofl/amatic # 46\n\n# Upgrade\n# Deleted: ofl/mavenpro # 45\n"
        );

        // The category ordering comes from `PushCategory::values()`.
        let items = PushItems(vec![
            item("ofl/opensans", PushCategory::Upgrade, "47"),
            item("ofl/amatic", PushCategory::New, "46"),
            item("ofl/mavenpro", PushCategory::Upgrade, "45"),
        ]);
        assert_eq!(
            items.to_server_file(root),
            "# New\n# Deleted: ofl/amatic # 46\n\n# Upgrade\n# Deleted: ofl/mavenpro # 45\n# Deleted: ofl/opensans # 47\n"
        );

        // Duplicate items appear once.
        let items = PushItems(vec![
            item("ofl/mavenpro", PushCategory::Upgrade, "45"),
            item("ofl/mavenpro", PushCategory::Upgrade, "45"),
            item("ofl/mavenpro", PushCategory::Upgrade, "45"),
        ]);
        assert_eq!(
            items.to_server_file(root),
            "# Upgrade\n# Deleted: ofl/mavenpro # 45\n"
        );

        // Blocked items are dropped.
        let items = PushItems(vec![item("ofl/mavenpro", PushCategory::Blocked, "45")]);
        assert_eq!(items.to_server_file(root), "");
    }

    #[test]
    fn test_to_server_file_marks_existing_paths() {
        let root = std::env::temp_dir().join("gftools-push-to-server-file-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("ofl/mavenpro")).unwrap();
        std::fs::write(root.join("ofl/mavenpro/MavenPro[wght].ttf"), b"").unwrap();

        // A raw `PushItems` is not normalised, so the font file path is kept.
        let items = PushItems(vec![
            item(
                "ofl/mavenpro/MavenPro[wght].ttf",
                PushCategory::Upgrade,
                "45",
            ),
            item("ofl/amatic", PushCategory::New, "46"),
        ]);
        assert_eq!(
            items.to_server_file(&root),
            "# New\n# Deleted: ofl/amatic # 46\n\n# Upgrade\nofl/mavenpro/MavenPro[wght].ttf # 45\n"
        );

        // `add()` is what collapses the font file to the family directory,
        // which also exists, so this one is not marked deleted.
        let mut normalised = PushItems::default();
        normalised.add(item(
            "ofl/mavenpro/MavenPro[wght].ttf",
            PushCategory::Upgrade,
            "45",
        ));
        assert_eq!(
            normalised.to_server_file(&root),
            "# Upgrade\nofl/mavenpro # 45\n"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn test_missing_paths() {
        let root = Path::new("/nonexistent-root");
        assert_eq!(
            PushItems(vec![item("ofl/mavenpro", PushCategory::Upgrade, "45")]).missing_paths(root),
            vec![PathBuf::from("ofl/mavenpro")]
        );
        // Deleted items are ignored.
        assert!(
            PushItems(vec![item("ofl/mavenpro", PushCategory::Deleted, "45")])
                .missing_paths(root)
                .is_empty()
        );
        // lang/ and axisregistry/ paths are transformed before checking.
        assert_eq!(
            PushItems(vec![item(
                "lang/languages/aa_Latn.textproto",
                PushCategory::SampleTexts,
                "45"
            )])
            .missing_paths(root),
            vec![PathBuf::from(
                "lang/Lib/gflanguages/data/languages/aa_Latn.textproto"
            )]
        );
    }

    #[test]
    fn test_to_json() {
        let json = item("ofl/mavenpro", PushCategory::Upgrade, "45").to_json();
        assert_eq!(json["path"], Value::String("ofl/mavenpro".to_string()));
        assert_eq!(json["category"], Value::String("Upgrade".to_string()));
        assert_eq!(
            json["status"],
            Value::String("In Dev / PR Merged".to_string())
        );
        assert_eq!(json["url"], Value::String("45".to_string()));

        // Absent fields are null, and an empty url counts as absent.
        let empty = PushItem {
            path: PathBuf::from("ofl/mavenpro"),
            category: None,
            status: None,
            url: Some(String::new()),
            push_list: None,
            merged: None,
            id: None,
            linked_issues: Vec::new(),
        };
        let json = empty.to_json();
        assert_eq!(json["category"], Value::Null);
        assert_eq!(json["status"], Value::Null);
        assert_eq!(json["url"], Value::Null);
    }
}
