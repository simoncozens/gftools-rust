//! Port of `gftools/scripts/add_font.py` as a library (thin binary in `src/bin/add_font.rs`).
//!
//! Planned contents (PORTING_PLAN.md §4B):
//!
//! - `RELAXED_SUBSETS`, the axis-registry singleton
//! - `file_family_style_weights`
//! - `make_metadata` (new + carry-over-from-existing paths)
//! - `registry_overrides`, `get_avg_size`, `write_text_file`
//! - `add_font(directory, args)`: writes `METADATA.pb` and creates
//!   `article/ARTICLE.en_us.html` when neither a DESCRIPTION nor an article exists.
//!
//! `packager::save_metadata` calls `add_font` — this is shared code, not just a script.
