//! Update specific nameIDs in a collection of fonts with new strings.
//!
//! ```sh
//! gftools-update-nameids -c "Copyright 2016" font.ttf
//! gftools-update-nameids -v "4.000" --urllicense "http://license.org" font.ttf
//! ```
//!
//! If you need to change the name or style of a collection of font families,
//! use `gftools-nametable-from-filename` instead.
//!
//! Like the Python original, the result is written next to the input as
//! `<font>.fix` and the input file is left alone.
//!
//! `-v`/`--version` here is the *name ID 5* replacement string rather than a
//! request to print the tool's version, so clap's version flag is disabled.

use anyhow::{Context, Result};
use clap::Parser;
use skrifa::raw::tables::name::{Encoding, MacRomanMapping};
use skrifa::{FontRef, raw::TableProvider as _};
use std::path::{Path, PathBuf};
use write_fonts::{FontBuilder, from_obj::ToOwnedTable, tables::name::Name, types::NameId};

/// Update specific nameIDs in a collection of fonts with new strings.
///
/// gftools-update-nameids -c "Copyright 2016" font.ttf
///
/// if you need to change the name or style of a collection of font families,
/// use gftools-nametable-from-filename instead.
///
/// Each font is written to `<font>.fix`; the input file is not modified.
#[derive(Debug, Parser)]
#[command(disable_version_flag = true)]
struct Args {
    /// Font files to update
    #[arg(required = true)]
    fonts: Vec<PathBuf>,

    /// Update copyright string (name ID 0)
    #[arg(short = 'c', long)]
    copyright: Option<String>,

    /// Update uniqueid string (name ID 3)
    #[arg(short = 'u', long)]
    uniqueid: Option<String>,

    /// Update version string (name ID 5)
    #[arg(short = 'v', long)]
    version: Option<String>,

    /// Update trademark string (name ID 7)
    #[arg(short = 't', long)]
    trademark: Option<String>,

    /// Update manufacturer string (name ID 8)
    #[arg(short = 'm', long)]
    manufacturer: Option<String>,

    /// Update designer string (name ID 9)
    #[arg(short = 'd', long)]
    designer: Option<String>,

    /// Update description string (name ID 10)
    #[arg(long, alias = "desc")]
    description: Option<String>,

    /// Update url vendor string (name ID 11)
    #[arg(long, alias = "uv")]
    urlvendor: Option<String>,

    /// Update url designer string (name ID 12)
    #[arg(long, alias = "ud")]
    urldesigner: Option<String>,

    /// Update license string (name ID 13)
    #[arg(short = 'l', long)]
    license: Option<String>,

    /// Update url license string (name ID 14)
    #[arg(long, alias = "ul")]
    urllicense: Option<String>,
}

impl Args {
    /// The replacement string for a name ID, if the user supplied one.
    fn replacement(&self, name_id: NameId) -> Option<&str> {
        let text = match name_id {
            NameId::COPYRIGHT_NOTICE => self.copyright.as_deref(),
            NameId::UNIQUE_ID => self.uniqueid.as_deref(),
            NameId::VERSION_STRING => self.version.as_deref(),
            NameId::TRADEMARK => self.trademark.as_deref(),
            NameId::MANUFACTURER => self.manufacturer.as_deref(),
            NameId::DESIGNER => self.designer.as_deref(),
            NameId::DESCRIPTION => self.description.as_deref(),
            NameId::VENDOR_URL => self.urlvendor.as_deref(),
            NameId::DESIGNER_URL => self.urldesigner.as_deref(),
            NameId::LICENSE_DESCRIPTION => self.license.as_deref(),
            NameId::LICENSE_URL => self.urllicense.as_deref(),
            _ => None,
        };
        text.filter(|text| !text.is_empty())
    }
}

/// Check that every name record in the table can be written back out.
fn check_name_table(name: &Name) -> Result<()> {
    for record in &name.name_record {
        match Encoding::new(record.platform_id, record.encoding_id) {
            Encoding::Utf16Be => {}
            Encoding::MacRoman => {
                for c in record.string.to_string().chars() {
                    if MacRomanMapping.encode(c).is_none() {
                        anyhow::bail!(
                            "name ID {} on the Mac platform cannot hold {c:?}: \
                             not representable in MacRoman",
                            record.name_id.to_u16()
                        );
                    }
                }
            }
            Encoding::Unknown => anyhow::bail!(
                "name ID {} uses an unsupported platform/encoding pair ({}, {})",
                record.name_id.to_u16(),
                record.platform_id,
                record.encoding_id
            ),
        }
    }
    Ok(())
}

/// Rewrite the requested name records of one font, writing `<font>.fix` and
/// returning its path.
fn update_font(path: &Path, args: &Args) -> Result<PathBuf> {
    let font_contents = std::fs::read(path)
        .with_context(|| format!("Failed to read font file: {}", path.display()))?;
    let font_ref = FontRef::new(&font_contents)
        .with_context(|| format!("Failed to parse font file: {}", path.display()))?;
    let mut name: Name = font_ref.name()?.to_owned_table();

    // Python walks the name records and calls ``setName(text, *fields)``, which
    // replaces the string of the record with those exact IDs; only the string
    // changes, so the records stay in place and keep their platform, encoding
    // and language IDs.
    for record in name.name_record.iter_mut() {
        if let Some(text) = args.replacement(record.name_id) {
            record.string = text.to_string().into();
        }
    }

    check_name_table(&name)?;

    let mut builder = FontBuilder::new();
    builder
        .add_table(&name)
        .with_context(|| "Failed to build name table".to_string())?;
    builder.copy_missing_tables(font_ref);

    let mut out_path = path.as_os_str().to_owned();
    out_path.push(".fix");
    let out_path = PathBuf::from(out_path);
    std::fs::write(&out_path, builder.build())
        .with_context(|| format!("Failed to write font file: {}", out_path.display()))?;
    Ok(out_path)
}

fn main() -> Result<()> {
    let args = Args::parse();
    for font in &args.fonts {
        let out_path = update_font(font, &args)?;
        println!("font saved {}", out_path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use write_fonts::tables::name::NameRecord;

    /// Every name ID the script knows about, plus two it must leave alone.
    const UPDATABLE: [NameId; 11] = [
        NameId::COPYRIGHT_NOTICE,
        NameId::UNIQUE_ID,
        NameId::VERSION_STRING,
        NameId::TRADEMARK,
        NameId::MANUFACTURER,
        NameId::DESIGNER,
        NameId::DESCRIPTION,
        NameId::VENDOR_URL,
        NameId::DESIGNER_URL,
        NameId::LICENSE_DESCRIPTION,
        NameId::LICENSE_URL,
    ];
    const UNTOUCHED: [NameId; 2] = [NameId::FAMILY_NAME, NameId::POSTSCRIPT_NAME];

    /// The platform/encoding/language combinations the fixture repeats every
    /// name ID on, as a real font does.
    const PLATFORMS: [(u16, u16, u16); 2] = [(1, 0, 0), (3, 1, 0x409)];

    /// A font whose only table is `name`: enough for the update to run (the
    /// other tables are copied through untouched) without needing a fixture.
    fn fixture_font() -> Vec<u8> {
        // The name table's records have to be sorted by
        // (platform, encoding, language, name ID) or `write-fonts` refuses to
        // build the table.
        let mut name_ids: Vec<u16> = UPDATABLE
            .into_iter()
            .chain(UNTOUCHED)
            .map(NameId::to_u16)
            .collect();
        name_ids.sort_unstable();
        let mut records = vec![];
        for (platform_id, encoding_id, language_id) in PLATFORMS {
            for name_id in &name_ids {
                let text = format!("old {name_id}");
                records.push(NameRecord::new(
                    platform_id,
                    encoding_id,
                    language_id,
                    NameId::new(*name_id),
                    text.into(),
                ));
            }
        }
        let mut builder = FontBuilder::new();
        builder
            .add_table(&Name::new(records))
            .expect("Failed to build the fixture name table");
        builder.build()
    }

    /// The name records of a font, as `(platform, encoding, language, id, string)`.
    fn records_of(data: &[u8]) -> Vec<(u16, u16, u16, u16, String)> {
        let font = FontRef::new(data).expect("Failed to parse font");
        let string_data = font.name().expect("No name table").string_data();
        font.name()
            .unwrap()
            .name_record()
            .iter()
            .map(|record| {
                (
                    record.platform_id(),
                    record.encoding_id(),
                    record.language_id(),
                    record.name_id().to_u16(),
                    record
                        .string(string_data)
                        .expect("Failed to read a name string")
                        .to_string(),
                )
            })
            .collect()
    }

    /// Write a fixture font into a temporary file and return the directory
    /// (kept alive by the caller) and the font's path.
    fn fixture_in_tempdir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("Failed to create a temp dir");
        let path = dir.path().join("Test-Regular.ttf");
        std::fs::write(&path, fixture_font()).expect("Failed to write the fixture");
        (dir, path)
    }

    fn parse_args(args: &[&str]) -> Args {
        let mut full = vec!["gftools-update-nameids"];
        full.extend_from_slice(args);
        Args::parse_from(full)
    }

    #[test]
    fn test_replacement_map() {
        let args = parse_args(&["-c", "Copyright 2026", "-v", "2.001", "font.ttf"]);
        assert_eq!(
            args.replacement(NameId::COPYRIGHT_NOTICE),
            Some("Copyright 2026")
        );
        assert_eq!(args.replacement(NameId::VERSION_STRING), Some("2.001"));
        // Not supplied, and not a name ID the script knows about at all.
        assert_eq!(args.replacement(NameId::DESIGNER), None);
        assert_eq!(args.replacement(NameId::FAMILY_NAME), None);
        assert_eq!(args.replacement(NameId::POSTSCRIPT_NAME), None);
    }

    #[test]
    fn test_empty_string_is_ignored() {
        let args = parse_args(&["-c", "", "font.ttf"]);
        assert_eq!(args.replacement(NameId::COPYRIGHT_NOTICE), None);
    }

    #[test]
    fn test_every_option_maps_to_its_name_id() {
        let args = parse_args(&[
            "-c",
            "0",
            "-u",
            "3",
            "-v",
            "5",
            "-t",
            "7",
            "-m",
            "8",
            "-d",
            "9",
            "--description",
            "10",
            "--urlvendor",
            "11",
            "--urldesigner",
            "12",
            "-l",
            "13",
            "--urllicense",
            "14",
            "font.ttf",
        ]);
        for name_id in UPDATABLE {
            assert_eq!(
                args.replacement(name_id),
                Some(name_id.to_u16().to_string().as_str()),
                "name ID {name_id} is not wired up"
            );
        }
    }

    #[test]
    fn test_long_option_aliases() {
        // Python accepts `-desc`, `-uv`, `-ud` and `-ul`; those cannot be clap
        // short flags, so they are long aliases.
        let args = parse_args(&[
            "--desc", "d", "--uv", "v", "--ud", "ud", "--ul", "ul", "f.ttf",
        ]);
        assert_eq!(args.description.as_deref(), Some("d"));
        assert_eq!(args.urlvendor.as_deref(), Some("v"));
        assert_eq!(args.urldesigner.as_deref(), Some("ud"));
        assert_eq!(args.urllicense.as_deref(), Some("ul"));
    }

    #[test]
    fn test_updates_only_the_requested_records() {
        let (_dir, path) = fixture_in_tempdir();
        let args = parse_args(&["-v", "Version 2.001", "-c", "New copyright", "unused.ttf"]);
        let out_path = update_font(&path, &args).expect("update_font failed");

        let records = records_of(&std::fs::read(&out_path).unwrap());
        assert_eq!(
            records.len(),
            PLATFORMS.len() * (UPDATABLE.len() + UNTOUCHED.len())
        );
        for (platform_id, encoding_id, language_id, name_id, string) in records {
            let expected = if name_id == NameId::VERSION_STRING.to_u16() {
                "Version 2.001".to_string()
            } else if name_id == NameId::COPYRIGHT_NOTICE.to_u16() {
                "New copyright".to_string()
            } else {
                format!("old {name_id}")
            };
            assert_eq!(
                string, expected,
                "wrong string for name ID {name_id} on ({platform_id}, {encoding_id}, {language_id})"
            );
        }
    }

    #[test]
    fn test_writes_a_fix_file_and_leaves_the_input_alone() {
        let (_dir, path) = fixture_in_tempdir();
        let before = std::fs::read(&path).unwrap();
        let args = parse_args(&["-v", "Version 2.001", "unused.ttf"]);

        let out_path = update_font(&path, &args).expect("update_font failed");

        assert_eq!(out_path, PathBuf::from(format!("{}.fix", path.display())));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "the input font must not be modified"
        );
        assert_ne!(std::fs::read(&out_path).unwrap(), before);
    }

    #[test]
    fn test_no_options_is_a_passthrough() {
        let (_dir, path) = fixture_in_tempdir();
        let before = std::fs::read(&path).unwrap();
        let args = parse_args(&["unused.ttf"]);

        let out_path = update_font(&path, &args).expect("update_font failed");

        assert_eq!(
            records_of(&std::fs::read(&out_path).unwrap()),
            records_of(&before)
        );
    }

    #[test]
    fn test_unencodable_mac_string_is_an_error() {
        // The fixture has platform 1 (Mac) records, and MacRoman cannot hold an
        // arrow. Python raises UnicodeEncodeError here; we error as well rather
        // than letting write-fonts panic.
        let (_dir, path) = fixture_in_tempdir();
        let args = parse_args(&["-c", "Copyright \u{2192} 2026", "unused.ttf"]);

        let err = update_font(&path, &args).expect_err("expected an error");

        let message = format!("{err:#}");
        assert!(message.contains("MacRoman"), "{message}");
        assert!(message.contains("name ID 0"), "{message}");
    }

    #[test]
    fn test_unknown_encoding_is_an_error() {
        // A record write-fonts has no encoder for at all, e.g. a Mac Japanese
        // (platform 1, encoding 1) entry.
        let name = Name::new(vec![NameRecord::new(
            1,
            1,
            0,
            NameId::VERSION_STRING,
            "Version 1.000".to_string().into(),
        )]);

        let err = check_name_table(&name).expect_err("expected an error");

        let message = format!("{err:#}");
        assert!(
            message.contains("unsupported platform/encoding"),
            "{message}"
        );
        assert!(message.contains("(1, 1)"), "{message}");
    }

    /// Compare against a real font from a google/fonts checkout: this is the
    /// part the synthetic fixture cannot cover, namely that every other table
    /// survives the rewrite.
    ///
    /// `GF_FONTS_PATH=<clone> cargo test -p gftools-bin -- --ignored`
    #[test]
    #[ignore]
    fn test_update_a_real_font() {
        let root = std::env::var("GF_FONTS_PATH")
            .unwrap_or_else(|_| "/home/simon/others-repos/fonts".to_string());
        let source = Path::new(&root).join("ofl/abeezee/ABeeZee-Regular.ttf");
        let dir = tempfile::tempdir().expect("Failed to create a temp dir");
        let path = dir.path().join("ABeeZee-Regular.ttf");
        std::fs::copy(&source, &path).expect("Failed to copy the fixture font");

        let args = parse_args(&["-v", "Version 9.999", "unused.ttf"]);
        let out_path = update_font(&path, &args).expect("update_font failed");

        let before = std::fs::read(&path).unwrap();
        let after = std::fs::read(&out_path).unwrap();
        let before_records = records_of(&before);
        let after_records = records_of(&after);
        assert_eq!(before_records.len(), after_records.len());
        for (before, after) in before_records.iter().zip(after_records.iter()) {
            assert_eq!(&before.0..=&before.3, &after.0..=&after.3);
            if before.3 == NameId::VERSION_STRING.to_u16() {
                assert_eq!(after.4, "Version 9.999");
            } else {
                assert_eq!(before.4, after.4, "name ID {} changed", before.3);
            }
        }
        // Same tables, same sizes: only the name table's strings were rewritten.
        let tables_before = skrifa::FontRef::new(&before)
            .unwrap()
            .table_directory()
            .table_records()
            .iter()
            .map(|record| record.tag())
            .collect::<Vec<_>>();
        let tables_after = skrifa::FontRef::new(&after)
            .unwrap()
            .table_directory()
            .table_records()
            .iter()
            .map(|record| record.tag())
            .collect::<Vec<_>>();
        assert_eq!(tables_before, tables_after);
    }
}
