#![allow(clippy::result_large_err)] // Errors are rare, who cares how big they are?
//! Onboarding tools for Google Fonts.
//!
//! One crate for the tools that add files to the Google Fonts system: the push
//! tools ([`push`], `gftools-push-status`, `gftools-push-stats`,
//! `gftools-compare-meta`, `gftools-gen-push-lists`,
//! `gftools-manage-traffic-jam`) and the packager ([`packager`],
//! `gftools-packager`, `gftools-add-font`).
//!
//! They share [`config`] (the `~/.gf_push_config.toml` file) and [`github`]
//! (the authenticated octocrab client and the Traffic Jam board mutations).
//! General font handling lives in `gftools-lib` instead.
//!
//! The packager half is still being ported: see `PORTING_PLAN.md`.

pub mod config;
pub mod github;
pub mod packager;
pub mod push;

pub use gftools::GftoolsError;
pub use std::path::Path;

pub fn read_server_file(root: &Path, name: &str) -> Result<String, GftoolsError> {
    Ok(std::fs::read_to_string(root.join(name))?)
}
