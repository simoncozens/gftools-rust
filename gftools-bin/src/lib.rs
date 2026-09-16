use fontspector_hotfix::{apply_hotfixes, Testable};
use gftools::GftoolsError;
use skrifa::FontRef;

pub fn fix_runner(
    font_path: &str,
    output_path: &str,
    verbosity: u8,
    check_ids: &[String],
    interactive: bool,
) -> Result<(), GftoolsError> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(match verbosity {
            0 => "warn",
            1 => "info",
            _ => "debug",
        }),
    )
    .init();
    let mut font = Testable::new(font_path)?;
    apply_hotfixes(&mut font, check_ids, interactive)
        .map_err(|_| GftoolsError::Misc("Failed to apply hotfixes".to_string()))?;
    // Save the fixed font
    std::fs::write(output_path, &font.contents)?;
    Ok(())
}
