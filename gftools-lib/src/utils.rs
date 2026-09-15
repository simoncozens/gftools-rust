use std::collections::BTreeMap;

use crate::error::GftoolsError;

pub const PROD_FAMILY_DOWNLOAD: &str = "https://fonts.google.com/download?family={FAMILY}";

#[allow(dead_code)] // We'll use it one day
pub fn download_family_from_google_fonts(
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

    let request = reqwest::blocking::Client::new().get(request_url);
    let manifest: serde_json::Value = request
        .send()
        .and_then(|response| response.text())
        .map_or_else(
            |e| {
                Err(GftoolsError::Misc(format!(
                    "Failed to fetch metadata: {}",
                    e
                )))
            },
            |s| {
                serde_json::from_str(&s[5..])
                    .map_err(|e| GftoolsError::Misc(format!("Failed to parse metadata: {}", e)))
            },
        )?;
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
        let contents = reqwest::blocking::get(url)
            .map_err(|e| GftoolsError::Misc(format!("Failed to fetch font: {}", e)))?
            .bytes()
            .map_err(|e| GftoolsError::Misc(format!("Failed to fetch font: {}", e)))?;
        fonts.insert(filename.to_string(), contents.to_vec());
    }
    Ok(fonts)
}

/// Checks if the given path is a Google Fonts repository by verifying the presence of the `ofl` directory.
pub fn is_google_fonts_repo(path: &std::path::Path) -> bool {
    path.join("ofl").is_dir()
}
