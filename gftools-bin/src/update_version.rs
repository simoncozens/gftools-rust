//! Update a collection of fonts version number to a new version number.
//!
//! gftools-update-version [fonts] 2.300 2.301

use anyhow::{Context, Result};
use clap::Parser;
use skrifa::{FontRef, raw::TableProvider as _};
use std::path::PathBuf;
use write_fonts::{
    FontBuilder,
    from_obj::ToOwnedTable,
    tables::{head::Head, name::Name},
    types::Fixed,
};

#[derive(Parser, Debug)]
struct Args {
    /// The paths to the font files to update.
    font_paths: Vec<PathBuf>,
    /// Old version string to replace in the font's metadata.
    old_version: String,
    /// The new version string to set in the font's metadata.
    new_version: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let new_revision: f64 = args
        .new_version
        .parse()
        .with_context(|| format!("Failed to parse new version string: {}", args.new_version))?;
    for font in args.font_paths {
        let font_contents = std::fs::read(&font)
            .with_context(|| format!("Failed to read font file: {:?}", font))?;
        let font_ref = FontRef::new(&font_contents)
            .with_context(|| format!("Failed to parse font file: {:?}", font))?;
        let mut name: Name = font_ref.name()?.to_owned_table();
        for name in name.name_record.iter_mut() {
            name.string = name
                .string
                .to_string()
                .replace(&args.old_version, &args.new_version)
                .into();
        }
        let mut head: Head = font_ref.head()?.to_owned_table();
        head.font_revision = Fixed::from_f64(new_revision);
        let mut builder = FontBuilder::new();
        builder
            .add_table(&head)
            .with_context(|| "Failed to build head table".to_string())?;
        builder
            .add_table(&name)
            .with_context(|| "Failed to build name table".to_string())?;
        builder.copy_missing_tables(font_ref);
        let new_font_data = builder.build();
        std::fs::write(&font, new_font_data)
            .with_context(|| format!("Failed to write updated font file: {:?}", font))?;
    }

    Ok(())
}
