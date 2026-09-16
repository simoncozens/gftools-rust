//! Port of `gftools/packager/__init__.py`.
//!
//! Planned contents (PORTING_PLAN.md §4D), in four groups:
//!
//! - metadata: `create_metadata`, `expected_source`, `append_source_template`,
//!   `no_source_metadata`, `incomplete_source_metadata`, `load_metadata`
//!   (incl. legacy `upstream.yaml` merge), `save_metadata`
//! - assets: `get_family_dir`, `find_family_in_repo`, `download_assets`,
//!   `assets_are_same`, `package_family`
//! - git: `git_branch_name`, `create_git_branch`, `commit_family`, `push_family`,
//!   `current_git_state`
//! - PR: `pr_family`, `make_package` (returns an outcome enum rather than Python's bare
//!   `return` on the placeholder paths)

pub mod addfont;
pub mod article;
pub mod fonts;
pub mod source;
