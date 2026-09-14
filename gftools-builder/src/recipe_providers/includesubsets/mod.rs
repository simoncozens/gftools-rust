use crate::{
    error::ApplicationError,
    operations::addsubset::{layout_handling_deser, layout_handling_ser},
};
use google_fonts_glyphsets::GLYPHSETS;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
#[serde(untagged)]
pub enum IncludeSubsetsSource {
    NamedSource(String),
    Github { repo: String, path: String },
}

impl IncludeSubsetsSource {
    pub fn resolve_source(&self) -> Result<(&str, &str), ApplicationError> {
        match self {
            IncludeSubsetsSource::NamedSource(name) if name == "Noto Sans" => Ok((
                "notofonts/latin-greek-cyrillic",
                "sources/NotoSans.glyphspackage",
            )),
            IncludeSubsetsSource::NamedSource(name) if name == "Noto Serif" => Ok((
                "notofonts/latin-greek-cyrillic",
                "sources/NotoSerif.glyphspackage",
            )),
            IncludeSubsetsSource::NamedSource(name) if name == "Noto Sans Devanagari" => Ok((
                "notofonts/devanagari",
                "sources/NotoSansDevanagari.glyphspackage",
            )),
            IncludeSubsetsSource::Github { repo, path } => Ok((repo, path)),
            _ => Err(ApplicationError::InvalidRecipe(format!(
                "Unknown subset source: {:?}",
                self
            ))),
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct UnicodeRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct IncludeSubsetsCodepoints {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub ranges: Option<Vec<UnicodeRange>>,
}

impl IncludeSubsetsCodepoints {
    /// Validate that exactly one of `name` or `ranges` is specified
    pub fn validate(&self) -> Result<(), ApplicationError> {
        match (&self.name, &self.ranges) {
            (Some(_), Some(_)) => Err(ApplicationError::InvalidRecipe(
                "Cannot specify both 'name' and 'ranges' in includeSubsets".to_string(),
            )),
            (None, None) => Err(ApplicationError::InvalidRecipe(
                "Must specify either 'name' or 'ranges' in includeSubsets".to_string(),
            )),
            _ => Ok(()),
        }
    }

    pub(crate) fn resolve(&self) -> Result<Vec<u32>, ApplicationError> {
        self.validate()?;

        match (&self.name, &self.ranges) {
            (Some(name), _) => {
                if let Some(glyphset) = GLYPHSETS.get(name.as_str()) {
                    Ok(glyphset.iter_codepoints().collect())
                } else {
                    Err(ApplicationError::InvalidRecipe(format!(
                        "Unknown glyphset name: {}",
                        name
                    )))
                }
            }
            (_, Some(ranges)) => {
                let mut codepoints = Vec::new();
                for range in ranges {
                    for cp in range.start..=range.end {
                        codepoints.push(cp);
                    }
                }
                Ok(codepoints)
            }
            _ => unreachable!(), // validate() ensures one is set
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct IncludeSubsetsOptions {
    pub from: IncludeSubsetsSource,
    #[serde(flatten)]
    pub subset: IncludeSubsetsCodepoints,
    #[serde(
        default,
        serialize_with = "layout_handling_ser",
        deserialize_with = "layout_handling_deser"
    )]
    pub layout_handling: fontmerge::LayoutHandling,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub exclude_glyphs: Vec<String>,
    #[serde(default)]
    pub exclude_codepoints: Vec<u32>,
    pub exclude_glyphs_file: Option<String>,
    pub exclude_codepoints_file: Option<String>,
}

impl IncludeSubsetsOptions {
    #[allow(dead_code)]
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ApplicationError> {
        self.subset.validate()
    }

    pub fn obtain_donor_font(&self) -> Result<String, ApplicationError> {
        let (repo, path) = self.from.resolve_source()?;
        let (repo, revision) = if let Some(at_pos) = repo.rfind('@') {
            (&repo[..at_pos], &repo[at_pos + 1..])
        } else {
            (repo, "main")
        };
        // Have we downloaded this repo already?
        let cache_dir = dirs::cache_dir()
            .ok_or(ApplicationError::IncludeSubsetsError(
                "Could not determine cache directory".to_string(),
            ))?
            .join("gftools-builder")
            .join("includesubsets")
            .join(repo.replace("/", "_"));
        if !cache_dir.exists() {
            // Download the repo
            std::fs::create_dir_all(&cache_dir).map_err(|e| {
                ApplicationError::IncludeSubsetsError(format!(
                    "Could not create cache directory: {}",
                    e
                ))
            })?;
            // Grab a zipball. Split off @ for ref, use main if not present

            let repo_zipball = format!("https://github.com/{repo}/archive/{revision}.zip");
            log::info!("Downloading donor font from {}...", repo_zipball);
            // This may panic because we're inside Tokio.
            let response = reqwest::blocking::get(&repo_zipball).map_err(|e| {
                ApplicationError::IncludeSubsetsError(format!(
                    "Failed to download donor font from {}: {}",
                    repo_zipball, e
                ))
            })?;
            if !response.status().is_success() {
                return Err(ApplicationError::IncludeSubsetsError(format!(
                    "Failed to download donor font from {}: HTTP {}",
                    repo_zipball,
                    response.status()
                )));
            }
            let zip_bytes = response.bytes().map_err(|e| {
                ApplicationError::IncludeSubsetsError(format!(
                    "Failed to read downloaded donor font from {}: {}",
                    repo_zipball, e
                ))
            })?;
            let reader = std::io::Cursor::new(zip_bytes);
            let mut zip = zip::ZipArchive::new(reader).map_err(|e| {
                ApplicationError::IncludeSubsetsError(format!(
                    "Failed to open zip archive from {}: {}",
                    repo_zipball, e
                ))
            })?;
            zip.extract(&cache_dir).map_err(|e| {
                ApplicationError::IncludeSubsetsError(format!(
                    "Failed to extract zip archive from {}: {}",
                    repo_zipball, e
                ))
            })?;
        }
        // Locate the donor font inside the extracted repo; it'll be in a subdirectory
        // named after the repo-revision. Then we add our full path to it.
        let (_owner, repo_name) =
            repo.split_once('/')
                .ok_or(ApplicationError::IncludeSubsetsError(format!(
                    "Invalid GitHub repo format: {}",
                    repo
                )))?;
        let donor_path = cache_dir
            .join(format!("{}-{}", repo_name, revision))
            .join(path);
        if !donor_path.exists() {
            return Err(ApplicationError::IncludeSubsetsError(format!(
                "Donor font path does not exist: {}",
                donor_path.display()
            )));
        }

        Ok(donor_path.as_os_str().to_string_lossy().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_parse() {
        let yaml = r#"
- from: Noto Sans
  name: GF_Latin_Core
- from: Noto Sans Devanagari
  ranges:
    - start: 0x0964
      end: 0x0965
"#;
        let options: Vec<IncludeSubsetsOptions> = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(
            options[0],
            IncludeSubsetsOptions {
                from: IncludeSubsetsSource::NamedSource("Noto Sans".to_string()),
                subset: IncludeSubsetsCodepoints {
                    name: Some("GF_Latin_Core".to_string()),
                    ranges: None,
                },
                layout_handling: fontmerge::LayoutHandling::Subset,
                force: false,
                exclude_glyphs: vec![],
                exclude_codepoints: vec![],
                exclude_glyphs_file: None,
                exclude_codepoints_file: None,
            }
        );
        assert_eq!(
            options[1],
            IncludeSubsetsOptions {
                from: IncludeSubsetsSource::NamedSource("Noto Sans Devanagari".to_string()),
                subset: IncludeSubsetsCodepoints {
                    name: None,
                    ranges: Some(vec![UnicodeRange {
                        start: 0x0964,
                        end: 0x0965
                    }]),
                },
                layout_handling: fontmerge::LayoutHandling::Subset,
                force: false,
                exclude_glyphs: vec![],
                exclude_codepoints: vec![],
                exclude_glyphs_file: None,
                exclude_codepoints_file: None,
            }
        );
    }

    #[test]
    fn test_validation_fails_with_both_fields() {
        let subset = IncludeSubsetsCodepoints {
            name: Some("GF_Latin_Core".to_string()),
            ranges: Some(vec![UnicodeRange {
                start: 0x0964,
                end: 0x0965,
            }]),
        };
        assert!(subset.validate().is_err());
    }

    #[test]
    fn test_validation_fails_with_neither_field() {
        let subset = IncludeSubsetsCodepoints {
            name: None,
            ranges: None,
        };
        assert!(subset.validate().is_err());
    }

    #[test]
    fn test_resolve_named_glyphset() {
        let subset = IncludeSubsetsCodepoints {
            name: Some("GF_Latin_Core".to_string()),
            ranges: None,
        };
        let codepoints = subset.resolve().unwrap();
        assert!(codepoints.contains(&0x0041)); // 'A'
        assert!(!codepoints.contains(&0x0410)); // 'Ж'
    }
}
