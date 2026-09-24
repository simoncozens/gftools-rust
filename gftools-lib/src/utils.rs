use std::collections::BTreeMap;

use skrifa::raw::TableProvider as _;
use skrifa::string::StringId;

use crate::error::GftoolsError;

pub const PROD_FAMILY_DOWNLOAD: &str = "https://fonts.google.com/download?family={FAMILY}";

/// Remove Google's `)]}'` XSSI guard, which prefixes some endpoint responses.
pub fn strip_json_guard(text: &str) -> &str {
    text.strip_prefix(")]}'")
        .unwrap_or(text)
        .trim_start_matches('\n')
}

/// Download a family from a Google Fonts server, returning the font files keyed
/// by filename. Nothing is written to disk.
pub async fn download_family_from_google_fonts(
    family: &str,
    dl_url_override: Option<&str>,
    ignore_static: bool,
) -> Result<BTreeMap<String, Vec<u8>>, GftoolsError> {
    let server_url = dl_url_override
        .unwrap_or(PROD_FAMILY_DOWNLOAD)
        .replace("download?family=", "download/list?family=");
    // `PROD_FAMILY_DOWNLOAD` uses `{FAMILY}`, but the download urls in
    // `~/.gf_push_config.*` are Python format strings and use `{}`.
    let family = family.replace(" ", "%20");
    let request_url = server_url
        .replace("{FAMILY}", &family)
        .replace("{}", &family);

    let client = reqwest::Client::new();
    let text = client
        .get(&request_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let manifest: serde_json::Value = serde_json::from_str(strip_json_guard(&text))
        .map_err(|e| GftoolsError::Misc(format!("Failed to parse metadata: {}", e)))?;
    let mut fonts = BTreeMap::new();
    for file in manifest
        .as_object()
        .and_then(|x| x.get("manifest"))
        .and_then(|x| x.as_object())
        .and_then(|x| x.get("fileRefs"))
        .and_then(|x| x.as_array())
        .ok_or(GftoolsError::Misc(format!(
            "Failed to find fileRefs in manifest: {:?}",
            manifest
        )))?
    {
        let url = file
            .as_object()
            .and_then(|x| x.get("url"))
            .and_then(|x| x.as_str())
            .ok_or(GftoolsError::Misc("Failed to find url in file".to_string()))?;
        let filename = file
            .as_object()
            .and_then(|x| x.get("filename"))
            .and_then(|x| x.as_str())
            .ok_or(GftoolsError::Misc(
                "Failed to filename url in file".to_string(),
            ))?;
        if (ignore_static && filename.contains("static"))
            || !filename.ends_with("otf") && !filename.ends_with("ttf")
        {
            continue;
        }
        let contents = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        fonts.insert(filename.to_string(), contents.to_vec());
    }
    Ok(fonts)
}

/// Checks if the given path is a Google Fonts repository by verifying the presence of the `ofl` directory.
pub fn is_google_fonts_repo(path: &std::path::Path) -> bool {
    path.join("ofl").is_dir()
}

/// Port of `gftools.utils.font_is_italic`: does the font's style name contain
/// "Italic"?
///
/// The Python reads name ID 2 for platform 3, encoding 1, language 0x409, and
/// raises if there is no such record; here a missing or undecodable record is
/// reported as "not italic".
pub fn font_is_italic(font: &skrifa::FontRef) -> bool {
    let Ok(name_table) = font.name() else {
        return false;
    };
    let string_data = name_table.string_data();
    name_table
        .name_record()
        .iter()
        .find(|record| {
            record.name_id() == StringId::SUBFAMILY_NAME
                && record.platform_id() == 3
                && record.encoding_id() == 1
                && record.language_id() == 0x409
        })
        .and_then(|record| record.string(string_data).ok())
        .map(|name| name.to_string().contains("Italic"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::FontRef;
    use write_fonts::{
        FontBuilder,
        tables::{
            maxp::Maxp,
            name::{Name, NameRecord},
        },
        types::NameId,
    };

    /// A minimal font whose subfamily name is `subfamily`, so that the
    /// positive case can be tested without a fixture.
    fn font_with_subfamily(subfamily: &str) -> Vec<u8> {
        let records = vec![NameRecord::new(
            3,
            1,
            0x409,
            NameId::SUBFAMILY_NAME,
            subfamily.to_string().into(),
        )];
        let mut builder = FontBuilder::new();
        builder.add_table(&Maxp::default()).unwrap();
        builder.add_table(&Name::new(records)).unwrap();
        builder.build()
    }

    #[test]
    fn test_font_is_italic() {
        let italic = font_with_subfamily("Italic");
        assert!(font_is_italic(&FontRef::new(&italic).unwrap()));
        let bold_italic = font_with_subfamily("Bold Italic");
        assert!(font_is_italic(&FontRef::new(&bold_italic).unwrap()));
        let regular = font_with_subfamily("Regular");
        assert!(!font_is_italic(&FontRef::new(&regular).unwrap()));
        let roboto = std::fs::read("resources/test/Roboto[wdth,wght].ttf").unwrap();
        assert!(!font_is_italic(&FontRef::new(&roboto).unwrap()));
    }
}
