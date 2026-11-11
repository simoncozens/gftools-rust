use anyhow::Result;
use std::{fmt::Display, path::Path};

use fontspector_hotfix::Testable;
use gftools::GftoolsError;
use tabled::settings::Style;

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
    if interactive {
        fontspector_hotfix::apply_hotfixes_interactive(&mut font, check_ids)
            .map_err(|_| GftoolsError::Misc("Failed to apply hotfixes".to_string()))?;
    } else {
        fontspector_hotfix::apply_hotfixes(&mut font, check_ids)
            .map_err(|_| GftoolsError::Misc("Failed to apply hotfixes".to_string()))?;
    }
    // Save the fixed font
    std::fs::write(output_path, &font.contents)?;
    Ok(())
}

pub fn open_font(font_path: &Path) -> skrifa::Font {
    let backing = std::fs::read(font_path).unwrap_or_else(|e| {
        eprintln!("{}: {e:?}", font_path.display());
        std::process::exit(1);
    });
    skrifa::Font::new(backing, 0).unwrap_or_else(|| {
        eprintln!("{}: Failed to open font", font_path.display());
        std::process::exit(1);
    })
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
