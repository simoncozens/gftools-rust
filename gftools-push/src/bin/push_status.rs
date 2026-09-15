//! Port of `gftools.scripts.push_status`.
//!
//! Reports whether the families listed in a google/fonts repo's
//! `to_sandbox.txt`/`to_production.txt` have reached the server they are
//! destined for. `--lint` instead checks those files for paths which do not
//! exist in the checkout.

use std::path::{Path, PathBuf};

use clap::Parser;
use gftools::GftoolsError;
use gftools_push::config::PushConfig;
use gftools_push::items::Item;
use gftools_push::read_server_file;
use gftools_push::servers::gf_server_metadata;
use gftools_push::trafficjam::{PushItems, PushStatus};
use serde_json::Value;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Check the status of families being pushed to Google Fonts"
)]
struct Args {
    /// Path to the google/fonts repo
    path: PathBuf,
    /// Check the server files have valid paths
    #[arg(long)]
    lint: bool,
}

#[tokio::main]
async fn main() {
    // Warnings are how problems surface here - a family whose directory cannot
    // be read is otherwise reported as "N/A" - so show them by default. Raise
    // the level with `RUST_LOG`.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args = Args::parse();
    if let Err(error) = run(args).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), GftoolsError> {
    if !args.path.join("ofl").is_dir() {
        return Err(GftoolsError::Misc(format!(
            "'{}' is not a google/fonts repo",
            args.path.display()
        )));
    }

    if args.lint {
        lint_server_files(&args.path)
    } else {
        let config = PushConfig::load_default()?;
        push_report(&args.path, &config).await
    }
}

fn missing_paths_message(items: &PushItems, root: &Path) -> String {
    items
        .missing_paths(root)
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn lint_server_files(root: &Path) -> Result<(), GftoolsError> {
    const FOOTNOTE: &str = "lang and axisregistry dir paths need to be transformed.\n\
                          See https://github.com/googlefonts/gftools/issues/603";

    let production_file = PushItems::from_server_file(
        &read_server_file(root, "to_production.txt")?,
        Some(PushStatus::InSandbox),
        None,
    );
    let prod_missing = missing_paths_message(&production_file, root);

    let sandbox_file = PushItems::from_server_file(
        &read_server_file(root, "to_sandbox.txt")?,
        Some(PushStatus::InDev),
        None,
    );
    let sandbox_missing = missing_paths_message(&sandbox_file, root);

    let message = |name: &str, missing: &str| {
        format!("{}: Following paths are not valid:\n{}\n\n", name, missing)
    };

    match (prod_missing.is_empty(), sandbox_missing.is_empty()) {
        (false, false) => Err(GftoolsError::Misc(format!(
            "{}{}{FOOTNOTE}",
            message("to_production.txt", &prod_missing),
            message("to_sandbox.txt", &sandbox_missing)
        ))),
        (false, true) => Err(GftoolsError::Misc(format!(
            "{}{FOOTNOTE}",
            message("to_production.txt", &prod_missing)
        ))),
        (true, false) => Err(GftoolsError::Misc(format!(
            "{}{FOOTNOTE}",
            message("to_sandbox.txt", &sandbox_missing)
        ))),
        (true, true) => {
            println!("Server files have valid paths");
            Ok(())
        }
    }
}

/// The families listed in a server file which the server already has, as
/// `family: lastModified`, alongside those it does not have yet.
async fn server_push_status(
    root: &Path,
    filename: &str,
    url: &str,
) -> Result<(Vec<String>, Vec<String>), GftoolsError> {
    let items = PushItems::from_server_file(&read_server_file(root, filename)?, None, None);
    let family_names: Vec<String> = items
        .0
        .iter()
        .filter_map(|item| match item.item(root) {
            Some(Item::Family(family)) => Some(family.name),
            _ => None,
        })
        .collect();

    let gf_meta = gf_server_metadata(url).await?;

    let mut new_families = Vec::new();
    let mut existing_families = Vec::new();
    for name in family_names {
        if gf_meta.contains_key(&name) {
            existing_families.push(name);
        } else {
            new_families.push(name);
        }
    }

    let mut gf_families: Vec<&Value> = existing_families
        .iter()
        .filter_map(|name| gf_meta.get(name))
        .collect();
    gf_families.sort_by(|a, b| {
        a.get("lastModified")
            .and_then(|v| v.as_str())
            .cmp(&b.get("lastModified").and_then(|v| v.as_str()))
    });
    let existing_families = gf_families
        .iter()
        .map(|family| {
            format!(
                "{}: {}",
                family
                    .get("family")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
                family
                    .get("lastModified")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
            )
        })
        .collect();

    Ok((new_families, existing_families))
}

async fn server_push_report(
    name: &str,
    root: &Path,
    filename: &str,
    server_url: &str,
) -> Result<(), GftoolsError> {
    let (new_families, existing_families) = server_push_status(root, filename, server_url).await?;
    let new = if new_families.is_empty() {
        "N/A".to_string()
    } else {
        new_families.join("\n")
    };
    let existing = if existing_families.is_empty() {
        "N/A".to_string()
    } else {
        existing_families.join("\n")
    };
    println!(
        "\n***{} Status***\nNew families:\n{}\n\nExisting families, last pushed:\n{}\n",
        name, new, existing
    );
    Ok(())
}

async fn push_report(root: &Path, config: &PushConfig) -> Result<(), GftoolsError> {
    server_push_report(
        "Production",
        root,
        "to_production.txt",
        config.url("production_meta")?,
    )
    .await?;
    server_push_report(
        "Sandbox",
        root,
        "to_sandbox.txt",
        config.url("sandbox_meta")?,
    )
    .await?;
    Ok(())
}
