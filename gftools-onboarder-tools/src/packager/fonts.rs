//! Port of the used subset of `gftools.util.google_fonts` / `gftools.util.styles`.
//!
//! Planned contents (PORTING_PLAN.md §4A):
//!
//! - `read_proto` / `write_metadata` (text-proto I/O, incl. the `  # Language name`
//!   comment injection that `WriteProto` performs line-wise)
//! - `language_comments` (from the `google-fonts-languages` data)
//! - `extract_name` / `extract_names`
//! - `family_style_weight` (`FileFamilyStyleWeight` / `VFFamilyStyleWeight`),
//!   `weight`, `vf_weight`, `style`, `family_name`
//! - `license_from_path`
