mod closure;
mod error;
mod fix;
mod names;
mod overlaps;
mod utils;

pub use fix::{FixFvarTable, IncludeSourceFixes, Interactive, fix_font};
use google_fonts_languages::LANGUAGES;
pub use overlaps::remove_overlaps;
use skrifa::raw::TableProvider;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};
use std::{fmt::Display, path::Path};
use unicode_script::UnicodeScript as _;

pub use error::GftoolsError;
pub use names::{AxisLimits, AxisTriple, update_name_table};
pub use utils::{
    PROD_FAMILY_DOWNLOAD, download_family_from_google_fonts, is_google_fonts_repo, strip_json_guard,
};
// Have to make these pub so our scripts can use them
pub use gf_metadata::{AxisProto, DesignerInfoProto, FamilyProto};
use tabled::settings::Style;

pub fn parse_pb<T>(path: &Path) -> Result<T, GftoolsError>
where
    T: protobuf::MessageFull,
{
    let contents = std::fs::read(path)?;
    let utf8_contents = std::str::from_utf8(&contents)
        .map_err(|_| GftoolsError::Misc("METADATA.pb is not valid UTF-8".to_string()))?;
    let data = protobuf::text_format::parse_from_str::<T>(utf8_contents)
        .map_err(GftoolsError::ProtobufParse)?;
    Ok(data)
}

pub fn write_family_metadata(data: &FamilyProto, comments: bool) -> Result<String, GftoolsError> {
    // Rust's print_to_string doesn't handle comments. We'll do it as a post-processing step.
    let s = protobuf::text_format::print_to_string_pretty(data);
    if !comments {
        return Ok(s);
    }
    let mut rewritten = String::new();
    for line in s.lines() {
        rewritten += line;
        if line.starts_with("languages: ")
            && let Some(lang) = line.split('"').nth(1)
            && let Some(lang) = LANGUAGES.get(lang)
        {
            rewritten += &format!("  # {}", lang.name());
        }
        rewritten.push('\n');
    }
    Ok(rewritten)
}

pub fn list_some_things<T: Display>(
    font_files: &[String],
    lister: impl Fn(&str, &skrifa::FontRef) -> Option<Vec<T>>,
    headers: &[&str],
    csv: bool,
) {
    let mut info: Vec<Vec<String>> = Vec::new();
    for font in font_files.iter() {
        let Ok(font_data) = std::fs::read(font) else {
            log::warn!("{}: Failed to read font file, skipping", font);
            continue;
        };
        let Ok(fontref) = skrifa::FontRef::new(&font_data) else {
            log::warn!("{}: Failed to parse font file, skipping", font);
            continue;
        };
        if let Some(result) = lister(font, &fontref) {
            info.push(
                std::iter::once(font.to_string())
                    .chain(result.into_iter().map(|x| x.to_string()))
                    .collect::<Vec<String>>(),
            );
        } // list should do its own error reporting
    }
    if csv {
        println!("font,{}", headers.join(","));
        for row in info {
            println!("{}", row.join(","));
        }
    } else {
        let mut builder = tabled::builder::Builder::default();
        builder.push_record(headers.iter().map(|s| s.to_string()));
        for row in info {
            builder.push_record(row);
        }
        let mut table = builder.build();
        table.with(Style::sharp());
        println!("{}", table);
    }
}

/// Returns the version string of the given font.
pub fn font_version(f: &FontRef) -> String {
    if let Some(version) = f
        .localized_strings(StringId::VERSION_STRING)
        .english_or_first()
        .map(|name| name.chars().collect())
    {
        version
    } else {
        f.head()
            .map(|head| head.font_revision().to_string())
            .unwrap_or_else(|_| "0.0".to_string())
    }
}

pub fn primary_script(fontref: &FontRef, ignore_latin: bool) -> Option<String> {
    let classification = closure::classify_glyphs(
        |cp| {
            let Some(c) = char::from_u32(cp) else {
                return vec![];
            };
            let mut scripts = vec![c.script().short_name().to_string()];
            scripts.extend(
                c.script_extension()
                    .iter()
                    .map(|s| s.short_name().to_string()),
            );
            scripts
        },
        &fontref.charmap(),
        fontref.gsub().ok().as_ref(),
    )
    .ok()?;
    let mut badkeys = vec!["Zinh", "Zyyy", "Zzzz"];
    if ignore_latin {
        badkeys.push("Latn");
    }
    let mut script_counts = classification
        .iter()
        .filter(|(script, _)| !badkeys.contains(&script.as_str()))
        .map(|(script, glyphs)| (script.clone(), glyphs.len()))
        .collect::<Vec<(String, usize)>>();
    script_counts.sort_by_key(|b| std::cmp::Reverse(b.1));
    // If there isn't a clear winner, give up.
    if script_counts.len() > 2 && script_counts[0].1 < 2 * script_counts[1].1 {
        return None;
    }
    script_counts.first().map(|(script, _)| script.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_roundtrip_proto() {
        let pushster_string =
            std::fs::read_to_string("resources/test/pushster/METADATA.pb").unwrap();
        let pushster =
            parse_pb::<FamilyProto>(Path::new("resources/test/pushster/METADATA.pb")).unwrap();
        let serialized = write_family_metadata(&pushster, true).unwrap();
        assert_eq!(pushster_string, serialized);
    }
}
