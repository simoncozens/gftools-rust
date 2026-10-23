//! Removes unwanted tables from one or more font files.
//!
//! ```sh
//! gftools-fix-unwanted-tables font.ttf
//! gftools-fix-unwanted-tables -t TSI0,TSI1 font.ttf
//! ```
//!
//! With `-t`, exactly the listed tables are removed. Without it, fontspector's
//! `unwanted_tables` hotfix removes every table on its unwanted list. Fonts are
//! rewritten in place.

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use fontspector_hotfix::{Testable, apply_hotfixes};
use gftools::remove_tables;
use skrifa::Tag;
use std::collections::HashSet;
use std::path::PathBuf;

/// Removes unwanted tables from one or more font files
#[derive(Debug, Parser)]
struct Args {
    /// One or more comma separated table names
    #[arg(short, long)]
    tables: Option<String>,

    /// One or more font files
    #[arg(required = true)]
    fontpath: Vec<PathBuf>,
}

fn parse_tables(spec: &str) -> Result<HashSet<Tag>> {
    spec.split(',')
        .map(|name| {
            Tag::new_checked(name.as_bytes())
                .map_err(|e| anyhow!("Invalid table tag `{name}`: {e}"))
        })
        .collect()
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    let requested = args.tables.as_deref().map(parse_tables).transpose()?;

    for path in &args.fontpath {
        let out = match &requested {
            Some(tags) => {
                let data = std::fs::read(path)
                    .with_context(|| format!("Error reading font `{}`", path.display()))?;
                remove_tables(data, tags.clone())
                    .with_context(|| format!("Failed to remove tables from `{}`", path.display()))?
            }
            None => {
                let mut font = Testable::new(path)
                    .map_err(|e| anyhow!("Failed to load font `{}`: {e}", path.display()))?;
                apply_hotfixes(&mut font, &["unwanted_tables".to_string()], false)
                    .map_err(|_| anyhow!("Failed to apply hotfixes to `{}`", path.display()))?;
                font.contents
            }
        };
        std::fs::write(path, out).with_context(|| format!("Error writing `{}`", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::FontRef;

    #[test]
    fn parses_tags() {
        let tags = parse_tables("Debg,TSI0").unwrap();
        assert!(tags.contains(&Tag::new(b"Debg")) && tags.contains(&Tag::new(b"TSI0")));
        assert!(parse_tables("toolong").is_err());
    }

    #[test]
    fn hotfix_removes_default_unwanted_tables() {
        let src = "../gftools-onboarder-tools/data/test/gf_fonts/ofl/mavenpro/MavenPro[wght].ttf";
        let data = std::fs::read(src).unwrap();
        let font = FontRef::new(&data).unwrap();
        let mut builder = write_fonts::FontBuilder::new();
        for r in font.table_directory().table_records() {
            builder.add_raw(r.tag(), font.table_data(r.tag()).unwrap());
        }
        builder.add_raw(Tag::new(b"Debg"), b"{}".to_vec());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.ttf");
        std::fs::write(&path, builder.build()).unwrap();

        let mut font = Testable::new(&path).unwrap();
        apply_hotfixes(&mut font, &["unwanted_tables".to_string()], false).unwrap();
        let out = FontRef::new(&font.contents).unwrap();
        assert!(out.table_data(Tag::new(b"Debg")).is_none());
        assert!(out.table_data(Tag::new(b"glyf")).is_some());
    }

    #[test]
    fn removes_requested_tables() {
        let data = std::fs::read(
            "../gftools-onboarder-tools/data/test/gf_fonts/ofl/mavenpro/MavenPro[wght].ttf",
        )
        .unwrap();
        let font = FontRef::new(&data).unwrap();
        let mut builder = write_fonts::FontBuilder::new();
        for r in font.table_directory().table_records() {
            builder.add_raw(r.tag(), font.table_data(r.tag()).unwrap());
        }
        builder.add_raw(Tag::new(b"Debg"), b"{}".to_vec());
        let with_debg = builder.build();

        let out = remove_tables(with_debg, HashSet::from([Tag::new(b"Debg")])).unwrap();
        let out = FontRef::new(&out).unwrap();
        assert!(out.table_data(Tag::new(b"Debg")).is_none());
        assert!(out.table_data(Tag::new(b"glyf")).is_some());
    }
}
