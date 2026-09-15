#![allow(clippy::result_large_err)] // Errors are rare, who cares how big they are?
pub mod config;
pub mod items;
pub mod servers;
pub mod trafficjam;
pub mod utils;

pub use gftools::GftoolsError;
pub use std::path::Path;

pub fn read_server_file(root: &Path, name: &str) -> Result<String, GftoolsError> {
    Ok(std::fs::read_to_string(root.join(name))?)
}
