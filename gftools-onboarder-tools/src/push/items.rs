use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use scraper::Html;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use skrifa::{FontRef, MetadataProvider, string::StringId};

use gf_metadata::{AxisProto, DesignerInfoProto, FamilyProto};
use gftools::{GftoolsError, download_family_from_google_fonts, font_version, parse_pb};

use crate::push::utils::google_path_to_repo_path;

/// Read an HTML file and reduce it to plain text. `None` when there is nothing
/// to read.
fn parse_html_file(p: &Path) -> Result<Option<String>, GftoolsError> {
    let contents = std::fs::read_to_string(p)
        .map_err(|_| GftoolsError::Misc(format!("Failed to read HTML file: {:?}", p)))?;
    Ok(parse_html(&contents))
}

/// Strip tags and collapse whitespace, mirroring `gftools.push.items.parse_html`.
/// Empty or blank input yields `None`, so "there is no description/article" is
/// represented by absence rather than an empty string.
fn parse_html(s: &str) -> Option<String> {
    if s.trim().is_empty() {
        return None;
    }
    let document = Html::parse_fragment(s);
    let text = document
        .tree
        .nodes()
        .filter_map(|node| match node.value() {
            scraper::Node::Text(text) => Some(text.trim()),
            _ => None,
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .replace("\n", " ");
    (!text.is_empty()).then_some(text)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Family {
    pub name: String,
    pub version: String,
}

impl Family {
    pub(crate) fn from_fontref(f: FontRef) -> Self {
        let family: String = f
            .localized_strings(StringId::FAMILY_NAME)
            .english_or_first()
            .map(|name| name.chars().collect())
            .unwrap_or("Unknown family".to_string());
        let version: String = font_version(&f);
        Family {
            name: family,
            version,
        }
    }
    /// Create a new family from a directory containing font files.
    pub(crate) fn from_path(f: &PathBuf) -> Result<Self, GftoolsError> {
        // Read the first font file in the directory
        let font_file = std::fs::read_dir(f)?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.is_file() && path.extension().map(|ext| ext == "ttf").unwrap_or(false) {
                    Some(path)
                } else {
                    None
                }
            })
            .next()
            .ok_or_else(|| {
                GftoolsError::Misc(format!("No font file found in directory: {:?}", f))
            })?;
        let contents = std::fs::read(font_file)?;
        let font = skrifa::FontRef::new(&contents)?;
        Ok(Self::from_fontref(font))
    }
    #[allow(dead_code)]
    pub(crate) async fn from_googlefonts_json(
        data: Value,
        url: &str,
    ) -> Result<Self, GftoolsError> {
        let name = data
            .as_object()
            .and_then(|m| m.get("family"))
            .and_then(|f| f.as_str())
            .ok_or_else(|| GftoolsError::Misc(format!("Couldn't find family in JSON: {}", data)))?;
        Self::from_googlefonts(name, url).await
    }
    /// Download a family from a Google Fonts server and read the name and
    /// version from its first font, mirroring `Family.from_gf`.
    pub(crate) async fn from_googlefonts(name: &str, dl_url: &str) -> Result<Self, GftoolsError> {
        let fonts = download_family_from_google_fonts(name, Some(dl_url), true).await?;
        let bytes = fonts.values().next().ok_or_else(|| {
            GftoolsError::Misc(format!("No font files found for family '{}'", name))
        })?;
        let font = skrifa::FontRef::new(bytes)?;
        Ok(Self::from_fontref(font))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxisFallback {
    pub name: String,
    pub value: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Axis {
    pub tag: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "min")]
    pub min_value: f32,
    #[serde(rename = "defaultValue")]
    pub default_value: f32,
    #[serde(rename = "max")]
    pub max_value: f32,
    pub precision: f32,
    #[serde(default)]
    #[serde(rename = "fallbacks")]
    pub fallback: Vec<AxisFallback>,
    #[serde(default)]
    pub fallback_only: bool,
    pub description: String,
}

impl Axis {
    /// Creates an `Axis` instance from the given file path.
    ///
    /// The path should be the actual protobuf file.
    pub(crate) fn from_path(path: &Path) -> Result<Self, GftoolsError> {
        let path = google_path_to_repo_path(path);
        let proto: AxisProto = parse_pb::<AxisProto>(&path)?;
        Ok(Axis {
            tag: proto.tag().to_string(),
            display_name: proto.display_name().to_string(),
            min_value: proto.min_value(),
            default_value: proto.default_value(),
            max_value: proto.max_value(),
            // "Why the f32/i32 difference?" The protobuf definition file says:
            // Input values for this axis must aligned to 10^precision
            //   optional int32 precision = 5;
            precision: proto.precision() as f32,
            fallback: proto
                .fallback
                .iter()
                .map(|f| AxisFallback {
                    name: f.name().to_string(),
                    value: f.value(),
                })
                .collect(),
            fallback_only: proto.fallback_only(),
            description: proto.description().to_string(),
        })
    }

    // No from_googlefonts_json, just deserialize it.
}
/// The canonical item shape is the *server's* shape, so these field names and
/// the `coverage` map match what `fonts.google.com/metadata/fonts/<family>`
/// returns. Values are normalised on the way in (see the deserialisers below)
/// so that an item read from a family directory compares equal to the same
/// family read from a server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FamilyMeta {
    #[serde(rename = "family")]
    pub name: String,

    pub designers: Vec<Designer>,
    #[serde(deserialize_with = "deserialize_lowercase")]
    pub license: String,
    #[serde(deserialize_with = "deserialize_upper_snake")]
    pub category: String,
    /// The server sends a map of subset name to unicode ranges. We only ever
    /// compare the subset names, so the ranges are discarded and replaced with
    /// empty strings; that keeps the server's shape while letting `from_path`
    /// (which has no ranges to offer) produce an equal value.
    #[serde(deserialize_with = "coverage_keys")]
    pub coverage: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_stroke")]
    pub stroke: String,
    #[serde(default, deserialize_with = "deserialize_lowercase_vec")]
    pub classifications: Vec<String>,
    /// Kept separate from `article` so the existence and content of each can be
    /// compared independently.
    #[serde(default, deserialize_with = "deserialize_description")]
    pub description: Option<String>,
    #[serde(
        default,
        rename = "primaryScript",
        deserialize_with = "deserialize_opt_empty"
    )]
    pub primary_script: Option<String>,
    /// The server sends an *array* of HTML documents; the repo has a single
    /// `article/ARTICLE.en_us.html`. Both become parsed text, or `None`.
    #[serde(default, deserialize_with = "deserialize_article")]
    pub article: Option<String>,
    #[serde(
        default,
        rename = "minisiteUrl",
        deserialize_with = "deserialize_opt_empty"
    )]
    pub minisite_url: Option<String>,
}

/// Designer bios (and image urls) cannot be reproduced from a family directory,
/// and Python deliberately compared designer *names* only. Every other field is
/// normalised on the way in, so it can be compared as-is.
impl PartialEq for FamilyMeta {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.license == other.license
            && self.category == other.category
            && self.coverage == other.coverage
            && self.stroke == other.stroke
            && self.classifications == other.classifications
            && self.description == other.description
            && self.primary_script == other.primary_script
            && self.article == other.article
            && self.minisite_url == other.minisite_url
            && self.designers.len() == other.designers.len()
            && self
                .designers
                .iter()
                .zip(other.designers.iter())
                .all(|(a, b)| a.name == b.name)
    }
}

// Value normalisation lives in these deserialisers rather than in
// `from_googlefonts_json`, because `GFServer::update_metadata` deserialises the
// raw server payload directly. Each is idempotent, so a value read back from
// the saved cache normalises to itself.

/// `{"latin": "0,13,32-126", ...}` -> `{"latin": "", ...}`
fn coverage_keys<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    let map: BTreeMap<String, Value> = de::Deserialize::deserialize(deserializer)?;
    Ok(map.into_keys().map(|k| (k, String::new())).collect())
}

/// Proto values are already `SANS_SERIF`; the server sends `Sans Serif`.
fn deserialize_upper_snake<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(String::deserialize(deserializer)?
        .replace(' ', "_")
        .to_uppercase())
}

fn deserialize_lowercase<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(String::deserialize(deserializer)?.to_lowercase())
}

fn deserialize_lowercase_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Vec::<String>::deserialize(deserializer)?
        .into_iter()
        .map(|s| s.to_lowercase())
        .collect())
}

/// `stroke` is optional; the repo falls back to the category when it is absent.
fn deserialize_stroke<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let stroke = Option::<String>::deserialize(deserializer)?;
    Ok(stroke.unwrap_or_default().replace(' ', "_").to_uppercase())
}

/// The server sends `""` rather than `null` for absent optional strings.
fn deserialize_opt_empty<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.filter(|s| !s.is_empty()))
}

fn deserialize_description<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let description = Option::<String>::deserialize(deserializer)?;
    Ok(description.as_deref().and_then(parse_html))
}

/// The server sends `article` as an array of HTML documents, or `null`.
fn deserialize_article<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let text = match Value::deserialize(deserializer)? {
        Value::Null => return Ok(None),
        Value::String(s) => s,
        Value::Array(items) => items
            .into_iter()
            .find_map(|item| item.as_str().map(str::to_string))
            .unwrap_or_default(),
        _ => return Ok(None),
    };
    Ok(parse_html(&text))
}

impl FamilyMeta {
    pub(crate) fn from_path(path: &Path) -> Result<Self, GftoolsError> {
        let data = parse_pb::<FamilyProto>(&path.join("METADATA.pb"))?;
        let stroke = data
            .stroke
            .as_ref()
            .map(|x| x.replace(' ', "_").to_uppercase())
            .or_else(|| data.category.first().cloned())
            .unwrap_or_else(|| "UNKNOWN".to_string());

        let article = path.join("article").join("ARTICLE.en_us.html");
        let article = if article.exists() {
            parse_html_file(&article)?
        } else {
            None
        };
        let description = path.join("DESCRIPTION.en_us.html");
        let description = if description.exists() {
            parse_html_file(&description)?
        } else {
            None
        };
        Ok(Self {
            name: data.name().to_string(),
            designers: data
                .designer
                .as_ref()
                .map(|x| {
                    x.split(", ")
                        .map(|s| Designer {
                            name: s.trim().to_string(),
                            bio: String::new(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            license: data.license().to_lowercase(),
            category: data.category.first().cloned().unwrap_or_default(),
            // `menu` is not a real subset; the server's coverage map excludes
            // it too.
            coverage: data
                .subsets
                .iter()
                .filter(|s| *s != "menu")
                .map(|s| (s.clone(), String::new()))
                .collect(),
            stroke,
            classifications: data
                .classifications
                .iter()
                .map(|c| c.to_lowercase())
                .collect(),
            description,
            primary_script: data.primary_script.clone().filter(|s| !s.is_empty()),
            article,
            minisite_url: data.minisite_url.clone().filter(|s| !s.is_empty()),
        })
    }

    #[allow(dead_code)]
    pub(crate) fn from_googlefonts_json(s: &str) -> Result<Self, GftoolsError> {
        serde_json::from_str(s)
            .map_err(|e| GftoolsError::Misc(format!("Failed to parse JSON: {}", e)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Designer {
    pub name: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub bio: String,
}

impl Designer {
    pub(crate) fn from_path(path: &Path) -> Result<Self, GftoolsError> {
        let data = parse_pb::<DesignerInfoProto>(&path.join("info.pb"))?;
        let bio = path.join("bio.html");
        let bio = if bio.exists() {
            parse_html_file(&bio)?
        } else {
            None
        };
        Ok(Self {
            name: data.designer().to_string(),
            bio: bio.unwrap_or_default(),
        })
    }
    #[allow(dead_code)]
    pub(crate) fn from_googlefonts_json(data: Value, _url: &str) -> Result<Self, GftoolsError> {
        Ok(Self {
            name: data["name"]
                .as_str()
                .ok_or(GftoolsError::Misc(format!(
                    "Couldn't find designer in JSON: {}",
                    data
                )))?
                .trim()
                .to_string(),
            bio: data["bio"]
                .as_str()
                .and_then(parse_html)
                .unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub enum Item {
    Family(Family),
    AxisFallback(AxisFallback),
    Axis(Axis),
    FamilyMeta(FamilyMeta),
    Designer(Designer),
}

impl Item {
    /// The item as JSON, in the server's shape. `Family` is the exception: it is
    /// a composite of the versions manifest and the font itself, so it keeps its
    /// own `{name, version}` shape.
    pub fn to_json(&self) -> Value {
        match self {
            Item::Family(v) => serde_json::to_value(v),
            Item::AxisFallback(v) => serde_json::to_value(v),
            Item::Axis(v) => serde_json::to_value(v),
            Item::FamilyMeta(v) => serde_json::to_value(v),
            Item::Designer(v) => serde_json::to_value(v),
        }
        .unwrap_or(Value::Null)
    }

    /// The name this item is known by, which is what Python's `.name` gives —
    /// except for an axis, which Python would fail on: an axis has a `tag`.
    pub fn name(&self) -> &str {
        match self {
            Item::Family(v) => &v.name,
            Item::AxisFallback(v) => &v.name,
            Item::Axis(v) => &v.tag,
            Item::FamilyMeta(v) => &v.name,
            Item::Designer(v) => &v.name,
        }
    }
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use crate::push::servers::PROD_FAMILY_DOWNLOAD;

    use super::*;
    use pretty_assertions::assert_eq;

    const CRATE_ROOT: &str = env!("CARGO_MANIFEST_DIR");

    const FAMILY_JSON: &str = include_str!("../../data/test/servers/family.json");
    const FONTS_JSON: &str = include_str!("../../data/test/servers/fonts.json");

    #[test]
    fn test_family_meta() {
        let family_path: PathBuf = PathBuf::from(CRATE_ROOT)
            .join("data")
            .join("test")
            .join("gf_fonts")
            .join("ofl")
            .join("mavenpro");
        println!("Family path: {:?}", family_path);
        let from_fp = FamilyMeta::from_path(&family_path).unwrap();
        let from_json: FamilyMeta = FamilyMeta::from_googlefonts_json(FAMILY_JSON).unwrap();
        let expected = FamilyMeta {
            name: "Maven Pro".to_string(),
            // display_name: Some("Maven Pro".to_string()),
            designers: vec![Designer { name: "Joe Prince".to_string(), bio: "".to_string() }],
            license: "ofl".to_string(),
            category: "SANS_SERIF".to_string(),
            coverage: BTreeMap::from([
                ("latin".to_string(), String::new()),
                ("latin-ext".to_string(), String::new()),
                ("vietnamese".to_string(), String::new()),
            ]),
            stroke: "SANS_SERIF".to_string(),
            classifications: vec![],
            description: Some("Maven Pro is a sans-serif typeface with unique curvature and flowing rhythm. Its forms make it very distinguishable and legible when in context. It blends styles of many great typefaces and is suitable for any design medium. Maven Pro’s modern design is great for the web and fits in any environment. Updated in January 2019 with a Variable Font \"Weight\" axis. The Maven Pro project was initiated by Joe Price, a type designer based in the USA. To contribute, see github.com/googlefonts/mavenproFont".to_string()),
            primary_script: None,
            article: None,
            minisite_url: None,
        };
        assert_eq!(from_fp, expected);
        assert_eq!(from_json, expected);
    }

    #[tokio::test]
    async fn test_family() {
        let family_path: PathBuf = PathBuf::from(CRATE_ROOT)
            .join("data")
            .join("test")
            .join("gf_fonts")
            .join("ofl")
            .join("mavenpro");
        println!("Family path: {:?}", family_path);
        let from_fp = Family::from_path(&family_path).unwrap();
        let from_json: Family = Family::from_googlefonts_json(
            serde_json::from_str(FAMILY_JSON).unwrap(),
            PROD_FAMILY_DOWNLOAD,
        )
        .await
        .unwrap();
        let expected = Family {
            name: "Maven Pro".to_string(),
            version: "Version 2.103".to_string(),
        };
        assert_eq!(from_fp, expected);
        assert_eq!(from_json, expected);
    }

    #[test]
    fn test_designer() {
        let designer_path: PathBuf = PathBuf::from(CRATE_ROOT)
            .join("data")
            .join("test")
            .join("gf_fonts")
            .join("joeprince");
        let from_fp = Designer::from_path(&designer_path).unwrap();
        let designer_json =
            serde_json::from_str::<Value>(FAMILY_JSON).unwrap()["designers"][0].clone();
        println!("Designer JSON: {:?}", designer_json);
        let from_json: Designer =
            Designer::from_googlefonts_json(designer_json, PROD_FAMILY_DOWNLOAD).unwrap();
        let expected = Designer {
            name: "Joe Prince".to_string(),
            bio: "".to_string(),
        };
        assert_eq!(from_fp, expected);
        assert_eq!(from_json, expected);
    }

    #[test]
    fn test_axis() {
        let axis_path: PathBuf = PathBuf::from(CRATE_ROOT)
            .join("data")
            .join("test")
            .join("axisregistry")
            .join("data")
            .join("weight.textproto");
        println!("Axis path: {:?}", axis_path);
        let from_fp = Axis::from_path(&axis_path).unwrap();
        let binding = serde_json::from_str::<Value>(FONTS_JSON).unwrap();
        let axis_from_json: &Value = binding["axisRegistry"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["tag"] == "wght")
            .unwrap();
        let from_json: Axis = serde_json::from_value(axis_from_json.clone()).unwrap();
        let expected = Axis {
            tag: "wght".to_string(),
            display_name: "Weight".to_string(),
            min_value: 1.0,
            default_value: 400.0,
            max_value: 1000.0,
            precision: 0.0,
            fallback: vec![
                AxisFallback {
                    name: "Thin".to_string(),
                    value: 100.0,
                },
                AxisFallback {
                    name: "ExtraLight".to_string(),
                    value: 200.0,
                },
                AxisFallback {
                    name: "Light".to_string(),
                    value: 300.0,
                },
                AxisFallback {
                    name: "Regular".to_string(),
                    value: 400.0,
                },
                AxisFallback {
                    name: "Medium".to_string(),
                    value: 500.0,
                },
                AxisFallback {
                    name: "SemiBold".to_string(),
                    value: 600.0,
                },
                AxisFallback {
                    name: "Bold".to_string(),
                    value: 700.0,
                },
                AxisFallback {
                    name: "ExtraBold".to_string(),
                    value: 800.0,
                },
                AxisFallback {
                    name: "Black".to_string(),
                    value: 900.0,
                },
            ],
            fallback_only: false,
            description: "Adjust the style from lighter to bolder in typographic color, by varying stroke weights, spacing and kerning, and other aspects of the type. This typically changes overall width, and so may be used in conjunction with Width and Grade axes.".to_string(),
        };
        assert_eq!(from_fp, expected);
        assert_eq!(from_json, expected);
    }
}

/*
TEST_DIR = os.path.join(CWD, "..", "..", "data", "test", "gf_fonts")
SERVER_DIR = os.path.join(CWD, "..", "..", "data", "test", "servers")
TEST_FAMILY_DIR = Path(TEST_DIR) / "ofl" / "mavenpro"
DESIGNER_DIR = Path(TEST_DIR) / "joeprince"
WEIGHT_AXIS = file_manager.enter_context(
    as_file(files("axisregistry") / "data" / "weight.textproto")
)


@pytest.mark.parametrize(
    "type_, fp, gf_data, res",
    [
            Axis,
            WEIGHT_AXIS,
            next(a for a in FONTS_JSON["axisRegistry"] if a["tag"] == "wght"),
            Axis(
                tag="wght",
                display_name="Weight",
                min_value=1.0,
                default_value=400.0,
                max_value=1000.0,
                precision=0,
                fallback=[
                    AxisFallback(name="Thin", value=100.0),
                    AxisFallback(name="ExtraLight", value=200.0),
                    AxisFallback(name="Light", value=300.0),
                    AxisFallback(name="Regular", value=400.0),
                    AxisFallback(name="Medium", value=500.0),
                    AxisFallback(name="SemiBold", value=600.0),
                    AxisFallback(name="Bold", value=700.0),
                    AxisFallback(name="ExtraBold", value=800.0),
                    AxisFallback(name="Black", value=900.0),
                ],
                fallback_only=False,
                description="Adjust the style from lighter to bolder in typographic color, by varying stroke weights, spacing and kerning, and other aspects of the type. This typically changes overall width, and so may be used in conjunction with Width and Grade axes.",
            ),
        ),
    ],
 */
