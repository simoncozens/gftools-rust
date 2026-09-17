//! Walk a directory tree and set the `primary_script` field in METADATA.pb.
//!
//! ```sh
//! gftools-set-primary-script ofl/
//! ```
//!
//! Each family's script is guessed from its first font with
//! [`gftools::primary_script`]. Latin is discounted unless the family
//! directory name starts with `noto`, and families whose script cannot be
//! determined (or is Latin) are left alone.
//!
//! Like the Python original, this rewrites METADATA.pb in place and reports
//! each family it touched on stdout. Unlike the Python original it carries on
//! when a family is unreadable, rather than raising.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use gftools::{parse_pb, primary_script, write_family_metadata, FamilyProto};
use skrifa::FontRef;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
/// Walk a directory tree and set the primary script
struct Args {
    /// Directory tree to walk
    directory: PathBuf,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = Args::parse();
    set_primary_scripts(&args.directory)?;
    Ok(())
}

/// Find every METADATA.pb under `directory` and set its `primary_script`.
///
/// Returns the `(family directory name, script)` pairs which were written.
fn set_primary_scripts(directory: &Path) -> Result<Vec<(String, String)>> {
    // The directory is a literal path, not a pattern, so escape any glob
    // metacharacters in it before appending our own pattern.
    let mut pattern = glob::Pattern::escape(&directory.to_string_lossy());
    if !pattern.ends_with(std::path::MAIN_SEPARATOR) {
        pattern.push(std::path::MAIN_SEPARATOR);
    }
    pattern.push_str("**/METADATA.pb");

    let mut updated = Vec::new();
    let paths = glob::glob(&pattern).with_context(|| format!("Bad search pattern: {pattern}"))?;
    for entry in paths {
        let path = match entry {
            Ok(path) => path,
            Err(e) => {
                log::warn!("Skipping unreadable path: {e}");
                continue;
            }
        };
        let mut family: FamilyProto = match parse_pb(&path) {
            Ok(family) => family,
            Err(e) => {
                log::warn!("{} doesn't conform to font schema: {}", path.display(), e);
                continue;
            }
        };
        let Some(directory) = path.parent() else {
            log::warn!("{}: has no parent directory, skipping", path.display());
            continue;
        };
        let Some(font_filename) = family.fonts.first().and_then(|f| f.filename.as_deref()) else {
            log::warn!("{}: no fonts to check, skipping", path.display());
            continue;
        };
        let font_path = directory.join(font_filename);
        let Ok(font_data) = std::fs::read(&font_path) else {
            log::warn!(
                "{}: couldn't read {}, skipping",
                path.display(),
                font_path.display()
            );
            continue;
        };
        let Ok(font) = FontRef::new(&font_data) else {
            log::warn!(
                "{}: couldn't parse {}, skipping",
                path.display(),
                font_path.display()
            );
            continue;
        };
        // Noto families usually have full Latin coverage as well, which would
        // otherwise drown out the script they exist to support.
        let ignore_latin = directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("noto"));
        let Some(script) = primary_script(&font, ignore_latin) else {
            continue;
        };
        if script == "Latn" {
            continue;
        }

        let family_name = directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!("{family_name} -> {script}");

        family.primary_script = Some(script.clone());
        let contents = write_family_metadata(&family, true)?;
        std::fs::write(&path, contents)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        updated.push((family_name, script));
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture family directories, and the files in each one we need. They
    /// live in gftools-lib, so we walk up out of gftools-bin to find them.
    const FIXTURES: &str = "../gftools-lib/resources/test";
    const FAMILIES: &[(&str, &[&str])] = &[
        ("gulzar", &["METADATA.pb", "Gulzar-Regular.ttf"]),
        (
            "notosansbuhid",
            &["METADATA.pb", "NotoSansBuhid-Regular.ttf"],
        ),
        // No font file: this family should be skipped, not fatal.
        ("pushster", &["METADATA.pb"]),
    ];

    fn fixture(path: &str) -> PathBuf {
        Path::new(FIXTURES).join(path)
    }

    fn copy_fixtures(destination: &Path) {
        for (family, files) in FAMILIES {
            let directory = destination.join(family);
            std::fs::create_dir(&directory).unwrap();
            for file in *files {
                let from = fixture(family).join(file);
                std::fs::copy(&from, directory.join(file))
                    .unwrap_or_else(|e| panic!("Failed to copy {}: {e}", from.display()));
            }
        }
    }

    /// The metadata we expect to end up with, whatever order the text proto
    /// writer chooses for the fields: the fixture plus `script`.
    fn expected_metadata(family: &str, script: &str) -> FamilyProto {
        let mut expected: FamilyProto = parse_pb(&fixture(family).join("METADATA.pb")).unwrap();
        expected.primary_script = Some(script.to_string());
        expected
    }

    #[test]
    fn test_set_primary_script() {
        let tmp = tempfile::tempdir().unwrap();
        copy_fixtures(tmp.path());
        // Give both families the wrong value first, so the test exercises the
        // write path rather than rewriting the same content.
        for (family, correct, wrong) in [
            ("gulzar", "Arab", "Zinh"),
            ("notosansbuhid", "Buhd", "Latn"),
        ] {
            let path = tmp.path().join(family).join("METADATA.pb");
            let metadata = std::fs::read_to_string(&path).unwrap();
            let metadata = metadata.replace(
                &format!("primary_script: \"{correct}\""),
                &format!("primary_script: \"{wrong}\""),
            );
            assert!(
                metadata.contains(wrong),
                "{family} fixture has no {correct}"
            );
            std::fs::write(&path, metadata).unwrap();
        }

        let mut updated = set_primary_scripts(tmp.path()).unwrap();
        updated.sort();
        assert_eq!(
            updated,
            vec![
                ("gulzar".to_string(), "Arab".to_string()),
                ("notosansbuhid".to_string(), "Buhd".to_string())
            ]
        );

        // Gulzar keeps its script even though Latin wasn't discounted; Noto
        // Sans Buhid only gets one because it was. Both fixtures should now be
        // as they were, with only the wrong primary_script corrected. We
        // compare parsed protos rather than the files themselves, since the
        // text proto writers order fields differently.
        for (family, script) in [("gulzar", "Arab"), ("notosansbuhid", "Buhd")] {
            let written: FamilyProto =
                parse_pb(&tmp.path().join(family).join("METADATA.pb")).unwrap();
            assert_eq!(
                written,
                expected_metadata(family, script),
                "{family}'s METADATA.pb was not written as expected"
            );
        }

        // Pushster has no font file, so it's skipped and left untouched.
        assert_eq!(
            std::fs::read(tmp.path().join("pushster/METADATA.pb")).unwrap(),
            std::fs::read(fixture("pushster").join("METADATA.pb")).unwrap()
        );
    }
}
