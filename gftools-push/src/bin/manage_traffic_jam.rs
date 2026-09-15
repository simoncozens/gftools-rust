//! Port of `gftools.scripts.manage_traffic_jam`.
//!
//! An interactive release-queue manager. For every Traffic Jam item which is
//! merged and not live, it checks out the pull request, works out which server
//! the item has reached and records that status on the board, shows what the
//! local checkout and each server hold, then asks what to do with the item.
//!
//! Needs `GH_TOKEN`, the GitHub CLI (for `gh pr checkout`) and `vimdiff` (for
//! the inspect action).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::{Parser, ValueEnum};
use gftools::GftoolsError;
use gftools_push::config::PushConfig;
use gftools_push::items::Item;
use gftools_push::servers::GfServers;
use gftools_push::trafficjam::{PushCategory, PushItem, PushItems, PushStatus};
use gftools_push::utils::{branch_matches_googlefonts_main, write_json};
use serde_json::{Map, Value, json};

/// Python hardcodes the production site; only the dev and sandbox hosts come
/// from the config (or their environment variables).
const PRODUCTION_URL: &str = "https://fonts.google.com";
/// Where the server and board caches live, as in Python.
const SERVER_DATA: &str = ".gf_server_data.json";
const TRAFFIC_JAM_DATA: &str = ".gf_traffic_jam_data.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Filter {
    #[value(name = "lists")]
    Lists,
    #[value(name = "in_sandbox")]
    InSandbox,
    /// Python's `--filter` choices omit this one, although the filter itself is
    /// implemented, so `-f in_dev` is a parser error there.
    #[value(name = "in_dev")]
    InDev,
    #[value(name = "upgrade")]
    Upgrade,
    #[value(name = "new")]
    New,
    #[value(name = "no_fonts")]
    NoFonts,
    #[value(name = "fonts")]
    Fonts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum LogLevel {
    #[value(name = "DEBUG")]
    Debug,
    #[value(name = "INFO")]
    Info,
    #[value(name = "WARNING")]
    Warning,
    #[value(name = "ERROR")]
    Error,
    #[value(name = "CRITICAL")]
    Critical,
}

impl LogLevel {
    /// The `env_logger` filter this corresponds to. Python's CRITICAL has no
    /// direct equivalent, so it maps to `error`.
    fn as_filter(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warning => "warn",
            LogLevel::Error | LogLevel::Critical => "error",
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Set the Status items in the Google Fonts Traffic Jam board"
)]
struct Args {
    /// Path to the google/fonts repo
    fonts_repo: PathBuf,

    /// Filter the items
    #[arg(short = 'f', long, value_enum, num_args(1..), ignore_case = true)]
    filter: Vec<Filter>,

    /// Specify a range of prs to check e.g 1000-1012
    #[arg(short = 'r', long)]
    pr_range: Option<String>,

    /// Also show items whose pull request is still open
    #[arg(short = 'p', long)]
    show_open_prs: bool,

    /// Where the server data cache is written
    #[arg(short = 's', long)]
    server_data: Option<PathBuf>,

    /// Log level
    #[arg(
        short = 'l',
        long,
        value_enum,
        ignore_case = true,
        default_value = "INFO"
    )]
    log_level: LogLevel,

    /// Only update each traffic jam item's server status
    // Python spells this `-uso`, which is not a flag clap can express.
    #[arg(short = 'u', long)]
    update_servers_only: bool,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    // `-l` sets the default level; `RUST_LOG` still overrides it.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(args.log_level.as_filter()),
    )
    .init();

    if let Err(error) = run(args).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), GftoolsError> {
    // Python checks for the GitHub CLI at import time and refuses to start
    // without it, which is all this does (it ignores the exit status, since `gh`
    // with no arguments does not exit cleanly).
    if Command::new("gh")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return Err(GftoolsError::Misc(
            "GitHub CLI is not installed. https://github.com/cli/cli#installation".to_string(),
        ));
    }

    let gf_path = args.fonts_repo.clone();
    branch_matches_googlefonts_main(&gf_path)?;

    let config = PushConfig::load_default()?;
    let server_data = match &args.server_data {
        Some(path) => path.clone(),
        None => home_path(SERVER_DATA)?,
    };
    let mut servers = if server_data.exists() {
        GfServers::open(&server_data)?
    } else {
        log::warn!(
            "{} not found. Generating file. This may take a while",
            server_data.display()
        );
        GfServers::new(&config).await?
    };

    // Python fails here rather than letting the first board call fail.
    if std::env::var("GH_TOKEN").is_err() {
        return Err(GftoolsError::Misc(
            "GH_TOKEN not found in environment variables. Please set it.".to_string(),
        ));
    }

    servers.servers_online().await?;
    servers.last_pushes()?;
    servers.update_all().await?;
    servers.save(&server_data)?;

    // Python `chdir`s into the checkout here, and every path below is relative
    // to it. The root is passed around instead.
    let traffic_jam_data = home_path(TRAFFIC_JAM_DATA)?;
    let mut push_items = PushItems::from_traffic_jam(Some(&traffic_jam_data)).await?;
    // Python sorts by a boolean, so everything which is not a font comes first,
    // and then drops the items whose pull request is not merged.
    push_items.0.sort_by_key(|item| {
        matches!(
            item.category,
            Some(PushCategory::New) | Some(PushCategory::Upgrade)
        )
    });
    if !args.show_open_prs {
        push_items.0.retain(|item| item.merged == Some(true));
    }
    let mut push_items = apply_filters(push_items, &args, &gf_path)?;
    // Python works the queue from the oldest item, so it reverses the list.
    push_items.0.reverse();

    let dev_url = std::env::var("DEV_META_URL")
        .ok()
        .or_else(|| config.url("dev_url").ok().map(str::to_string));
    let sandbox_url = std::env::var("SANDBOX_URL")
        .ok()
        .or_else(|| config.url("sandbox_url").ok().map(str::to_string));

    let mut checker = ItemChecker {
        push_items: push_items.0,
        gf_fp: gf_path,
        servers: &mut servers,
        config: &config,
        skip_pr: None,
        dev_url,
        sandbox_url,
        input: Box::new(std::io::stdin().lock()),
    };

    if args.update_servers_only {
        log::info!("Updating servers");
        checker.update_servers().await?;
    } else {
        // `Quit` only means the user asked to stop; the checkout below happens
        // either way, as Python's context manager does.
        let _ = checker.run().await?;
    }
    checker.git_checkout_main();
    Ok(())
}

/// `~/.<name>`, as Python's `Path("~") / name` with `expanduser`.
fn home_path(name: &str) -> Result<PathBuf, GftoolsError> {
    dirs::home_dir()
        .map(|home| home.join(name))
        .ok_or_else(|| GftoolsError::Misc(format!("Cannot find the home directory for {name}")))
}

/// Whether the user asked to stop, which is how Python's `sys.exit()` is
/// modelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
}

/// The `--filter` chain and `--pr-range`, in Python's order.
fn apply_filters(
    mut items: PushItems,
    args: &Args,
    gf_path: &Path,
) -> Result<PushItems, GftoolsError> {
    let filters = &args.filter;

    if filters.contains(&Filter::Lists) {
        // Both files are read with the status of the server they are destined
        // for, and no push list, as Python does.
        let production = PushItems::from_server_file(
            &std::fs::read_to_string(gf_path.join("to_production.txt"))?,
            Some(PushStatus::InSandbox),
            None,
        );
        let sandbox = PushItems::from_server_file(
            &std::fs::read_to_string(gf_path.join("to_sandbox.txt"))?,
            Some(PushStatus::InDev),
            None,
        );
        let urls = production
            .0
            .iter()
            .chain(sandbox.0.iter())
            .map(|item| item.url.clone())
            .collect::<Vec<_>>();
        items.0.retain(|item| urls.contains(&item.url));
    }
    if filters.contains(&Filter::InDev) {
        items = items.in_dev();
    }
    if filters.contains(&Filter::InSandbox) {
        items = items.in_sandbox();
    }
    if filters.contains(&Filter::Upgrade) {
        items.0.retain(|item| item.category == Some(PushCategory::Upgrade));
    }
    if filters.contains(&Filter::New) {
        items.0.retain(|item| item.category == Some(PushCategory::New));
    }
    if filters.contains(&Filter::NoFonts) {
        items.0.retain(|item| {
            !matches!(
                item.category,
                Some(PushCategory::New) | Some(PushCategory::Upgrade)
            )
        });
    }
    if filters.contains(&Filter::Fonts) {
        items.0.retain(|item| {
            matches!(
                item.category,
                Some(PushCategory::New) | Some(PushCategory::Upgrade)
            )
        });
    }
    if let Some(range) = &args.pr_range {
        let (start, end) = range.split_once('-').ok_or_else(|| {
            GftoolsError::Misc(format!("'{range}' is not a pr range, e.g. 1000-1012"))
        })?;
        let start: u64 = start
            .trim()
            .parse()
            .map_err(|_| GftoolsError::Misc(format!("'{start}' is not the start of a pr range")))?;
        let end: u64 = end
            .trim()
            .parse()
            .map_err(|_| GftoolsError::Misc(format!("'{end}' is not the end of a pr range")))?;
        // The pull request number is what the url ends with. Python crashes on
        // an item whose url is not a pull request, this skips it.
        items.0.retain(|item| {
            item.url
                .as_deref()
                .and_then(|url| url.rsplit('/').next())
                .and_then(|number| number.parse::<u64>().ok())
                .is_some_and(|number| (start..=end).contains(&number))
        });
    }
    Ok(items)
}

/// `PushItem.__dict__`, which Python splats into the display dict.
fn push_item_fields(item: &PushItem) -> Map<String, Value> {
    let mut res = item.to_json();
    res.insert(
        "push_list".to_string(),
        item.push_list
            .map(|list| json!(list.as_str()))
            .unwrap_or(Value::Null),
    );
    res.insert(
        "merged".to_string(),
        item.merged.map(|merged| json!(merged)).unwrap_or(Value::Null),
    );
    res.insert(
        "id".to_string(),
        item.id.as_deref().map(|id| json!(id)).unwrap_or(Value::Null),
    );
    res.insert(
        "linked_issues".to_string(),
        Value::Array(item.linked_issues.clone()),
    );
    res
}

/// The specimen links Python adds for new and upgraded families. A host which
/// is not configured is left out, where Python would fail to start without its
/// `dev_url`.
fn specimen_urls(item: &Item, dev_url: Option<&str>, sandbox_url: Option<&str>) -> Map<String, Value> {
    let name = item.name().replace(' ', "+");
    let mut res = Map::new();
    for (key, base) in [
        ("dev url", dev_url),
        ("sandbox url", sandbox_url),
        ("prod url", Some(PRODUCTION_URL)),
    ] {
        if let Some(base) = base {
            res.insert(key.to_string(), json!(format!("{base}/specimen/{name}")));
        }
    }
    res
}

/// Write one `vimdiff` view. Python only writes a file for the views which have
/// an item.
fn push_view(
    dir: &tempfile::TempDir,
    files: &mut Vec<PathBuf>,
    name: &str,
    item: Option<&Item>,
) -> Result<(), GftoolsError> {
    let Some(item) = item else {
        return Ok(());
    };
    // Python's temporary file is suffixed with the server name rather than named
    // after it, which is not what vim shows in its status line.
    let path = dir.path().join(format!("{name}.json"));
    write_json(&path, &item.to_json())?;
    files.push(path);
    Ok(())
}

struct ItemChecker<'a> {
    push_items: Vec<PushItem>,
    gf_fp: PathBuf,
    servers: &'a mut GfServers,
    config: &'a PushConfig,
    skip_pr: Option<String>,
    dev_url: Option<String>,
    sandbox_url: Option<String>,
    /// The interactive prompt reads from here, so a test can script it.
    input: Box<dyn BufRead>,
}

impl ItemChecker<'_> {
    /// Python's three skip conditions: already live, not in the checkout, or
    /// part of a pull request the user asked to skip.
    fn should_skip(&self, item: &PushItem) -> bool {
        item.status == Some(PushStatus::Live)
            || !item.exists(&self.gf_fp)
            || item.url == self.skip_pr
    }

    async fn run(&mut self) -> Result<Flow, GftoolsError> {
        for index in 0..self.push_items.len() {
            // Cloned out of the list because the methods below take `&mut self`;
            // it is written back at the end of the iteration.
            let mut item = self.push_items[index].clone();
            if self.should_skip(&item) {
                continue;
            }
            if item.category == Some(PushCategory::Other) {
                log::info!(
                    "No push category defined for {} ({}), skipping",
                    item.path.display(),
                    item.url.as_deref().unwrap_or("")
                );
                continue;
            }
            if let Some(parsed) = item.item(&self.gf_fp)
                && matches!(parsed, Item::Family(_) | Item::FamilyMeta(_))
            {
                self.servers.update(parsed.name()).await;
            }

            self.git_checkout_item(&item);
            self.update_server(&mut item).await?;
            self.display_item(&item)?;
            if self.user_input(&mut item).await? == Flow::Quit {
                return Ok(Flow::Quit);
            }
            self.push_items[index] = item;
        }
        Ok(Flow::Continue)
    }

    async fn update_servers(&mut self) -> Result<(), GftoolsError> {
        for index in 0..self.push_items.len() {
            let mut item = self.push_items[index].clone();
            if self.should_skip(&item) {
                continue;
            }
            if item.category == Some(PushCategory::Other) {
                log::debug!(
                    "No push category defined for {} ({})",
                    item.path.display(),
                    item.url.as_deref().unwrap_or("")
                );
                continue;
            }
            self.update_server(&mut item).await?;
            self.push_items[index] = item;
        }
        Ok(())
    }

    /// Check out the pull request, or `main` when it has been merged.
    fn git_checkout_item(&self, item: &PushItem) {
        match (
            item.merged,
            item.url.as_deref().and_then(|url| url.rsplit('/').next()),
        ) {
            (Some(true), _) => self.git_checkout_main(),
            (_, Some(number)) => self.call("gh", &["pr", "checkout", number, "-f"]),
            (_, None) => log::warn!(
                "{} has no pull request url to check out",
                item.path.display()
            ),
        }
    }

    fn git_checkout_main(&self) {
        self.call("git", &["checkout", "main", "-f"]);
    }

    /// Python runs these with `subprocess.call` and ignores the result: failing
    /// to check out is not fatal.
    fn call(&self, program: &str, args: &[&str]) {
        match Command::new(program)
            .current_dir(&self.gf_fp)
            .args(args)
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => log::warn!("{program} {} exited with {status}", args.join(" ")),
            Err(e) => log::warn!("Failed to run {program}: {e}"),
        }
    }

    /// Record on the board which server the item has reached.
    async fn update_server(&mut self, push_item: &mut PushItem) -> Result<(), GftoolsError> {
        if push_item.merged != Some(true) {
            return Ok(());
        }
        let Some(item) = push_item.item(&self.gf_fp) else {
            // Generally because this is something we do not track, e.g. lang
            // data.
            log::debug!(
                "Cannot update server for {} ({:?}).",
                push_item.path.display(),
                push_item.category
            );
            return Ok(());
        };

        if self.servers.production.compare_push_item(&item) {
            push_item.set_server(PushStatus::Live, self.config).await?;
        } else if self.servers.sandbox.compare_push_item(&item) {
            push_item.set_server(PushStatus::InSandbox, self.config).await?;
        } else {
            // Python's third branch is `servers.dev.find_item(item)`, but
            // `GFServers` has no `dev` server, so it raises AttributeError for
            // every item which is on neither server — which is every brand new
            // family. The status it is reaching for is In Dev.
            push_item.set_server(PushStatus::InDev, self.config).await?;
        }
        Ok(())
    }

    /// Show what the checkout and each server hold for the item.
    fn display_item(&self, push_item: &PushItem) -> Result<(), GftoolsError> {
        let item = push_item.item(&self.gf_fp);
        let mut res = Map::new();
        if let Some(item) = &item {
            res.extend(self.servers.compare_item(item));
            res.extend(push_item_fields(push_item));
            if matches!(
                push_item.category,
                Some(PushCategory::New) | Some(PushCategory::Upgrade)
            ) {
                res.extend(specimen_urls(
                    item,
                    self.dev_url.as_deref(),
                    self.sandbox_url.as_deref(),
                ));
            }
        } else {
            res.extend(push_item_fields(push_item));
        }
        // Python hands the dict to rich's `pprint`, which shows Python objects
        // (`PosixPath(...)`, enum reprs) rather than data; this prints JSON, so
        // the keys come out sorted.
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Object(res))
                .map_err(|e| GftoolsError::Misc(format!("Failed to serialize item: {e}")))?
        );
        Ok(())
    }

    /// Ask what to do with the item, acting on every marker in the answer.
    async fn user_input(&mut self, item: &mut PushItem) -> Result<Flow, GftoolsError> {
        print!("Bump pushlist: [y/n], block: [b] skip pr: [s], inspect: [i], quit: [q]?: ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        // Python's `input()` raises EOFError when there is nothing to read, so
        // end of input quits.
        if self.input.read_line(&mut answer)? == 0 {
            return Ok(Flow::Quit);
        }

        if answer.contains('*') {
            item.bump_pushlist(self.config).await?;
            // Every item of this pull request follows the one just bumped.
            let push_list = item.push_list;
            for other in self.push_items.iter_mut() {
                if other.url == item.url {
                    other.push_list = push_list;
                }
            }
            self.skip_pr = item.url.clone();
        }
        if answer.contains('y') {
            item.bump_pushlist(self.config).await?;
        }
        if answer.contains('b') {
            item.block(self.config).await?;
        }
        if answer.contains('s') {
            self.skip_pr = item.url.clone();
        }
        if answer.contains('i') {
            self.vim_diff(item.item(&self.gf_fp).as_ref())?;
            // Python prompts again here, and the nested answer can quit. The
            // markers in *this* answer are still acted on afterwards.
            if Box::pin(self.user_input(item)).await? == Flow::Quit {
                return Ok(Flow::Quit);
            }
        }
        if answer.contains('q') {
            return Ok(Flow::Quit);
        }
        Ok(Flow::Continue)
    }

    /// Show the checkout's item and each server's version of it in `vimdiff`.
    fn vim_diff(&self, item: Option<&Item>) -> Result<(), GftoolsError> {
        let temp_dir = tempfile::TempDir::new()?;
        let mut files = Vec::new();
        push_view(&temp_dir, &mut files, "local", item)?;
        for server in self.servers.iter() {
            let found = item.and_then(|item| server.find_item(item));
            push_view(&temp_dir, &mut files, server.name(), found.as_ref())?;
        }
        if files.is_empty() {
            // Python starts an empty vimdiff here, which shows nothing.
            log::warn!("Nothing to compare for this item");
            return Ok(());
        }

        match Command::new("vimdiff")
            .current_dir(&self.gf_fp)
            .arg("-c")
            .arg("windo set wrap")
            .args(&files)
            .status()
        {
            // Python ignores this: the user may quit vim with `:cq`.
            Ok(status) if status.success() => {}
            Ok(status) => log::warn!("vimdiff exited with {status}"),
            Err(e) => log::warn!("Failed to run vimdiff: {e}"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use gftools_push::items::Family;

    const CRATE_ROOT: &str = env!("CARGO_MANIFEST_DIR");
    const PR: &str = "https://github.com/google/fonts/pull/1234";

    fn item(
        path: &str,
        category: PushCategory,
        status: Option<PushStatus>,
        url: Option<&str>,
        merged: Option<bool>,
    ) -> PushItem {
        PushItem {
            path: PathBuf::from(path),
            category: Some(category),
            status,
            url: url.map(str::to_string),
            push_list: None,
            merged,
            id: Some("PVTI_item".to_string()),
            linked_issues: Vec::new(),
        }
    }

    fn ids(items: &PushItems) -> Vec<String> {
        items
            .0
            .iter()
            .map(|item| item.path.to_string_lossy().to_string())
            .collect()
    }

    /// `Args` as the CLI would build it, which also pins the flag names.
    fn args(extra: &[&str]) -> Args {
        let mut argv = vec!["gftools-manage-traffic-jam", "/nonexistent"];
        argv.extend_from_slice(extra);
        Args::parse_from(argv)
    }

    /// An empty config is enough for everything which does not reach the board.
    fn config() -> PushConfig {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        PushConfig::load(&path).unwrap()
    }

    /// Servers with no families in them, so nothing is found on either.
    fn servers() -> GfServers {
        GfServers::from_dict(json!({
            "sandbox": {},
            "production": {},
            "last_checked": "2026-01-01",
        }))
        .unwrap()
    }

    fn checker<'a>(
        items: Vec<PushItem>,
        servers: &'a mut GfServers,
        config: &'a PushConfig,
        input: &str,
    ) -> ItemChecker<'a> {
        ItemChecker {
            push_items: items,
            gf_fp: PathBuf::from(CRATE_ROOT).join("data").join("test").join("gf_fonts"),
            servers,
            config,
            skip_pr: None,
            dev_url: Some("https://dev.example.com".to_string()),
            sandbox_url: Some("https://sandbox.example.com".to_string()),
            input: Box::new(Cursor::new(input.to_string())),
        }
    }

    #[test]
    fn test_category_filters() {
        let items = PushItems(vec![
            item(
                "ofl/upgrade",
                PushCategory::Upgrade,
                Some(PushStatus::InDev),
                Some(PR),
                Some(true),
            ),
            item(
                "ofl/new",
                PushCategory::New,
                Some(PushStatus::InDev),
                Some(PR),
                Some(true),
            ),
            item(
                "ofl/metadata",
                PushCategory::Metadata,
                Some(PushStatus::InSandbox),
                Some(PR),
                Some(true),
            ),
        ]);

        let filters = |extra: &[&str]| {
            ids(&apply_filters(items.clone(), &args(extra), Path::new("/nonexistent")).unwrap())
        };

        // `-f` takes one or more values, and no filter keeps everything.
        assert_eq!(ids(&items).len(), 3);
        assert_eq!(filters(&["-f", "upgrade"]), ["ofl/upgrade"]);
        assert_eq!(filters(&["-f", "new"]), ["ofl/new"]);
        assert_eq!(filters(&["-f", "fonts"]), ["ofl/upgrade", "ofl/new"]);
        assert_eq!(filters(&["-f", "no_fonts"]), ["ofl/metadata"]);
        assert_eq!(filters(&["-f", "in_dev"]), ["ofl/upgrade", "ofl/new"]);
        assert_eq!(filters(&["-f", "in_sandbox"]), ["ofl/metadata"]);
        // Several filters are applied in turn.
        assert_eq!(
            filters(&["-f", "in_dev", "upgrade"]),
            ["ofl/upgrade"]
        );
    }

    #[test]
    fn test_lists_filter() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("to_sandbox.txt"),
            "# Upgrade\n# Deleted: ofl/new # https://github.com/google/fonts/pull/1234\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("to_production.txt"),
            "# Upgrade\nofl/metadata # https://github.com/google/fonts/pull/4321\n",
        )
        .unwrap();

        let items = PushItems(vec![
            item(
                "ofl/new",
                PushCategory::New,
                Some(PushStatus::InDev),
                Some(PR),
                Some(true),
            ),
            item(
                "ofl/metadata",
                PushCategory::Metadata,
                Some(PushStatus::InSandbox),
                Some("https://github.com/google/fonts/pull/4321"),
                Some(true),
            ),
            // Not in either file.
            item(
                "ofl/other",
                PushCategory::New,
                Some(PushStatus::InDev),
                Some("https://github.com/google/fonts/pull/9999"),
                Some(true),
            ),
        ]);

        let filtered = apply_filters(items, &args(&["-f", "lists"]), dir.path()).unwrap();
        assert_eq!(ids(&filtered), ["ofl/new", "ofl/metadata"]);
    }

    #[test]
    fn test_pr_range_filter() {
        let items = PushItems(vec![
            item(
                "ofl/one",
                PushCategory::New,
                Some(PushStatus::InDev),
                Some("https://github.com/google/fonts/pull/1001"),
                Some(true),
            ),
            item(
                "ofl/two",
                PushCategory::New,
                Some(PushStatus::InDev),
                Some("https://github.com/google/fonts/pull/1012"),
                Some(true),
            ),
            item(
                "ofl/three",
                PushCategory::New,
                Some(PushStatus::InDev),
                Some("https://github.com/google/fonts/pull/1013"),
                Some(true),
            ),
        ]);

        let filtered = apply_filters(
            items,
            &args(&["-r", "1000-1012"]),
            Path::new("/nonexistent"),
        )
        .unwrap();
        assert_eq!(ids(&filtered), ["ofl/one", "ofl/two"]);

        // A range which is not one is an error rather than a crash.
        let error = apply_filters(
            PushItems::default(),
            &args(&["-r", "nonsense"]),
            Path::new("/nonexistent"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("pr range"), "{error}");
    }

    #[test]
    fn test_specimen_urls() {
        let family = Item::Family(Family {
            name: "Maven Pro".to_string(),
            version: "2.003".to_string(),
        });
        let urls = specimen_urls(&family, Some("https://dev"), Some("https://sandbox"));
        assert_eq!(urls["dev url"], "https://dev/specimen/Maven+Pro");
        assert_eq!(urls["sandbox url"], "https://sandbox/specimen/Maven+Pro");
        assert_eq!(
            urls["prod url"],
            "https://fonts.google.com/specimen/Maven+Pro"
        );

        // A host which is not configured is left out.
        let urls = specimen_urls(&family, None, None);
        assert_eq!(urls.len(), 1);
        assert!(urls.contains_key("prod url"));
    }

    #[test]
    fn test_should_skip() {
        let servers = &mut servers();
        let config = config();
        // The checkout fixture has `ofl/mavenpro` and nothing else.
        let checker = checker(
            Vec::new(),
            servers,
            &config,
            "",
        );

        let present = item(
            "ofl/mavenpro",
            PushCategory::Upgrade,
            Some(PushStatus::InDev),
            Some(PR),
            Some(true),
        );
        assert!(!checker.should_skip(&present));

        // Already live, not in the checkout, or skipped by the user.
        let mut live = present.clone();
        live.status = Some(PushStatus::Live);
        assert!(checker.should_skip(&live));

        let mut missing = present.clone();
        missing.path = PathBuf::from("ofl/not-here");
        assert!(checker.should_skip(&missing));

        let mut skipped = checker;
        skipped.skip_pr = Some(PR.to_string());
        assert!(skipped.should_skip(&present));
        // Every item of the skipped pull request goes with it.
        let mut other = present.clone();
        other.path = PathBuf::from("ofl/mavenpro");
        other.url = Some(PR.to_string());
        assert!(skipped.should_skip(&other));
    }

    #[test]
    fn test_display_item_fields_and_urls() {
        let servers = &mut servers();
        let config = config();
        let checker = checker(Vec::new(), servers, &config, "");

        // An upgrade of the fixture family: the item is read from the checkout,
        // and the specimen urls are added.
        let upgrade = item(
            "ofl/mavenpro",
            PushCategory::Upgrade,
            Some(PushStatus::InDev),
            Some(PR),
            Some(true),
        );
        let mut res = Map::new();
        let parsed = upgrade.item(&checker.gf_fp).unwrap();
        res.extend(checker.servers.compare_item(&parsed));
        res.extend(push_item_fields(&upgrade));
        res.extend(specimen_urls(
            &parsed,
            checker.dev_url.as_deref(),
            checker.sandbox_url.as_deref(),
        ));

        // Neither server has the family, so both comparisons are false, and the
        // item's own fields are all there.
        assert_eq!(res["In sandbox"], json!(false));
        assert_eq!(res["In production"], json!(false));
        assert_eq!(res["path"], json!("ofl/mavenpro"));
        assert_eq!(res["category"], json!("Upgrade"));
        assert_eq!(res["status"], json!("In Dev / PR Merged"));
        assert_eq!(res["url"], json!(PR));
        assert_eq!(res["id"], json!("PVTI_item"));
        assert_eq!(res["merged"], json!(true));
        assert_eq!(res["push_list"], Value::Null);
        assert_eq!(res["linked_issues"], json!([]));
        assert!(res["prod url"].as_str().unwrap().contains("Maven+Pro"));

        // A non-font item is listed without the specimen urls.
        let metadata = item(
            "ofl/mavenpro",
            PushCategory::Metadata,
            Some(PushStatus::InSandbox),
            Some(PR),
            Some(true),
        );
        assert_eq!(
            specimen_urls(&metadata.item(&checker.gf_fp).unwrap(), Some("d"), Some("s"))
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn test_user_input_skip_and_quit() {
        let servers = &mut servers();
        let config = config();
        let one_item = || {
            vec![item(
                "ofl/mavenpro",
                PushCategory::Upgrade,
                Some(PushStatus::InDev),
                Some(PR),
                Some(true),
            )]
        };

        {
            // "s" skips this pull request, "q" stops.
            let mut checker = checker(one_item(), servers, &config, "s\nq\n");
            let mut push_item = checker.push_items[0].clone();
            assert_eq!(
                checker.user_input(&mut push_item).await.unwrap(),
                Flow::Continue
            );
            assert_eq!(checker.skip_pr.as_deref(), Some(PR));
            assert_eq!(
                checker.user_input(&mut push_item).await.unwrap(),
                Flow::Quit
            );
        }

        // `*` makes every item of the pull request follow the one bumped, but
        // that reaches the board, so it is not exercised here.
        let mut checker = checker(one_item(), servers, &config, "");
        let mut push_item = checker.push_items[0].clone();
        // End of input quits, where Python raises EOFError.
        assert_eq!(checker.user_input(&mut push_item).await.unwrap(), Flow::Quit);
    }
}
