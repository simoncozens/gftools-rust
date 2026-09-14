use std::path::{Path, PathBuf};

use scraper::Html;
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value;
use skrifa::{raw::TableProvider, string::StringId, FontRef, MetadataProvider};

use gf_metadata::{DesignerInfoProto, FamilyProto};
use gftools::{parse_metadatapb, GftoolsError};

fn parse_html_file(p: &PathBuf) -> Result<String, GftoolsError> {
    parse_html(
        &std::fs::read_to_string(p)
            .map_err(|_| GftoolsError::Misc(format!("Failed to read HTML file: {:?}", p)))?,
    )
}

fn parse_html(s: &str) -> Result<String, GftoolsError> {
    let document = Html::parse_fragment(&s);
    Ok(document
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
        .replace("\n", " ")
        .to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Family {
    pub name: String,
    pub version: String,
}

fn font_version(f: &FontRef) -> String {
    if let Some(version) = f
        .localized_strings(StringId::VERSION_STRING)
        .english_or_first()
        .map(|name| name.chars().collect())
    {
        version
    } else {
        f.head()
            .map(|head| head.font_revision().to_string())
            .unwrap_or_else(|_| "0.0.0".to_string())
    }
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
    pub(crate) fn from_filepath(f: &PathBuf) -> Result<Self, GftoolsError> {
        let contents = std::fs::read(f)?;
        let font = skrifa::FontRef::new(&contents)?;
        Ok(Self::from_fontref(font))
    }
    pub(crate) fn from_googlefonts_json(data: Value, url: &str) -> Result<Self, GftoolsError> {
        let name = data
            .as_object()
            .and_then(|m| m.get("family"))
            .and_then(|f| f.as_str())
            .ok_or_else(|| {
                GftoolsError::Misc(format!(
                    "Couldn't find family in JSON: {}",
                    data.to_string()
                ))
            })?;
        Self::from_googlefonts(name, url)
    }
    pub(crate) fn from_googlefonts(_name: &str, _url: &str) -> Result<Self, GftoolsError> {
        unimplemented!()
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
    pub fallback: Vec<AxisFallback>,
    #[serde(default)]
    pub fallback_only: bool,
    pub description: String,
}

impl Axis {
    fn from_path(_path: &Path) -> Result<Self, GftoolsError> {
        unimplemented!()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FamilyMeta {
    #[serde(rename = "family")]
    pub name: String,

    // Not used in gftools-python?
    // #[serde(default, rename = "displayName")]
    // pub display_name: Option<String>,
    pub designers: Vec<Designer>,
    pub license: String,
    pub category: String,
    #[serde(rename = "coverage", deserialize_with = "get_keys")]
    pub subsets: Vec<String>,
    #[serde(deserialize_with = "deserialize_null_default", default)]
    pub stroke: String,
    pub classifications: Vec<String>,
    pub description: String,
    pub primary_script: Option<String>,
    pub article: Option<String>,
    pub minisite_url: Option<String>,
}

fn get_keys<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: serde_json::Map<String, Value> = de::Deserialize::deserialize(deserializer)?;
    Ok(s.keys().cloned().collect())
}

impl FamilyMeta {
    fn from_path(path: &Path) -> Result<Self, GftoolsError> {
        let data = parse_metadatapb::<FamilyProto>(path)?;
        let stroke = data
            .stroke
            .as_ref()
            .map(|x| x.replace("_,", " ").to_uppercase())
            .or_else(|| data.category.first().cloned())
            .unwrap_or_else(|| "UNKNOWN".to_string());

        let article = path.join("article").join("ARTICLE.en_us.html");
        let article = if article.exists() {
            Some(parse_html_file(&article)?)
        } else {
            None
        };
        let description = path.join("DESCRIPTION.en_us.html");
        let description = if description.exists() {
            Some(parse_html_file(&description)?)
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
                            name: s.to_string(),
                            bio: "".to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            license: data.license().to_string().to_lowercase(),
            // display_name: data.display_name.map(|x| x.to_string()),
            category: data.category.first().cloned().unwrap_or_default(),
            subsets: data
                .subsets
                .iter()
                .filter(|&x| x != &"menu".to_string())
                .cloned()
                .collect(),
            stroke,
            classifications: data.classifications,
            description: description.unwrap_or_default(),
            primary_script: data.primary_script,
            article,
            minisite_url: data.minisite_url,
        })
    }

    fn from_googlefonts_json(s: &str) -> Result<Self, GftoolsError> {
        let mut initial: Self = serde_json::from_str(s)
            .map_err(|e| GftoolsError::Misc(format!("Failed to parse JSON: {}", e)))?;
        initial.stroke = initial.stroke.replace(" ", "_").to_uppercase();
        initial.category = initial.category.replace(" ", "_").to_uppercase();
        initial.description = parse_html(&initial.description)?;
        initial.article = initial
            .article
            .as_ref()
            .map(|text| parse_html(text))
            .transpose()?;
        Ok(initial)
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
        let data = parse_metadatapb::<DesignerInfoProto>(path)?;
        let bio = path.join("bio.html");
        let bio = bio.exists().then(|| parse_html_file(&bio)).transpose()?;
        Ok(Self {
            name: data.designer().to_string(),
            bio: bio.unwrap_or_default(),
        })
    }
    pub(crate) fn from_googlefonts_json(data: Value, _url: &str) -> Result<Self, GftoolsError> {
        Ok(Self {
            name: data["designer"]
                .as_str()
                .ok_or(GftoolsError::Misc(format!(
                    "Couldn't find designer in JSON: {}",
                    data
                )))?
                .to_string(),
            bio: data["bio"]
                .as_str()
                .ok_or(GftoolsError::Misc(format!(
                    "Couldn't find bio in JSON: {}",
                    data
                )))?
                .to_string(),
        })
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) enum PushItem {
    Family(Family),
    AxisFallback(AxisFallback),
    Axis(Axis),
    FamilyMeta(FamilyMeta),
    Designer(Designer),
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
    use crate::servers::PROD_FAMILY_DOWNLOAD;

    use super::*;
    use pretty_assertions::{assert_eq, assert_ne};

    const CRATE_ROOT: &str = env!("CARGO_MANIFEST_DIR");

    const FAMILY_JSON: &str = include_str!("../data/test/servers/family.json");
    const fonts_json: &str = include_str!("../data/test/servers/fonts.json");

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
            subsets: vec!["latin".to_string(), "latin-ext".to_string(), "vietnamese".to_string()],
            stroke: "SANS_SERIF".to_string(),
            classifications: vec![],
            description: "Maven Pro is a sans-serif typeface with unique curvature and flowing rhythm. Its forms make it very distinguishable and legible when in context. It blends styles of many great typefaces and is suitable for any design medium. Maven Pro’s modern design is great for the web and fits in any environment. Updated in January 2019 with a Variable Font \"Weight\" axis. The Maven Pro project was initiated by Joe Price, a type designer based in the USA. To contribute, see github.com/googlefonts/mavenproFont".to_string(),
            primary_script: None,
            article: None,
            minisite_url: None,
        };
        assert_eq!(from_fp, expected);
        assert_eq!(from_json, expected);
    }

    fn test_family() {
        let family_path: PathBuf = PathBuf::from(CRATE_ROOT)
            .join("data")
            .join("test")
            .join("gf_fonts")
            .join("ofl")
            .join("mavenpro");
        println!("Family path: {:?}", family_path);
        let from_fp = Family::from_path(&family_path).unwrap();
        let from_json: Family =
            Family::from_googlefonts_json(FAMILY_JSON, PROD_FAMILY_DOWNLOAD).unwrap();
        let expected = Family {
            name: "Maven Pro".to_string(),
            version: "Version 2.103".to_string(),
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
        (
            Family,
            TEST_FAMILY_DIR,
            next(
                f
                for f in FONTS_JSON["familyMetadataList"]
                if f["family"] == "Maven Pro"
            ),
            Family(
                name="Maven Pro",
                version="Version 2.103",
            ),
        ),
        (
            Designer,
            DESIGNER_DIR,
            FAMILY_JSON["designers"][0],
            Designer(name="Joe Prince", bio=None),
        ),
        (
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
