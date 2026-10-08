use std::collections::BTreeSet;

use crate::{
    error::ApplicationError,
    operations::{
        ConfigOperationBuilder,
        addsubset::{AddSubsetConfig, layout_handling_deser, layout_handling_ser},
    },
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

/// Consolidate a list of include-subsets specifications down to the minimal
/// number of merge operations.
///
/// Subsets that share a donor font and the same merge options (layout handling,
/// force, and any exclusion files) are combined into a single specification
/// whose codepoints are the union of the originals. This mirrors the Python
/// builder's `prepare_minimal_subsets`: specifying two subsets from the same
/// donor produces a single `AddSubset` operation rather than two.
///
/// Codepoints excluded by one subset are removed from that subset's
/// contribution before the union, so an exclusion in one subset does not affect
/// the codepoints requested by another.
pub fn minimize_subsets(
    subsets: &[IncludeSubsetsOptions],
) -> Result<Vec<IncludeSubsetsOptions>, ApplicationError> {
    struct Group {
        template: IncludeSubsetsOptions,
        codepoints: BTreeSet<u32>,
        exclude_glyphs: BTreeSet<String>,
        members: usize,
    }

    let mut groups: Vec<Group> = Vec::new();
    for subset in subsets {
        let codepoints = subset.subset.resolve()?;
        let group = match groups
            .iter_mut()
            .find(|group| same_merge_group(&group.template, subset))
        {
            Some(group) => group,
            None => {
                groups.push(Group {
                    template: subset.clone(),
                    codepoints: BTreeSet::new(),
                    exclude_glyphs: BTreeSet::new(),
                    members: 0,
                });
                groups.last_mut().expect("a group was just pushed")
            }
        };
        group.codepoints.extend(
            codepoints
                .into_iter()
                .filter(|codepoint| !subset.exclude_codepoints.contains(codepoint)),
        );
        group
            .exclude_glyphs
            .extend(subset.exclude_glyphs.iter().cloned());
        group.members += 1;
    }

    Ok(groups
        .into_iter()
        .map(|group| {
            // A lone subset with nothing to exclude needs no rewriting. Keeping it
            // verbatim preserves named glyphsets (e.g. `GF_Latin_Core`) instead of
            // expanding them into explicit ranges.
            if group.members == 1 && group.template.exclude_codepoints.is_empty() {
                return group.template;
            }
            let mut options = group.template;
            options.subset = IncludeSubsetsCodepoints {
                name: None,
                ranges: Some(codepoints_to_ranges(&group.codepoints)),
            };
            options.exclude_glyphs = group.exclude_glyphs.into_iter().collect();
            // Excluded codepoints have been folded into `subset` above.
            options.exclude_codepoints = Vec::new();
            options
        })
        .collect())
}

/// Two subsets can be merged when they draw from the same donor with the same
/// merge options. The exclusion *files* must also match, because we cannot merge
/// two different files into one field.
fn same_merge_group(a: &IncludeSubsetsOptions, b: &IncludeSubsetsOptions) -> bool {
    a.from == b.from
        && a.layout_handling == b.layout_handling
        && a.force == b.force
        && a.exclude_glyphs_file == b.exclude_glyphs_file
        && a.exclude_codepoints_file == b.exclude_codepoints_file
}

/// Compress a sorted set of codepoints into the smallest set of inclusive ranges.
fn codepoints_to_ranges(codepoints: &BTreeSet<u32>) -> Vec<UnicodeRange> {
    let mut ranges = Vec::new();
    let mut iter = codepoints.iter().copied();
    let Some(first) = iter.next() else {
        return ranges;
    };
    let mut start = first;
    let mut end = first;
    for codepoint in iter {
        if codepoint == end.saturating_add(1) {
            end = codepoint;
        } else {
            ranges.push(UnicodeRange { start, end });
            start = codepoint;
            end = codepoint;
        }
    }
    ranges.push(UnicodeRange { start, end });
    ranges
}

/// Append the `AddSubset` operations described by `subsets` to `builder`.
///
/// Shared by the Google Fonts and Noto recipe providers. The subsets are
/// consolidated with [`minimize_subsets`] first, so that a source only ever gets
/// one merge per donor font.
pub fn add_subset_steps(
    mut builder: ConfigOperationBuilder,
    subsets: &[IncludeSubsetsOptions],
) -> Result<ConfigOperationBuilder, ApplicationError> {
    for options in minimize_subsets(subsets)? {
        let donor_font = options.obtain_donor_font()?;
        let codepoints = options.subset.resolve()?;
        builder = builder.add_subset(
            &AddSubsetConfig {
                include_glyphs: vec![],
                exclude_glyphs: options.exclude_glyphs.clone(),
                include_codepoints: codepoints,
                existing_glyph_handling: if options.force {
                    fontmerge::ExistingGlyphHandling::Replace
                } else {
                    fontmerge::ExistingGlyphHandling::Skip
                },
                layout_handling: options.layout_handling,
            },
            &donor_font,
        );
    }
    Ok(builder)
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

    fn options(
        from: &str,
        range: Option<(u32, u32)>,
        exclude_codepoints: Vec<u32>,
    ) -> IncludeSubsetsOptions {
        IncludeSubsetsOptions {
            from: IncludeSubsetsSource::NamedSource(from.to_string()),
            subset: IncludeSubsetsCodepoints {
                name: None,
                ranges: range.map(|(start, end)| vec![UnicodeRange { start, end }]),
            },
            layout_handling: fontmerge::LayoutHandling::Subset,
            force: false,
            exclude_glyphs: vec![],
            exclude_codepoints,
            exclude_glyphs_file: None,
            exclude_codepoints_file: None,
        }
    }

    #[test]
    fn test_minimize_merges_subsets_from_the_same_donor() {
        let subsets = vec![
            options("Noto Sans", Some((0x41, 0x43)), vec![]),
            options("Noto Sans", Some((0x43, 0x45)), vec![]),
        ];

        let minimized = minimize_subsets(&subsets).unwrap();

        assert_eq!(minimized.len(), 1);
        assert_eq!(
            minimized[0].subset.resolve().unwrap(),
            vec![0x41, 0x42, 0x43, 0x44, 0x45]
        );
    }

    #[test]
    fn test_minimize_keeps_different_donors_separate() {
        let subsets = vec![
            options("Noto Sans", Some((0x41, 0x41)), vec![]),
            options("Noto Serif", Some((0x41, 0x41)), vec![]),
        ];

        assert_eq!(minimize_subsets(&subsets).unwrap().len(), 2);
    }

    #[test]
    fn test_minimize_keeps_distinct_options_separate() {
        let force = IncludeSubsetsOptions {
            force: true,
            ..options("Noto Sans", Some((0x41, 0x41)), vec![])
        };
        let subsets = vec![options("Noto Sans", Some((0x41, 0x41)), vec![]), force];

        assert_eq!(minimize_subsets(&subsets).unwrap().len(), 2);
    }

    #[test]
    fn test_minimize_leaves_single_named_subset_alone() {
        let subsets = vec![IncludeSubsetsOptions {
            from: IncludeSubsetsSource::NamedSource("Noto Sans".to_string()),
            subset: IncludeSubsetsCodepoints {
                name: Some("GF_Latin_Core".to_string()),
                ranges: None,
            },
            ..options("Noto Sans", None, vec![])
        }];

        let minimized = minimize_subsets(&subsets).unwrap();

        // The named glyphset should be preserved verbatim rather than expanded.
        assert_eq!(minimized, subsets);
    }

    #[test]
    fn test_minimize_bakes_excluded_codepoints() {
        let subsets = vec![options("Noto Sans", Some((0x41, 0x43)), vec![0x41])];

        let minimized = minimize_subsets(&subsets).unwrap();

        assert_eq!(minimized.len(), 1);
        assert_eq!(minimized[0].subset.resolve().unwrap(), vec![0x42, 0x43]);
        assert!(minimized[0].exclude_codepoints.is_empty());
    }
}
