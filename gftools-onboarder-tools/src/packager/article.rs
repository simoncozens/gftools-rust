//! Port of `gftools/article.py`.
//!
//! Planned contents (PORTING_PLAN.md §4C):
//!
//! - `MAX_WIDTH` / `MAX_HEIGHT` / `MAXSIZE_VECTOR` / `MAXSIZE_RASTER`
//! - `fix_image_dimensions`, `fix_image_filesize` (via the `image` crate)
//! - `image_to_mp4` (shells out to `ffmpeg`)
//! - `update_hrefs`, `found_media`, `remove_unused_media`
//! - `fix_article(fp, out, inplace, dry_run)`
//!
//! Deviation to take: only rewrite `ARTICLE.en_us.html` when media actually changed, so
//! untouched articles keep their exact bytes (see PORTING_PLAN.md §8.8).
