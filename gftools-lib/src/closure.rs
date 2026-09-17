//! Utilities for closing a font's glyphset over GSUB substitutions

use crate::GftoolsError;
use skrifa::{
    raw::tables::gsub::{Gsub, SingleSubst, SubstitutionSubtables},
    GlyphId16,
};
use std::collections::{HashMap, HashSet};

pub fn close_glyphs_over_gsub(
    gsub: &Gsub,
    glyphset: &mut HashSet<GlyphId16>,
) -> Result<(), GftoolsError> {
    let mut safety_valve = 0;
    loop {
        let before_size = glyphset.len();
        for lookup in gsub.lookup_list()?.lookups().iter().flatten() {
            let subtables = lookup.subtables()?;
            match subtables {
                SubstitutionSubtables::Single(subtables) => {
                    for subtable in subtables.iter().flatten() {
                        match subtable {
                            SingleSubst::Format1(gsub1) => {
                                let delta = gsub1.delta_glyph_id();
                                for input in gsub1.coverage()?.iter() {
                                    if glyphset.contains(&input) {
                                        glyphset.insert(GlyphId16::new(
                                            input.to_u16().saturating_add_signed(delta),
                                        ));
                                    }
                                }
                            }
                            SingleSubst::Format2(gsub2) => {
                                for (input, substitute) in gsub2
                                    .coverage()?
                                    .iter()
                                    .zip(gsub2.substitute_glyph_ids().iter())
                                {
                                    if glyphset.contains(&input) {
                                        glyphset.insert(substitute.get());
                                    }
                                }
                            }
                        }
                    }
                }
                SubstitutionSubtables::Multiple(subtables) => {
                    for subtable in subtables.iter().flatten() {
                        for (input, sequence) in subtable
                            .coverage()?
                            .iter()
                            .zip(subtable.sequences().iter().flatten())
                        {
                            if glyphset.contains(&input) {
                                for substitute in sequence.substitute_glyph_ids().iter() {
                                    glyphset.insert(substitute.get());
                                }
                            }
                        }
                    }
                }
                SubstitutionSubtables::Alternate(subtables) => {
                    for subtable in subtables.iter().flatten() {
                        for (input, alternates) in subtable
                            .coverage()?
                            .iter()
                            .zip(subtable.alternate_sets().iter().flatten())
                        {
                            if glyphset.contains(&input) {
                                for substitute in alternates.alternate_glyph_ids().iter() {
                                    glyphset.insert(substitute.get());
                                }
                            }
                        }
                    }
                }
                SubstitutionSubtables::Ligature(subtables) => {
                    for subtable in subtables.iter().flatten() {
                        for (first_glyph, ligatures) in subtable
                            .coverage()?
                            .iter()
                            .zip(subtable.ligature_sets().iter().flatten())
                        {
                            if !glyphset.contains(&first_glyph) {
                                continue;
                            }
                            // If all ligature sets are in the glyphset, add their components to the glyphset
                            for ligature in ligatures.ligatures().iter().flatten() {
                                if ligature
                                    .component_glyph_ids()
                                    .iter()
                                    .map(|x| x.get())
                                    .all(|x| glyphset.contains(&x))
                                {
                                    glyphset.insert(ligature.ligature_glyph());
                                }
                            }
                        }
                    }
                }
                SubstitutionSubtables::Contextual(_) => {}
                SubstitutionSubtables::ChainContextual(_) => {}
                SubstitutionSubtables::Reverse(subtables) => {
                    for subtable in subtables.iter().flatten() {
                        if subtable.coverage()?.iter().all(|x| glyphset.contains(&x)) {
                            glyphset
                                .extend(subtable.substitute_glyph_ids().iter().map(|x| x.get()));
                        }
                    }
                }
                SubstitutionSubtables::EmptyExtension => {}
            }
        }
        safety_valve += 1;
        if safety_valve > 100 || glyphset.len() == before_size {
            break;
        }
    }
    Ok(())
}

pub fn classify_glyphs(
    predicate: impl Fn(u32) -> Vec<String>,
    charmap: &skrifa::charmap::Charmap,
    gsub: Option<&Gsub>,
) -> Result<HashMap<String, HashSet<GlyphId16>>, GftoolsError> {
    let mut classification = HashMap::new();
    let mut neutral_glyphs = HashSet::new();
    for (codepoint, glyph) in charmap.mappings() {
        let glyph = GlyphId16::new(glyph.to_u32() as u16); // This feels wrong?
        let keys = predicate(codepoint);
        if keys.is_empty() {
            neutral_glyphs.insert(glyph);
        }
        for key in keys {
            classification
                .entry(key)
                .or_insert_with(HashSet::new)
                .insert(glyph);
        }
    }
    if let Some(gsub) = gsub {
        if !neutral_glyphs.is_empty() {
            close_glyphs_over_gsub(gsub, &mut neutral_glyphs)?;
        }
        for glyphs in classification.values_mut() {
            let mut set = glyphs
                .union(&neutral_glyphs)
                .cloned()
                .collect::<HashSet<_>>();
            close_glyphs_over_gsub(gsub, &mut set)?;
            // Add everything we reached via GSUB, minus the neutrals again
            *glyphs = set
                .difference(&neutral_glyphs)
                .cloned()
                .collect::<HashSet<_>>();
        }
    }

    Ok(classification)
}

#[cfg(test)]
mod tests {
    use skrifa::{raw::TableProvider, FontRef};

    use super::*;

    #[test]
    fn test_closure() {
        let font_bytes = include_bytes!("../resources/test/Roboto[wdth,wght].ttf");
        let fontref = FontRef::new(font_bytes).unwrap();
        let gsub = fontref.gsub().unwrap();

        // Single subst
        // Start with "A"
        let mut glyphset = std::collections::HashSet::new();
        glyphset.insert(GlyphId16::new(37));
        close_glyphs_over_gsub(&gsub, &mut glyphset).unwrap();
        // A.sc via c2sc
        assert!(glyphset.contains(&GlyphId16::new(590)));

        // Ligature
        // Start with "f", "i"
        let mut glyphset = std::collections::HashSet::new();
        glyphset.insert(GlyphId16::new(74));
        glyphset.insert(GlyphId16::new(77));
        close_glyphs_over_gsub(&gsub, &mut glyphset).unwrap();
        // "fi" ligature
        assert!(glyphset.contains(&GlyphId16::new(471)));
    }
}
