//! Port of `gftools.scripts.compare_meta`.
//!
//! Fetches the same document from the sandbox and the production server, munges
//! both so that only meaningful differences are left, writes them as JSON and
//! shows them side by side in `vimdiff` — optionally saving that comparison as
//! HTML, otherwise opening it and waiting for a keypress.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{ArgGroup, Parser};
use gftools::{GftoolsError, strip_json_guard};
use gftools_onboarder_tools::config::PushConfig;
use gftools_onboarder_tools::push::utils::write_json;
use serde_json::{Map, Value, json};

/// What we return from `munge_family` for a document with no family in it.
const NOT_IN_SERVER: &str = "Not in server yet";

/// The family fields `munge_meta` drops, because they change on every request
/// and would drown the diff. `subsets` is deliberately *not* here: Python has it
/// commented out of the list, so it is compared.
const VOLATILE_FAMILY_KEYS: [&str; 6] = [
    "lastModified",
    "popularity",
    "trending",
    "defaultSort",
    "size",
    "dateAdded",
];

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Compare metadata from different servers and generate a vimdiff HTML report"
)]
#[command(group(
    ArgGroup::new("diff_type")
        .required(true)
        .args(["pr", "meta", "fontv", "family", "designer"])
))]
struct Args {
    /// Path to google/fonts
    #[arg(long)]
    gf_path: Option<PathBuf>,

    /// Diff a PR
    #[arg(long)]
    pr: Option<String>,

    /// Compare font metadata
    #[arg(long)]
    meta: bool,

    /// Compare font version
    #[arg(long)]
    fontv: bool,

    /// Family to compare
    #[arg(long)]
    family: Option<String>,

    /// Designer to compare
    #[arg(long)]
    designer: Option<String>,

    /// Output path to html file
    #[arg(short, long)]
    out: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    // Warnings are how a viewer that will not launch surfaces, so show them by
    // default. Raise the level with `RUST_LOG`.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args = Args::parse();
    if let Err(error) = run(args).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), GftoolsError> {
    if let Some(path) = &args.gf_path
        && !path.is_dir()
    {
        return Err(GftoolsError::Misc(format!(
            "'{}' is not a directory",
            path.display()
        )));
    }

    if let Some(number) = &args.pr {
        return Err(GftoolsError::Misc(format!(
            "PR diff is not implemented yet (would diff PR {number})"
        )));
    }

    let config = PushConfig::load_default()?;
    let http = reqwest::Client::new();

    let (sb_meta, prod_meta) = if args.meta {
        let sb = fetch_json(&http, config.url("sandbox_meta")?).await?;
        let prod = fetch_json(&http, config.url("production_meta")?).await?;
        (munge_meta(sb)?, munge_meta(prod)?)
    } else if args.fontv {
        let sb = fetch_json_guarded(&http, config.url("sandbox_versions")?).await?;
        let prod = fetch_json_guarded(&http, config.url("production_versions")?).await?;
        (sb, prod)
    } else if let Some(family) = &args.family {
        let sb_url = family_url(config.url("sandbox_meta")?, family);
        let prod_url = family_url(config.url("production_meta")?, family);
        let sb = fetch_json_guarded(&http, &sb_url).await?;
        let prod = fetch_json_guarded(&http, &prod_url).await?;
        (munge_family(sb), munge_family(prod))
    } else if let Some(designer) = &args.designer {
        let sb = get_designer(&http, config.url("sandbox_meta")?, designer).await?;
        let prod = get_designer(&http, config.url("production_meta")?, designer).await?;
        (sb, prod)
    } else {
        // clap's `diff_type` group makes this branch unreachable.
        unreachable!("one of --pr/--meta/--fontv/--family/--designer is required")
    };

    let temp_dir = tempfile::TempDir::new()?;
    let sb_file = temp_dir.path().join("sb_meta.json");
    let prod_file = temp_dir.path().join("prod_meta.json");
    write_json(&sb_file, &sb_meta)?;
    write_json(&prod_file, &prod_meta)?;

    if let Some(out) = &args.out {
        vimdiff(&sb_file, &prod_file, out)
    } else {
        let out = temp_dir.path().join("diff.html");
        vimdiff(&sb_file, &prod_file, &out)?;
        open_file(&out);
        // The scratch directory is removed when this function returns, so wait
        // for the user like Python's `input()` does. EOF (a pipeline or no tty)
        // simply carries on, where Python raises EOFError.
        println!("Hit any key to exit");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        Ok(())
    }
}

/// A per-family metadata url. Spaces are escaped the way `servers.rs` does it,
/// because family names contain them.
fn family_url(base: &str, family: &str) -> String {
    format!("{base}/{}", family.replace(' ', "%20"))
}

async fn fetch_text(http: &reqwest::Client, url: &str) -> Result<String, GftoolsError> {
    Ok(http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

/// A plain JSON endpoint, like the family list.
async fn fetch_json(http: &reqwest::Client, url: &str) -> Result<Value, GftoolsError> {
    parse_json(&fetch_text(http, url).await?, url)
}

/// An endpoint whose body carries Google's `)]}'` XSSI guard, which Python
/// strips by slicing the first four (or five) characters off.
async fn fetch_json_guarded(http: &reqwest::Client, url: &str) -> Result<Value, GftoolsError> {
    parse_json(strip_json_guard(&fetch_text(http, url).await?), url)
}

fn parse_json(text: &str, url: &str) -> Result<Value, GftoolsError> {
    serde_json::from_str(text)
        .map_err(|e| GftoolsError::Misc(format!("Failed to parse the response from {url}: {e}")))
}

/// Python's `munge_meta`: key the family list by family name, drop the fields
/// which change on every request, and sort the axis registry by tag.
fn munge_meta(mut obj: Value) -> Result<Value, GftoolsError> {
    let families = obj
        .get("familyMetadataList")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            GftoolsError::Misc("The metadata response has no familyMetadataList".to_string())
        })?;

    let mut keyed = Map::new();
    for family in families {
        let name = family
            .get("family")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                GftoolsError::Misc("A family in the metadata response has no name".to_string())
            })?;
        let mut family = family.as_object().cloned().unwrap_or_default();
        for key in VOLATILE_FAMILY_KEYS {
            family.remove(key);
        }
        keyed.insert(name.to_string(), Value::Object(family));
    }
    obj["familyMetadataList"] = Value::Object(keyed);

    if let Some(axes) = obj.get_mut("axisRegistry").and_then(Value::as_array_mut) {
        // Python sorts by `x["tag"]` and raises if an axis has no tag; entries
        // without one sort first here instead.
        axes.sort_by(|a, b| axis_tag(a).cmp(axis_tag(b)));
    }
    Ok(obj)
}

fn axis_tag(axis: &Value) -> &str {
    axis.get("tag").and_then(Value::as_str).unwrap_or_default()
}

/// Python's `munge_family`: the coverage map becomes the list of subset names
/// it is keyed by, and the volatile fields go. Missing volatile fields are
/// ignored, where Python's `pop` would raise.
fn munge_family(mut obj: Value) -> Value {
    let Some(family) = obj.as_object_mut() else {
        return Value::String(NOT_IN_SERVER.to_string());
    };
    if !family.contains_key("family") {
        return Value::String(NOT_IN_SERVER.to_string());
    }
    // Collect before inserting, so the map is not borrowed twice.
    let subsets = family
        .get("coverage")
        .and_then(Value::as_object)
        .map(|coverage| {
            coverage
                .keys()
                .cloned()
                .map(Value::String)
                .collect::<Vec<_>>()
        });
    if let Some(subsets) = subsets {
        family.insert("coverage".to_string(), Value::Array(subsets));
    }
    for key in ["stats", "size", "lastModified"] {
        family.remove(key);
    }
    obj
}

/// Python's `get_designer`: find the first family which lists `designer`, then
/// return that designer from the family's own document.
async fn get_designer(
    http: &reqwest::Client,
    url: &str,
    designer: &str,
) -> Result<Value, GftoolsError> {
    let root = fetch_json(http, url).await?;
    let Some(family) = designer_family(&root, designer) else {
        // No family lists this designer; Python returns an empty object.
        return Ok(json!({}));
    };
    let data = fetch_json_guarded(http, &family_url(url, family)).await?;
    data.get("designers")
        .and_then(Value::as_array)
        .and_then(|designers| {
            designers
                .iter()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(designer))
        })
        .cloned()
        .ok_or_else(|| {
            GftoolsError::Misc(format!(
                "'{designer}' is listed by '{family}' but missing from its metadata"
            ))
        })
}

/// The first family in server order whose `designers` names `designer`. In the
/// family list these are plain names, not objects.
fn designer_family<'a>(root: &'a Value, designer: &str) -> Option<&'a str> {
    root.get("familyMetadataList")?
        .as_array()?
        .iter()
        .find(|family| {
            family
                .get("designers")
                .and_then(Value::as_array)
                .is_some_and(|designers| {
                    designers
                        .iter()
                        .any(|entry| entry.as_str() == Some(designer))
                })
        })?
        .get("family")?
        .as_str()
}

/// Show the two files side by side in vim and save the result as HTML.
///
/// `-n` keeps vim from writing swap files, so Python's pre-emptive delete of
/// `.*.swp` is unnecessary. Python also swallows a vim failure and carries on to
/// open a report that was never written; here it is an error.
fn vimdiff(sb_file: &Path, prod_file: &Path, out: &Path) -> Result<(), GftoolsError> {
    let status = Command::new("vim")
        .args(["-i", "NONE", "-n", "-N", "-d"])
        .arg(sb_file)
        .arg(prod_file)
        .arg("-c")
        .arg("windo set wrap")
        .arg("-c")
        .arg("TOhtml")
        .arg("-c")
        .arg(format!("sav! {}", out.display()))
        .arg("-c")
        .arg("qall!")
        .status()
        .map_err(|e| GftoolsError::Misc(format!("Failed to run vim: {e}")))?;
    if !status.success() {
        return Err(GftoolsError::Misc(format!(
            "vim exited with {status} while writing {}",
            out.display()
        )));
    }
    println!("Comparison saved to {}", out.display());
    Ok(())
}

/// Open the report in the user's viewer, as Python's `xdg-open`/`open`/`start`
/// does. A viewer which cannot be started is only a warning: the file is on
/// disk either way.
fn open_file(path: &Path) {
    let mut command = match std::env::consts::OS {
        "linux" => Command::new("xdg-open"),
        "macos" => Command::new("open"),
        // `start` is a cmd builtin, so Python's bare `["start", path]` cannot
        // work here; the empty argument is the window title.
        "windows" => {
            let mut command = Command::new("cmd");
            command.args(["/C", "start", ""]);
            command
        }
        other => {
            log::warn!("Cannot open {} on {other}", path.display());
            return;
        }
    };
    match command.arg(path).status() {
        Ok(status) if status.success() => {}
        Ok(status) => log::warn!("The viewer exited with {status}"),
        Err(e) => log::warn!("Failed to open {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_munge_meta() {
        let root = json!({
            "familyMetadataList": [
                {
                    "family": "Second",
                    "designers": ["Someone"],
                    "lastModified": "2026-01-01",
                    "popularity": 1,
                    "trending": 2,
                    "defaultSort": 3,
                    "size": 4,
                    "dateAdded": "2015-01-01",
                    "subsets": ["latin"],
                },
                {"family": "First", "designers": ["Other"]},
            ],
            "axisRegistry": [{"tag": "wght"}, {"tag": "ital"}],
            "promotedScript": ["Hant"],
        });
        let munged = munge_meta(root).unwrap();

        let families = munged["familyMetadataList"].as_object().unwrap();
        assert_eq!(families.keys().collect::<Vec<_>>(), vec!["First", "Second"]);
        let second = families["Second"].as_object().unwrap();
        // Every volatile field is gone, but `subsets` is compared (Python has
        // it commented out of the exclude list).
        assert_eq!(
            second.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["designers", "family", "subsets"]
        );

        // The axis registry is sorted by tag, and the other root keys survive.
        assert_eq!(munged["axisRegistry"][0]["tag"], "ital");
        assert_eq!(munged["promotedScript"][0], "Hant");
    }

    #[test]
    fn test_munge_family() {
        let family = json!({
            "family": "Maven Pro",
            "coverage": {"latin": "U+0-7F", "menu": "U+20"},
            "stats": {"foo": 1},
            "size": 100,
            "lastModified": "2026-01-01",
            "designers": ["Joe Prince"],
        });
        let munged = munge_family(family);
        assert_eq!(munged["coverage"], json!(["latin", "menu"]));
        let munged = munged.as_object().unwrap();
        assert!(!munged.contains_key("stats"));
        assert!(!munged.contains_key("size"));
        assert!(!munged.contains_key("lastModified"));
        assert_eq!(munged["designers"][0], "Joe Prince");

        // Python pops the volatile fields and raises if one is missing; a
        // partial document is compared as it is instead.
        let partial = munge_family(json!({"family": "Maven Pro"}));
        assert_eq!(partial, json!({"family": "Maven Pro"}));

        // A document with no family in it is replaced by a note.
        assert_eq!(munge_family(json!({"nope": 1})), json!(NOT_IN_SERVER));
    }

    #[test]
    fn test_designer_family() {
        // The family list carries designer names, and the first family which
        // names one is the one to fetch.
        let root = json!({
            "familyMetadataList": [
                {"family": "First", "designers": ["Other"]},
                {"family": "Second", "designers": ["Someone", "Other"]},
                {"family": "Third", "designers": ["Someone"]},
            ]
        });
        assert_eq!(designer_family(&root, "Someone"), Some("Second"));
        assert_eq!(designer_family(&root, "Other"), Some("First"));
        assert_eq!(designer_family(&root, "Nobody"), None);
        assert_eq!(designer_family(&json!({}), "Someone"), None);
    }

    #[test]
    fn test_write_json_uses_four_space_indent() {
        // The writer lives in `gftools_onboarder_tools::push::utils::write_json`; this pins the
        // shape `compare_meta` and `manage_traffic_jam` write.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("meta.json");
        write_json(
            &path,
            &json!({"family": "Maven Pro", "coverage": ["latin"]}),
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "{\n    \"coverage\": [\n        \"latin\"\n    ],\n    \"family\": \"Maven Pro\"\n}"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"family": "Maven Pro", "coverage": ["latin"]})
        );
    }
}
