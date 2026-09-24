//! Print out copyright nameIDs strings.
//!
//! ```sh
//! gftools-list-copyright-notices --csv ~/fonts/*/*/*.ttf > ~/notices.txt
//! ```
//!
//! The Python original is called `check_copyright_notices`, but it only lists
//! the strings, so it is renamed here (as `check_name` became `list_name`) and
//! the `--csv` option writes the same bytes it did: pipe-delimited records with
//! CRLF terminators, since copyright notices are full of commas.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;
use skrifa::raw::TableProvider as _;
use skrifa::string::StringId;
use tabled::Tabled;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
/// Print out copyright notice nameIDs strings
struct Args {
    /// Fonts in OpenType (TTF/OTF) format
    #[arg(required = true)]
    fonts: Vec<PathBuf>,
    /// Output data in comma-separate-values (CSV) file format
    #[clap(long)]
    csv: bool,
}

#[derive(Tabled, Serialize)]
struct CopyrightNotice {
    #[tabled(rename = "filename")]
    filename: String,
    #[tabled(rename = "copyright notice")]
    copyright_notice: String,
    #[tabled(rename = "char length")]
    char_length: usize,
    #[tabled(rename = "platformID")]
    platform_id: String,
}

/// The CSV header, which the Python passes to `csv.writer` separately from the
/// rows.
const HEADERS: [&str; 4] = ["filename", "copyright notice", "char length", "platformID"];

fn main() -> Result<()> {
    let args = Args::parse();
    let mut rows = Vec::new();
    for file in &args.fonts {
        rows.extend(notice_rows(file)?);
    }
    if args.csv {
        print!("{}", to_csv(&rows)?);
    } else {
        let table = tabled::Table::new(&rows);
        println!("{table}");
    }
    Ok(())
}

/// The copyright notice name records of a single font, in name table order.
fn notice_rows(file: &Path) -> Result<Vec<CopyrightNotice>> {
    let font_data = std::fs::read(file)
        .with_context(|| format!("Failed to read font file: {}", file.display()))?;
    let font = skrifa::FontRef::new(&font_data)
        .with_context(|| format!("Failed to parse font file: {}", file.display()))?;
    let name_table = font.name()?;
    let string_data = name_table.string_data();
    let filename = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut rows = Vec::new();
    for record in name_table.name_record() {
        if record.name_id() != StringId::COPYRIGHT_NOTICE {
            continue;
        }
        let notice = record.string(string_data)?.to_string();
        let platform_id = record.platform_id();
        rows.push(CopyrightNotice {
            char_length: notice.chars().count(),
            filename: filename.clone(),
            copyright_notice: notice,
            platform_id: format!("{platform_id} ({})", platform_id_str(platform_id)),
        });
    }
    Ok(rows)
}

/// Write the rows as the Python's `csv.writer` did: `|` delimited, CRLF
/// terminated, quoting only where necessary.
fn to_csv(rows: &[CopyrightNotice]) -> Result<String> {
    let mut writer = csv::WriterBuilder::new()
        .delimiter(b'|')
        .terminator(csv::Terminator::CRLF)
        .has_headers(false)
        .from_writer(Vec::new());
    writer.write_record(HEADERS)?;
    for row in rows {
        writer.serialize(row)?;
    }
    let bytes = writer.into_inner()?;
    Ok(String::from_utf8(bytes)?)
}

/// The platform ID names from `gftools.constants.PLATID_STR`.
fn platform_id_str(platform_id: u16) -> &'static str {
    match platform_id {
        0 => "UNICODE",
        1 => "MACINTOSH",
        2 => "ISO",
        3 => "WINDOWS",
        4 => "CUSTOM",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROBOTO: &str = "../gftools-lib/resources/test/Roboto[wdth,wght].ttf";
    const MAVEN_PRO_NOTICE: &str = "Copyright 2011 The Maven Pro Project Authors \
        (https://github.com/m4rc1e/mavenproFont), with Reserved Font Name \"Maven Pro\".";

    #[test]
    fn test_notice_rows() {
        let rows = notice_rows(Path::new(ROBOTO)).unwrap();
        // Roboto only has one copyright notice, on the Windows platform.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].filename, "Roboto[wdth,wght].ttf");
        assert_eq!(
            rows[0].copyright_notice,
            "Copyright 2011 The Roboto Project Authors \
             (https://github.com/googlefonts/roboto-classic)"
        );
        assert_eq!(rows[0].char_length, 89);
        assert_eq!(rows[0].platform_id, "3 (WINDOWS)");
    }

    #[test]
    fn test_csv_output() {
        let rows = notice_rows(Path::new(ROBOTO)).unwrap();
        assert_eq!(
            to_csv(&rows).unwrap(),
            "filename|copyright notice|char length|platformID\r\n\
             Roboto[wdth,wght].ttf|\
             Copyright 2011 The Roboto Project Authors \
             (https://github.com/googlefonts/roboto-classic)|89|3 (WINDOWS)\r\n"
        );
    }

    #[test]
    fn test_csv_quoting() {
        // Commas and quotes in the notice, as in the Python's csv output.
        let rows = vec![CopyrightNotice {
            filename: "MavenPro[wght].ttf".to_string(),
            copyright_notice: MAVEN_PRO_NOTICE.to_string(),
            char_length: MAVEN_PRO_NOTICE.chars().count(),
            platform_id: "3 (WINDOWS)".to_string(),
        }];
        assert_eq!(
            to_csv(&rows).unwrap(),
            "filename|copyright notice|char length|platformID\r\n\
             MavenPro[wght].ttf|\
             \"Copyright 2011 The Maven Pro Project Authors \
             (https://github.com/m4rc1e/mavenproFont), with Reserved Font Name \
             \"\"Maven Pro\"\".\"|123|3 (WINDOWS)\r\n"
        );
    }

    #[test]
    fn test_csv_without_rows() {
        // The header is still written, as the Python does.
        assert_eq!(
            to_csv(&[]).unwrap(),
            "filename|copyright notice|char length|platformID\r\n"
        );
    }

    #[test]
    fn test_platform_id_str() {
        assert_eq!(platform_id_str(0), "UNICODE");
        assert_eq!(platform_id_str(1), "MACINTOSH");
        assert_eq!(platform_id_str(2), "ISO");
        assert_eq!(platform_id_str(3), "WINDOWS");
        assert_eq!(platform_id_str(4), "CUSTOM");
        assert_eq!(platform_id_str(5), "?");
    }
}
