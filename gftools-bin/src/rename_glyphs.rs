use anyhow::{Context, Result};
/// Rename glyph names in font_1 with font_2
use clap::Parser;
use skrifa::MetadataProvider;

#[derive(Parser, Debug)]
struct Args {
    /// Font to rewrite
    font_1: String,
    /// Font to copy glyph names from
    font_2: String,
    /// Output font file
    #[arg(short, long, default_value = "out-renamed.ttf")]
    output: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let font_1 = std::fs::read(&args.font_1)
        .with_context(|| format!("Error reading font 1 `{}`", args.font_1))?;
    let font_2 = std::fs::read(&args.font_2)
        .with_context(|| format!("Error reading font 2 `{}`", args.font_2))?;

    let output = args.output.unwrap_or("out-renamed.ttf".to_string());

    let font2_ref = skrifa::FontRef::new(&font_2)
        .with_context(|| format!("Failed to load font_2 `{}`", args.font_2))?;
    let target_names = font2_ref.glyph_names();

    let font1_ref = skrifa::FontRef::new(&font_1)
        .with_context(|| format!("Failed to load font_1 `{}`", args.font_1))?;

    let mut builder = write_fonts::FontBuilder::new();
    let source_count = font1_ref.glyph_names().num_glyphs();

    // Check both fonts have same number of glyphs
    if source_count != target_names.num_glyphs() {
        return Err(anyhow::anyhow!(
            "Font 1 has {} glyphs, but font 2 has {} glyphs. They must have the same number of glyphs.",
            source_count,
            target_names.num_glyphs()
        ));
    }

    let owned_names = target_names
        .iter()
        .map(|(_id, name)| name.to_string())
        .collect::<Vec<_>>();
    let owned_str_refs = owned_names.iter().map(|s| s.as_str()).collect::<Vec<_>>();
    let new_post = write_fonts::tables::post::Post::new_v2(owned_str_refs);
    builder
        .add_table(&new_post)
        .with_context(|| "Failed to add new post table".to_string())?;
    builder.copy_missing_tables(font1_ref);
    let output_bytes = builder.build();
    std::fs::write(&output, &output_bytes)
        .with_context(|| format!("Error writing output font `{}`", output))?;

    Ok(())
}
