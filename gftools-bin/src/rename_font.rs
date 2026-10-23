//! Rename a font.
//!
//! Changes font menu name and filename. The user can also specify their own
//! output path.
//!
//! ```sh
//! gftools-rename-font font.ttf "New Family Name"
//! ```

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use gftools::font_familyname;
use google_fonts_axisregistry::{RenameAggressiveness, build_name_table};
use skrifa::FontRef;
use std::path::PathBuf;

/// Rename a font
#[derive(Debug, Parser)]
struct Args {
    /// Font file to rename
    font: PathBuf,

    /// New family name
    new_name: String,

    /// Output path
    #[arg(short, long)]
    out: Option<PathBuf>,

    /// New name is added to old name
    #[arg(long)]
    suffix: bool,

    /// New font is written on old filename
    #[arg(long)]
    overwrite: bool,

    /// Only change family name and names based off it, such as the PostScript
    /// name. (By default, the old family name is replaced by the new name in
    /// all name table entries, including copyright, description, etc.)
    #[arg(long)]
    just_family: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let data = std::fs::read(&args.font)
        .with_context(|| format!("Error reading font `{}`", args.font.display()))?;
    let font = FontRef::new(&data)
        .with_context(|| format!("Failed to parse font `{}`", args.font.display()))?;

    let current_name = font_familyname(&font).ok_or_else(|| {
        anyhow!(
            "Name table does not contain nameID 1 or nameID 16. \
             This tool does not work on webfonts."
        )
    })?;
    let new_name = if args.suffix {
        format!("{current_name}{}", args.new_name)
    } else {
        args.new_name.clone()
    };

    let aggressiveness = if args.just_family {
        RenameAggressiveness::Conservative
    } else {
        RenameAggressiveness::Aggressive
    };
    let renamed = build_name_table(&data, Some(&new_name), None, &[], Some(aggressiveness))
        .map_err(|e| anyhow!("Failed to rebuild name table: {e}"))?;

    let out = if let Some(out) = args.out {
        out
    } else if args.overwrite {
        args.font.clone()
    } else {
        // Like the Python original, substitute in the whole path string.
        PathBuf::from(
            args.font
                .to_string_lossy()
                .replace(&current_name.replace(' ', ""), &new_name.replace(' ', "")),
        )
    };
    println!("Saving font: {}", out.display());
    std::fs::write(&out, renamed).with_context(|| format!("Error writing `{}`", out.display()))?;
    Ok(())
}
