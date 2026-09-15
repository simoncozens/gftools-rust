//! Port of `gftools.scripts.push_stats`.
//!
//! Writes an HTML report of a google/fonts repo's recent commit history, its
//! `to_sandbox.txt`/`to_production.txt` server files and its outstanding
//! issues. A `gf_repo_data.json` sidecar is written next to the report, holding
//! exactly the data the HTML embeds.

use std::path::{Path, PathBuf};

use chrono::Local;
use clap::Parser;
use gftools::GftoolsError;
use gftools_push::read_server_file;
use gftools_push::trafficjam::PushItems;
use gftools_push::utils::repo_commits;
use serde_json::json;

/// The report template, embedded so the binary needs no data files at runtime.
const TEMPLATE: &str = include_str!("../../resources/push-templates/index.html");
const JSON_PLACEHOLDER: &str = "{{ commit_data|safe }}";
const SIDECAR_NAME: &str = "gf_repo_data.json";

#[derive(Debug, Parser)]
#[command(version, about = "Generate a html report for the google/fonts repo")]
struct Args {
    /// Path to a google/fonts repo
    repo_path: PathBuf,
    /// Write the html report here
    out: PathBuf,
}

fn main() {
    let args = Args::parse();
    if let Err(error) = run(args) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), GftoolsError> {
    let data_out = args
        .out
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(SIDECAR_NAME);

    log::info!("Getting commits");
    let commits = repo_commits(&args.repo_path)?;

    log::info!("Getting server files");
    let sandbox = read_server_file(&args.repo_path, "to_sandbox.txt")?;
    let production = read_server_file(&args.repo_path, "to_production.txt")?;
    // No status or push list: the server files carry their own
    // categories.
    let sandbox = PushItems::from_server_file(&sandbox, None, None);
    let production = PushItems::from_server_file(&production, None, None);

    let data = json!({
        "last_run": Local::now().format("%Y-%m-%d").to_string(),
        "commits": commits,
        "pushes": {
            "sandbox": sandbox.0.iter().map(|item| item.to_json()).collect::<Vec<_>>(),
            "production": production.0.iter().map(|item| item.to_json()).collect::<Vec<_>>(),
        },
    });

    log::info!("Writing json data");
    std::fs::write(
        &data_out,
        serde_json::to_string_pretty(&data)
            .map_err(|e| GftoolsError::Misc(format!("Failed to serialize report data: {e}")))?,
    )?;

    log::info!("Writing report");
    let payload = serde_json::to_string(&data)
        .map_err(|e| GftoolsError::Misc(format!("Failed to serialize report data: {e}")))?;
    std::fs::write(&args.out, TEMPLATE.replace(JSON_PLACEHOLDER, &payload))?;
    Ok(())
}
