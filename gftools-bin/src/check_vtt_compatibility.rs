use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use clap::Parser;
use gftools_bin::open_font;
use skrifa::{
    Font, GlyphId,
    raw::{TableProvider as _, tables::glyf::Glyph},
};
use write_fonts::types::BigEndian;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
/// Check a hinted font will successfully transfer VTT instructions to an unhinted font.
struct Args {
    /// Hinted font in OpenType (TTF/OTF) format
    #[arg(required = true)]
    hinted: PathBuf,
    /// Unhinted font in OpenType (TTF/OTF) format
    #[arg(required = true)]
    unhinted: PathBuf,
}

fn font_glyphs<'a>(f: &'a Font) -> Result<HashMap<String, Glyph<'a>>> {
    let mut glyph_map = HashMap::new();
    let loca = f.tables().loca(None)?;
    let glyf = f.tables().glyf()?;
    for gid in 0..f.num_glyphs() {
        let gid = GlyphId::new(gid);
        let name = f
            .glyph_name(gid)
            .expect("Skrifa told us this glyph was in the font");
        if let Ok(Some(glyph)) = loca.get_glyf(gid, &glyf) {
            glyph_map.insert(name.to_string(), glyph.clone());
        }
    }
    Ok(glyph_map)
}

fn report_missing(missing: &[&String], font_a: &PathBuf, font_b: &PathBuf) {
    if !missing.is_empty() {
        println!(
            "Glyphs missing in {:?} compared to {:?}: {:?}",
            font_a, font_b, missing
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum GlyphSignature {
    Simple(Vec<BigEndian<u16>>),
    Composite(Vec<GlyphId>),
}

fn signature(glyph: &Glyph) -> GlyphSignature {
    match glyph {
        Glyph::Simple(simple) => GlyphSignature::Simple(simple.end_pts_of_contours().to_vec()),
        Glyph::Composite(composite) => {
            GlyphSignature::Composite(composite.components().map(|c| c.glyph).collect())
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let hinted = open_font(&args.hinted);
    let unhinted = open_font(&args.unhinted);
    let hinted_glyphs = font_glyphs(&hinted)?;
    let unhinted_glyphs = font_glyphs(&unhinted)?;
    log::debug!("Comparing glyph counts");
    let hinted_missing = unhinted_glyphs
        .keys()
        .filter(|k| !hinted_glyphs.contains_key(*k))
        .collect::<Vec<_>>();
    let unhinted_missing = hinted_glyphs
        .keys()
        .filter(|k| !unhinted_glyphs.contains_key(*k))
        .collect::<Vec<_>>();
    report_missing(&hinted_missing, &args.hinted, &args.unhinted);
    report_missing(&unhinted_missing, &args.unhinted, &args.hinted);
    log::debug!("Check glyph structures match");
    let mut incompatible_glyphs = vec![];
    for (glyph_name, hinted_glyph) in hinted_glyphs.iter() {
        if let Some(unhinted_glyph) = unhinted_glyphs.get(glyph_name)
            && signature(hinted_glyph) != signature(unhinted_glyph)
        {
            incompatible_glyphs.push(glyph_name.clone());
        }
    }
    if !incompatible_glyphs.is_empty() {
        println!(
            "Glyphs with incompatible structures: {}",
            incompatible_glyphs.join(", ")
        );
    }
    Ok(())
}
