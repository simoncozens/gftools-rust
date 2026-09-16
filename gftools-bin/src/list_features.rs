//! Tool to print GPOS and GSUB features supported by font file(s).
use anyhow::{self, Context, Result};
use clap::Parser;
use skrifa::{raw::TableProvider, Tag};
use std::path::PathBuf;

#[derive(Parser)]
struct FindFeatures {
    /// Path to the font file or directory
    #[clap(required = true)]
    path: PathBuf,
}

struct FeatureInfo {
    table: Tag,
    feature: Tag,
    lookups: Vec<u16>,
}
impl std::fmt::Display for FeatureInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lookup_joined = self
            .lookups
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(",");
        write!(
            f,
            "{:>4} {:>8} {:>15}",
            self.table.to_string(),
            self.feature.to_string(),
            "[".to_string() + &lookup_joined + "]"
        )
    }
}

fn list_features(font: &skrifa::FontRef) -> Vec<FeatureInfo> {
    let mut result: Vec<FeatureInfo> = Vec::new();
    let gsub_featurelist = font.gsub().ok().and_then(|gsub| gsub.feature_list().ok());
    let gpos_featurelist = font.gpos().ok().and_then(|gpos| gpos.feature_list().ok());
    let lists = [
        (Tag::new(b"GSUB"), gsub_featurelist),
        (Tag::new(b"GPOS"), gpos_featurelist),
    ];
    for (table, featurelist) in lists.iter() {
        if let Some(features) = featurelist.as_ref().map(|list| {
            list.feature_records().iter().flat_map(move |feature| {
                feature
                    .feature(list.offset_data())
                    .map(|f| FeatureInfo {
                        feature: feature.feature_tag(),
                        table: *table,
                        lookups: f
                            .lookup_list_indices()
                            .iter()
                            .map(|x| x.get())
                            .collect::<Vec<_>>(),
                    })
                    .ok()
            })
        }) {
            result.extend(features);
        }
    }
    result
}

fn main() -> Result<(), anyhow::Error> {
    let args = FindFeatures::parse();
    let font_files = if args.path.is_dir() {
        glob::glob(&format!("{}/*.ttf", args.path.display()))
            .with_context(|| {
                format!(
                    "Failed to read font files from directory: {}",
                    args.path.display()
                )
            })?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![args.path]
    };
    for file in font_files {
        let font_binary = std::fs::read(&file)
            .with_context(|| format!("Failed to read font file: {}", file.display()))?;
        let font_ref = skrifa::FontRef::new(&font_binary)
            .with_context(|| format!("Failed to parse font: {}", file.display()))?;
        let features = list_features(&font_ref);
        for feature in features {
            println!("{:>32} {}", file.display().to_string(), feature);
        }
    }

    Ok(())
}
