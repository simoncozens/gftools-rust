//! Port of `gftools.scripts.gen_push_lists`.
//!
//! Rewrites `to_sandbox.txt` and `to_production.txt` in a google/fonts checkout:
//! what is already in those files, plus the families the Traffic Jam board says
//! are ready, minus whatever has already moved on. The checkout has to be in
//! sync with the remote `main` branch, and the board is read over the network
//! with `GH_TOKEN`.

use std::path::{Path, PathBuf};

use clap::Parser;
use gftools::{GftoolsError, is_google_fonts_repo};
use gftools_push::trafficjam::{PushItems, PushList, PushStatus};
use gftools_push::utils::{branch_matches_googlefonts_main, worktree_changes};

/// `tags/all/families.csv` is not on the board, so a local edit to it is added
/// to `to_sandbox.txt` by hand. Python puts it in a `"tags"` bin which its
/// output loop never reaches, hence this append.
const TAGS_PATH: &str = "tags/all/families.csv";
const TAGS_ENTRY: &str = "\n# Tags\ntags/all/families.csv\n";

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Generate the to_production.txt and to_sandbox.txt server files in a local google/fonts repository"
)]
struct Args {
    /// Path to the google/fonts repo
    gf_path: PathBuf,
}

#[tokio::main]
async fn main() {
    // The board is fetched with log lines about pagination; warnings are how a
    // failed poll shows up, so show them by default. Raise the level with
    // `RUST_LOG`.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args = Args::parse();
    if let Err(error) = run(args).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), GftoolsError> {
    let gf_path = args.gf_path.as_path();
    if !is_google_fonts_repo(gf_path) {
        return Err(GftoolsError::Misc(format!(
            "'{}' is not a valid google/fonts repo",
            gf_path.display()
        )));
    }

    // Python `chdir`s into the checkout for the duration. Everything below takes
    // the root explicitly instead, so the process stays where it is.
    branch_matches_googlefonts_main(gf_path)?;

    // No cache file, as in Python's bare `from_traffic_jam()` call.
    let board_items = PushItems::from_traffic_jam(None).await?;

    write_push_lists(gf_path, &board_items)
}

/// Rewrite the two server files from what they hold plus `board_items`.
///
/// Split out of [`run`] so everything except the board fetch can be tested
/// without a GitHub token.
fn write_push_lists(gf_path: &Path, board_items: &PushItems) -> Result<(), GftoolsError> {
    let to_sandbox_fp = gf_path.join("to_sandbox.txt");
    let to_production_fp = gf_path.join("to_production.txt");

    // Read both files before either is rewritten.
    let sandbox_file = PushItems::from_server_file(
        &read_server_file(&to_sandbox_fp)?,
        Some(PushStatus::InDev),
        Some(PushList::ToSandbox),
    );
    let production_file = PushItems::from_server_file(
        &read_server_file(&to_production_fp)?,
        Some(PushStatus::InSandbox),
        Some(PushList::ToProduction),
    );

    let (to_sandbox, to_production) = combine(&sandbox_file, &production_file, board_items);

    std::fs::write(&to_sandbox_fp, to_sandbox.to_server_file(gf_path))?;
    std::fs::write(&to_production_fp, to_production.to_server_file(gf_path))?;

    // Python compares the working tree with the index here, after the two files
    // above have been written, and appends the family tag list if it is one of
    // the changed paths.
    if worktree_changes(gf_path)?
        .iter()
        .any(|path| path.contains(TAGS_PATH))
    {
        let mut contents = read_server_file(&to_sandbox_fp)?;
        contents.push_str(TAGS_ENTRY);
        std::fs::write(&to_sandbox_fp, contents)?;
    }

    Ok(())
}

fn read_server_file(path: &Path) -> Result<String, GftoolsError> {
    Ok(std::fs::read_to_string(path)?)
}

/// The two server files, from what they already hold plus what the board says.
///
/// Mirrors `(sandbox_file + sandbox_board) - production_board` and
/// `(production_file + production_board) - live_board`: an item which is already
/// on its way to production drops out of the sandbox list, and one which is live
/// is not pushed again.
fn combine(
    sandbox_file: &PushItems,
    production_file: &PushItems,
    board: &PushItems,
) -> (PushItems, PushItems) {
    let sandbox_board = board.to_sandbox();
    let production_board = board.to_production();
    let live_board = board.live();

    (
        sandbox_file
            .added(&sandbox_board)
            .subtracted(&production_board),
        production_file
            .added(&production_board)
            .subtracted(&live_board),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gftools_push::trafficjam::{PushCategory, PushItem};

    /// An item as `from_traffic_jam` builds them: a family directory, its
    /// category and the pull request which asked for it.
    fn item(path: &str, status: PushStatus, push_list: PushList, pr: u32) -> PushItem {
        PushItem {
            path: PathBuf::from(path),
            category: Some(PushCategory::Upgrade),
            status: Some(status),
            url: Some(format!("https://github.com/google/fonts/pull/{pr}")),
            push_list: Some(push_list),
            merged: Some(true),
            id: None,
            linked_issues: Vec::new(),
        }
    }

    fn paths(items: &PushItems) -> Vec<String> {
        let mut paths = items
            .0
            .iter()
            .map(|item| item.path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    /// `(sandbox_file + sandbox_board) - production_board`, and the same for
    /// production, are the whole point of the script, so pin them, and pin what
    /// they render to.
    #[test]
    fn test_push_lists_arithmetic() {
        let sandbox_file = PushItems(vec![
            item("ofl/upgrademe", PushStatus::InDev, PushList::ToSandbox, 1),
            // Already promoted on the board, so it must leave the sandbox list.
            item(
                "ofl/readyforprod",
                PushStatus::InSandbox,
                PushList::ToProduction,
                4,
            ),
        ]);
        let production_file = PushItems(vec![item(
            "ofl/insandbox",
            PushStatus::InSandbox,
            PushList::ToProduction,
            2,
        )]);
        let board = PushItems(vec![
            item("ofl/upgrademe", PushStatus::InDev, PushList::ToSandbox, 1),
            item("ofl/newfamily", PushStatus::InDev, PushList::ToSandbox, 3),
            item(
                "ofl/readyforprod",
                PushStatus::InSandbox,
                PushList::ToProduction,
                4,
            ),
            // Live, so it must not be pushed again.
            item(
                "ofl/alreadylive",
                PushStatus::Live,
                PushList::ToProduction,
                5,
            ),
        ]);

        let (to_sandbox, to_production) = combine(&sandbox_file, &production_file, &board);

        // `ofl/readyforprod` is on the production board, so it drops out of the
        // sandbox list, and `ofl/newfamily` joins it. Note `ofl/upgrademe`
        // arrives twice — from the file and from the board — because `add` does
        // not deduplicate, exactly as in Python.
        assert_eq!(
            paths(&to_sandbox),
            ["ofl/newfamily", "ofl/upgrademe", "ofl/upgrademe"]
        );
        // `ofl/alreadylive` is live, so it is not pushed again.
        assert_eq!(paths(&to_production), ["ofl/insandbox", "ofl/readyforprod"]);

        // The duplicate is resolved when the file is rendered.
        let root = tempfile::TempDir::new().unwrap();
        for path in [
            "ofl/upgrademe",
            "ofl/newfamily",
            "ofl/readyforprod",
            "ofl/insandbox",
            "ofl/alreadylive",
        ] {
            std::fs::create_dir_all(root.path().join(path)).unwrap();
        }
        assert_eq!(
            to_sandbox.to_server_file(root.path()),
            "# Upgrade\n\
             ofl/newfamily # https://github.com/google/fonts/pull/3\n\
             ofl/upgrademe # https://github.com/google/fonts/pull/1\n"
        );
        assert_eq!(
            to_production.to_server_file(root.path()),
            "# Upgrade\n\
             ofl/insandbox # https://github.com/google/fonts/pull/2\n\
             ofl/readyforprod # https://github.com/google/fonts/pull/4\n"
        );
    }

    /// `git` with a hermetic identity, as the `utils` tests do.
    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .status()
            .expect("git should be installed");
        assert!(status.success(), "git {args:?} failed");
    }

    /// The file side of the script end to end: what the files already hold, what
    /// the board adds, and the local family tag list being appended.
    #[test]
    fn test_write_push_lists() {
        let root = tempfile::TempDir::new().unwrap();
        let dir = root.path();
        for path in ["ofl/upgrademe", "ofl/newfamily", "tags/all"] {
            std::fs::create_dir_all(dir.join(path)).unwrap();
        }
        std::fs::write(
            dir.join("to_sandbox.txt"),
            "# Upgrade\nofl/upgrademe # https://github.com/google/fonts/pull/1\n",
        )
        .unwrap();
        std::fs::write(dir.join("to_production.txt"), "").unwrap();
        std::fs::write(dir.join("tags/all/families.csv"), "Family,Tag\n").unwrap();
        git(dir, &["init"]);
        git(dir, &["add", "."]);
        git(dir, &["commit", "-m", "Initial"]);

        let board = PushItems(vec![item(
            "ofl/newfamily",
            PushStatus::InDev,
            PushList::ToSandbox,
            3,
        )]);
        write_push_lists(dir, &board).unwrap();

        // What the file already held survives with its url, the new family
        // joins it, and the entries are sorted.
        assert_eq!(
            std::fs::read_to_string(dir.join("to_sandbox.txt")).unwrap(),
            "# Upgrade\n\
             ofl/newfamily # https://github.com/google/fonts/pull/3\n\
             ofl/upgrademe # https://github.com/google/fonts/pull/1\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("to_production.txt")).unwrap(),
            ""
        );

        // An unstaged edit to the family tag list is appended to the sandbox
        // file, because it is not something the board knows about.
        let mut tags = std::fs::read_to_string(dir.join("tags/all/families.csv")).unwrap();
        tags.push_str("Maven Pro,\n");
        std::fs::write(dir.join("tags/all/families.csv"), tags).unwrap();

        write_push_lists(dir, &board).unwrap();
        let sandbox = std::fs::read_to_string(dir.join("to_sandbox.txt")).unwrap();
        assert!(sandbox.ends_with(TAGS_ENTRY), "{sandbox:?}");
        assert_eq!(sandbox.matches("tags/all/families.csv").count(), 1);
    }
}
