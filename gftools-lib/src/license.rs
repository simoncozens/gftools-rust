use chrono::Datelike;
use regex::Regex;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};
use std::sync::LazyLock;

use crate::GftoolsError;

static COPYRIGHT_YEAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]{4}").unwrap());
static COPYRIGHT_GIT_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\()(.*)(\))").unwrap());
const OFL_BODY_TEXT: &str = include_str!("../resources/OFL.txt");

/// Generate the OFL.txt text from a font
///
/// # Arguments
///
/// * `font` - A reference to the font for which to generate the OFL license.
///
/// # Returns
///
/// A `Result` containing the generated OFL license text as a `String` if successful,
/// or a `GftoolsError` if the font metadata is incomplete or invalid.
pub fn generate_ofl_license(font: &FontRef) -> Result<String, GftoolsError> {
    let Some(copyright) = font
        .localized_strings(StringId::COPYRIGHT_NOTICE)
        .english_or_first()
    else {
        return Err(GftoolsError::Misc(
            "Font doesn't contain copyright string".to_string(),
        ));
    };

    let Some(family) = font
        .localized_strings(StringId::FAMILY_NAME)
        .english_or_first()
    else {
        return Err(GftoolsError::Misc(
            "Font doesn't contain family name string".to_string(),
        ));
    };
    let copyright = copyright.to_string();
    if copyright.to_lowercase().contains("reserved font name") {
        return Err(GftoolsError::Misc(
            "Font copyright has Reserved Font Name. Please ask if it can be removed.".to_string(),
        ));
    }
    let family = family.to_string();
    let font_year = COPYRIGHT_YEAR_RE
        .captures(&copyright)
        .and_then(|cap| cap.get(0))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| chrono::Utc::now().year_ce().1.to_string());
    let Some(git_url) = COPYRIGHT_GIT_URL_RE
        .captures(&copyright)
        .and_then(|cap| cap.get(2))
        .map(|m| m.as_str().to_string())
    else {
        return Err(GftoolsError::Misc(
            "Font copyright doesn't contain a git url".to_string(),
        ));
    };

    let first_line = format!("Copyright {font_year} The {family} Project Authors ({git_url})");
    Ok(format!("{first_line}\n{OFL_BODY_TEXT}"))
}
