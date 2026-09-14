use anyhow::{Context, Result};
/// Rename glyph names in font_1 with font_2
use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    /// Origin font file
    origin: String,
    /// Fonts to rewrite
    fonts: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let origin = norad::Font::load(&args.origin)
        .with_context(|| format!("Failed to load origin font: {}", args.origin))?;

    let origin_order = origin
        .lib
        .get("public.glyphOrder")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Origin font does not have a valid public.glyphOrder"))?;

    for font_path in args.fonts {
        let mut font = norad::Font::load(&font_path)
            .with_context(|| format!("Failed to load font: {}", font_path))?;

        font.lib
            .insert("public.glyphOrder".to_string(), origin_order.clone().into());

        font.save(&font_path)
            .with_context(|| format!("Failed to save font: {}", font_path))?;
    }

    Ok(())
}
