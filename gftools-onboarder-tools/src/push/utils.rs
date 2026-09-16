use chrono::{Local, TimeZone};
use gftools::GftoolsError;
use gix::Repository;
use gix::object::tree::diff::{Action, ChangeDetached};
use serde::Serialize;
use serde_json::Value;
use serde_json::ser::PrettyFormatter;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Remotes are identified by url rather than by name, because the name is
/// arbitrary (`origin` in a normal clone).
const GOOGLE_FONTS_URL_SUFFIX: &str = "google/fonts";

/// Does this remote url point at google/fonts?
///
/// Python tests for `"google/fonts.git" in remote.url`, which misses a clone
/// made with `git clone https://github.com/google/fonts` (git stores the url
/// exactly as given, so there is no `.git` to match). Accepting both spellings
/// costs nothing and avoids failing on a perfectly good checkout.
fn is_google_fonts_url(url: &str) -> bool {
    url.trim_end_matches('/')
        .trim_end_matches(".git")
        .ends_with(GOOGLE_FONTS_URL_SUFFIX)
}

/// The name of the remote which points at google/fonts. Mirrors Python's
/// `_get_google_fonts_remote`.
fn get_googlefonts_remote(repo: &Repository) -> Result<String, GftoolsError> {
    for name in repo.remote_names() {
        let name = name.to_string();
        let Ok(remote) = repo.find_remote(name.as_str()) else {
            continue;
        };
        if remote
            .urls(gix::remote::Direction::Fetch)
            .any(|url| is_google_fonts_url(&url.to_bstring().to_string()))
        {
            return Ok(name);
        }
    }
    Err(GftoolsError::Misc(
        "Cannot find a remote with a google/fonts url".to_string(),
    ))
}

/// Check that the local checkout is in sync with the google/fonts `main`
/// branch.
///
/// Mirrors `branch_matches_google_fonts_main`: fetch `main` from the google/fonts
/// remote, then compare the local HEAD tree with the fetched tree. The fetch
/// shells out to `git fetch`, as Python does — `gix` can only *add* refspecs to
/// a remote, not replace them, so a `gix` fetch would pull every branch of the
/// remote rather than just `main`.
pub fn branch_matches_googlefonts_main(path: &Path) -> Result<bool, GftoolsError> {
    let repo = gix::open(path)?;
    let remote_name = get_googlefonts_remote(&repo)?;

    // `git fetch <remote> main` writes the fetched tip to `FETCH_HEAD`, and
    // updates `refs/remotes/<remote>/main` when the usual fetch refspec is
    // configured.
    let status = Command::new("git")
        .current_dir(path)
        .args(["fetch", &remote_name, "main"])
        .status()
        .map_err(|e| GftoolsError::Git(format!("Failed to run git fetch: {e}")))?;
    if !status.success() {
        return Err(GftoolsError::GitNetwork(format!(
            "git fetch {remote_name} main failed: {status}"
        )));
    }

    // Prefer `FETCH_HEAD`, which is guaranteed to be what we just fetched; fall
    // back to the remote-tracking branch, which is what Python compares against.
    let remote_id = repo
        .rev_parse_single("FETCH_HEAD")
        .or_else(|_| repo.rev_parse_single(format!("refs/remotes/{remote_name}/main").as_str()))
        .map_err(|e| {
            GftoolsError::Git(format!(
                "Failed to resolve the fetched {remote_name}/main: {e}"
            ))
        })?;
    let remote_tree = repo
        .find_object(remote_id)
        .map_err(|e| GftoolsError::Git(format!("Failed to read the fetched commit: {e}")))?
        .peel_to_tree()
        .map_err(|e| GftoolsError::Git(format!("Failed to peel the fetched commit: {e}")))?;
    let head_tree = repo
        .head_tree()
        .map_err(|e| GftoolsError::Git(format!("Failed to read the HEAD tree: {e}")))?;

    // An empty change list means the two trees are identical.
    let changes = repo
        .diff_tree_to_tree(&head_tree, &remote_tree, None)
        .map_err(|e| GftoolsError::Git(format!("Failed to diff trees: {e}")))?;
    if !changes.is_empty() {
        return Err(GftoolsError::Git(
            "Your local branch is not in sync with the google/fonts main branch. \
             Please pull or remove any commits."
                .to_string(),
        ));
    }
    Ok(true)
}

/// Write JSON with Python's `indent=4`.
///
/// Keys come out sorted, because `serde_json` is built without
/// `preserve_order`; Python keeps whatever order the server sent. Both sides of
/// a comparison are sorted the same way, so this only shows up in the output.
pub fn write_json(path: &Path, value: &Value) -> Result<(), GftoolsError> {
    let file = std::fs::File::create(path)?;
    let mut serializer =
        serde_json::Serializer::with_formatter(file, PrettyFormatter::with_indent(b"    "));
    value
        .serialize(&mut serializer)
        .map_err(|e| GftoolsError::Misc(format!("Failed to serialize {}: {e}", path.display())))
}

/// Paths which differ between the index and the working tree.
///
/// This is what Python's `repo.diff()` computes — libgit2's `index.
/// diff_to_workdir` — so it is `git diff --name-only`: unstaged edits to tracked
/// files, and nothing else. Staged changes and untracked files are not
/// included, which is why `gen_push_lists` can use it to spot a locally edited
/// `tags/all/families.csv`.
///
/// `gix` only exposes tree-to-tree diffs, so the worktree side of this shells
/// out to `git`.
pub fn worktree_changes(root: &Path) -> Result<Vec<String>, GftoolsError> {
    let output = Command::new("git")
        .current_dir(root)
        // `-z` keeps paths raw: no quoting of non-ASCII names.
        .args(["diff", "--name-only", "--no-renames", "-z"])
        .output()
        .map_err(|e| GftoolsError::Git(format!("Failed to run git diff: {e}")))?;
    if !output.status.success() {
        return Err(GftoolsError::Git(format!(
            "git diff failed in {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

/// A commit in the google/fonts history, as reported by `push-stats`.
///
/// The field names are the keys of the commit dict Python builds, because the
/// report template reads them directly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommitInfo {
    /// Local time, formatted as Python's naive `datetime.isoformat()`.
    pub date: String,
    pub title: String,
    pub author: String,
    /// `"new"` or `"modified"`.
    pub status: &'static str,
    /// `"family"`, `"metadata"`, `"designer"` or `"infrastructure"`.
    pub kind: &'static str,
    pub id: String,
}

/// Classify every commit reachable from `HEAD` for the push-stats report.
///
/// Port of `get_commits`. Each commit is diffed against the next commit in the
/// walk, so the oldest commit is only ever used as a base and never reported
/// itself. The walk is sorted by commit time, newest first, which is what
/// libgit2 does when a walk is left unsorted — and the order matters, because it
/// decides which pair of commits gets diffed.
pub fn repo_commits(path: &Path) -> Result<Vec<CommitInfo>, GftoolsError> {
    let mut repo = gix::open(path)?;
    // Detached straight away: an attached id borrows the repository, which
    // would clash with setting the object cache below.
    let head = repo
        .head_id()
        .map_err(|e| GftoolsError::Git(format!("Failed to resolve HEAD: {e}")))?
        .detach();

    // gix has no object cache by default, and this walk decompresses the same
    // large trees for every commit pair, so set one sized for the repository.
    let index = repo
        .index_or_load_from_head_or_empty()
        .map_err(|e| GftoolsError::Git(format!("Failed to read the index: {e}")))?;
    let cache_size = repo.compute_object_cache_size_for_tree_diffs(&index);
    repo.object_cache_size_if_unset(cache_size);
    // Without a reusable diff cache, every tree diff reloads the index and
    // rebuilds the attribute stack, which turns the walk into a multi-minute
    // job.
    let mut resource_cache = repo
        .diff_resource_cache_for_tree_diff()
        .map_err(|e| GftoolsError::Git(format!("Failed to set up the diff cache: {e}")))?;

    // Python materialises the whole walk before pairing the commits up. libgit2
    // sorts a walk by commit time unless told otherwise, while gix's default is
    // breadth-first by generation; the order decides which commit each one is
    // diffed against, so it has to match.
    let mut commits = Vec::new();
    let walk = repo
        .rev_walk([head])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(|e| GftoolsError::Git(format!("Failed to walk the history: {e}")))?;
    for info in walk {
        let info =
            info.map_err(|e| GftoolsError::Git(format!("Failed to walk the history: {e}")))?;
        commits.push(
            info.object()
                .map_err(|e| GftoolsError::Git(format!("Failed to read a commit: {e}")))?,
        );
    }

    let mut res = Vec::new();
    for idx in 1..commits.len() {
        let current = &commits[idx - 1];
        let prev = &commits[idx];

        let message = current
            .message_raw()
            .map_err(|e| GftoolsError::Git(format!("Failed to read a commit message: {e}")))?;
        let message = String::from_utf8_lossy(message);
        if message.contains("Merge branch") {
            continue;
        }
        // Python takes `message.split("\n")[0]`, which is the first *line*, not
        // the commit title (which ends at the first blank line).
        let title = message.split('\n').next().unwrap_or_default().to_string();
        let author = String::from_utf8_lossy(
            current
                .author()
                .map_err(|e| GftoolsError::Git(format!("Failed to read a commit author: {e}")))?
                .name,
        )
        .to_string();
        // Python's `commit_time` is the *committer's* timestamp, formatted as
        // naive local time.
        let seconds = current
            .time()
            .map_err(|e| GftoolsError::Git(format!("Failed to read a commit time: {e}")))?
            .seconds;
        let date = Local
            .timestamp_opt(seconds, 0)
            .single()
            .ok_or_else(|| GftoolsError::Git(format!("Commit time out of range: {seconds}")))?
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();

        let prev_tree = prev
            .tree()
            .map_err(|e| GftoolsError::Git(format!("Failed to read a tree: {e}")))?;
        let current_tree = current
            .tree()
            .map_err(|e| GftoolsError::Git(format!("Failed to read a tree: {e}")))?;

        let mut platform = prev_tree
            .changes()
            .map_err(|e| GftoolsError::Git(format!("Failed to diff trees: {e}")))?;
        // Python's tree diff does no rename detection, but gix defaults to the
        // git configuration, which is rename tracking at 50% similarity — that
        // reads blobs to score them, so turn it off.
        platform.options(|options| {
            options.track_rewrites(None);
        });

        // Python's classification: all deltas added means "new" (`all()` of an
        // empty diff is true as well), anything else is "modified". The kind is
        // the first of family/metadata/designer which any delta matches, so the
        // flags have to be gathered across every delta before choosing.
        // Python's `endswith` is a plain suffix test on the whole path, with no
        // "." in front of the extensions.
        let mut all_added = true;
        let mut family = false;
        let mut metadata = false;
        let mut designer = false;
        platform
            .for_each_to_obtain_tree_with_cache(&current_tree, &mut resource_cache, |change| {
                let (location, added, is_tree) = match change.detach() {
                    ChangeDetached::Addition {
                        location,
                        entry_mode,
                        ..
                    } => (location, true, entry_mode.is_tree()),
                    ChangeDetached::Deletion {
                        location,
                        entry_mode,
                        ..
                    } => (location, false, entry_mode.is_tree()),
                    ChangeDetached::Modification {
                        location,
                        entry_mode,
                        ..
                    } => (location, false, entry_mode.is_tree()),
                    // A rewrite is a rename or a copy. Rewrite tracking is off,
                    // so this arm is unreachable, but it keeps the match
                    // exhaustive.
                    ChangeDetached::Rewrite { location, .. } => (location, false, false),
                };
                // pygit2's `diff_to_tree` recurses into trees and reports only
                // files, while gix also reports directory entries so that
                // directory changes can be reconstructed. Skip them, or a commit
                // which adds a family directory stops looking like it is all
                // additions.
                if is_tree {
                    return Ok::<_, Infallible>(Action::Continue(()));
                }
                all_added &= added;
                let path = String::from_utf8_lossy(&location);
                if path.ends_with("ttf") || path.ends_with("otf") {
                    family = true;
                } else if path.ends_with("metadata.pb") || path.ends_with("DESCRIPTION.en_us.html")
                {
                    metadata = true;
                } else if path.ends_with("info.pb") {
                    designer = true;
                }
                Ok::<_, Infallible>(Action::Continue(()))
            })
            .map_err(|e| GftoolsError::Git(format!("Failed to diff trees: {e}")))?;

        res.push(CommitInfo {
            date,
            title,
            author,
            status: if all_added { "new" } else { "modified" },
            kind: if family {
                "family"
            } else if metadata {
                "metadata"
            } else if designer {
                "designer"
            } else {
                "infrastructure"
            },
            id: current.id.to_string(),
        });
    }
    Ok(res)
}

pub(crate) fn repo_path_to_google_path(fp: &Path) -> PathBuf {
    let parts = fp.components().map(|c| c.as_os_str()).collect::<Vec<_>>();
    // We rename lang paths due to: https://github.com/google/fonts/pull/4679
    if parts.iter().any(|c| *c == "gflanguages") {
        if let Ok(rest) = fp.strip_prefix("lang/Lib/gflanguages/data") {
            return Path::new("lang").join(rest);
        }
    }
    // https://github.com/google/fonts/pull/5147
    else if parts.iter().any(|c| *c == "axisregistry")
        && let Some(name) = fp.file_name()
    {
        return Path::new("axisregistry").join(name);
    }
    fp.to_path_buf()
}

pub(crate) fn google_path_to_repo_path(fp: &Path) -> PathBuf {
    let parts = fp.components().map(|c| c.as_os_str()).collect::<Vec<_>>();
    let in_site_packages = parts.iter().any(|c| *c == "site-packages");
    if parts.iter().any(|c| *c == "lang") && !in_site_packages {
        if let Ok(rest) = fp.strip_prefix("lang") {
            return Path::new("lang/Lib/gflanguages/data/").join(rest);
        }
    } else if parts.iter().any(|c| *c == "axisregistry")
        && !in_site_packages
        && let (Some(parent), Some(name)) = (fp.parent(), fp.file_name())
    {
        return parent.join("Lib/axisregistry/data").join(name);
    }
    fp.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_path_to_google_path() {
        assert_eq!(
            repo_path_to_google_path(Path::new(
                "lang/Lib/gflanguages/data/languages/aa_Latn.textproto"
            )),
            PathBuf::from("lang/languages/aa_Latn.textproto")
        );
        assert_eq!(
            repo_path_to_google_path(Path::new(
                "axisregistry/Lib/axisregistry/data/bounce.textproto"
            )),
            PathBuf::from("axisregistry/bounce.textproto")
        );
        // Paths which need no rewriting are returned unchanged.
        assert_eq!(
            repo_path_to_google_path(Path::new("ofl/mavenpro")),
            PathBuf::from("ofl/mavenpro")
        );
        // "gflanguages" is present but the path is not the expected layout: we
        // must not panic.
        assert_eq!(
            repo_path_to_google_path(Path::new("somewhere/gflanguages/odd")),
            PathBuf::from("somewhere/gflanguages/odd")
        );
    }

    #[test]
    fn test_google_path_to_repo_path() {
        assert_eq!(
            google_path_to_repo_path(Path::new("lang/languages/aa_Latn.textproto")),
            PathBuf::from("lang/Lib/gflanguages/data/languages/aa_Latn.textproto")
        );
        assert_eq!(
            google_path_to_repo_path(Path::new("axisregistry/bounce.textproto")),
            PathBuf::from("axisregistry/Lib/axisregistry/data/bounce.textproto")
        );
        assert_eq!(
            google_path_to_repo_path(Path::new("ofl/mavenpro")),
            PathBuf::from("ofl/mavenpro")
        );
    }

    #[test]
    fn test_is_google_fonts_url() {
        assert!(is_google_fonts_url("https://github.com/google/fonts"));
        assert!(is_google_fonts_url("https://github.com/google/fonts.git"));
        assert!(is_google_fonts_url("git@github.com:google/fonts.git"));
        assert!(is_google_fonts_url("https://github.com/google/fonts/"));
        assert!(!is_google_fonts_url(
            "https://github.com/google/material-design"
        ));
        assert!(!is_google_fonts_url("https://github.com/someone/fonts"));
    }

    /// The families.csv check in `gen_push_lists` hinges on this being the
    /// *unstaged* diff, as libgit2's index-to-workdir diff is.
    #[test]
    fn test_worktree_changes() {
        let dir =
            std::env::temp_dir().join(format!("gftools-push-worktree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init"]);

        write(&dir, "tags/all/families.csv", "Family,Tag\n");
        write(&dir, "ofl/first/METADATA.pb", "name: \"First\"\n");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "Add files"]);
        assert_eq!(worktree_changes(&dir).unwrap().len(), 0);

        // An unstaged edit is reported...
        write(&dir, "tags/all/families.csv", "Family,Tag\nMaven Pro,\n");
        assert_eq!(
            worktree_changes(&dir).unwrap().join(","),
            "tags/all/families.csv"
        );

        // ...and staging it takes it out of the diff again.
        git(&dir, &["add", "tags/all/families.csv"]);
        assert_eq!(worktree_changes(&dir).unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Needs a clone of google/fonts and network access. Point `GF_FONTS_PATH`
    /// at one to run it, e.g.
    /// `GF_FONTS_PATH=~/others-repos/fonts cargo test -p gftools-push --lib -- --ignored`.
    #[test]
    #[ignore = "requires a google/fonts clone and network access"]
    fn test_branch_matches_googlefonts_main() {
        let path = std::env::var("GF_FONTS_PATH")
            .expect("set GF_FONTS_PATH to a local google/fonts clone");
        match branch_matches_googlefonts_main(Path::new(&path)) {
            // The checkout is in sync with remote main.
            Ok(true) => {}
            // Divergence is reported as an error, mirroring Python's `raise`,
            // so this arm is unreachable.
            Ok(false) => unreachable!("divergence is reported as an error"),
            // The checkout is behind or ahead. The fetch and the tree diff both
            // worked and this is the function doing its job, so it is not a
            // failure — which of the two outcomes we get just depends on the
            // state of the local clone.
            Err(GftoolsError::Git(message)) => assert!(
                message.contains("not in sync"),
                "unexpected git error: {message}"
            ),
            // Anything else means the remote lookup or the fetch broke.
            Err(other) => panic!("failed to check the checkout: {other}"),
        }
    }

    /// Run `git` in `dir` with a hermetic identity and no user/system config,
    /// so a `commit.gpgsign` or a hook in the real environment cannot break it.
    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Committer")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .status()
            .expect("git should be installed");
        assert!(status.success(), "git {args:?} failed");
    }

    fn write(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().expect("a file has a parent")).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Build a repository with one commit per reported category, plus an empty
    /// commit and a "Merge branch" commit, and check the classification matches
    /// `get_commits`.
    #[test]
    fn test_repo_commits() {
        let dir = std::env::temp_dir().join(format!("gftools-push-commits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init"]);

        // Oldest first: this commit is only ever the base of the next diff, so
        // it is never reported itself.
        write(&dir, "ofl/first/Foo.ttf", "font");
        write(&dir, "ofl/first/METADATA.pb", "name: \"First\"\n");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "Add First"]);

        write(&dir, "ofl/first/Foo.ttf", "font, modified");
        git(&dir, &["add", "."]);
        git(
            &dir,
            &["commit", "-m", "Update First\n\nAnd some body text."],
        );

        write(&dir, "ofl/second/Bar.otf", "font");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "Add Second"]);

        write(
            &dir,
            "ofl/first/METADATA.pb",
            "name: \"First\"\ncomment: x\n",
        );
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "Update metadata"]);

        write(&dir, "ofl/second/DESCRIPTION.en_us.html", "<p>Second</p>");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "Add description"]);

        write(&dir, "ofl/third/info.pb", "designer: \"Someone\"\n");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "Add designer"]);

        write(&dir, "README.md", "# Fonts\n");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "Add readme"]);

        git(&dir, &["commit", "--allow-empty", "-m", "Empty commit"]);
        git(
            &dir,
            &[
                "commit",
                "--allow-empty",
                "-m",
                "Merge branch 'main' into dev",
            ],
        );

        let commits = repo_commits(&dir).unwrap();
        let summary = commits
            .iter()
            .map(|commit| (commit.status, commit.kind))
            .collect::<Vec<_>>();

        let _ = std::fs::remove_dir_all(&dir);

        // Newest first. The "Merge branch" commit is skipped, the root commit is
        // not reported, the empty commit counts as "new", and a commit which
        // only touches METADATA.pb is "infrastructure" because Python's suffix
        // check is case sensitive and the real file name is upper case.
        assert_eq!(
            summary,
            vec![
                ("new", "infrastructure"),
                ("new", "infrastructure"),
                ("new", "designer"),
                ("new", "metadata"),
                ("modified", "infrastructure"),
                ("new", "family"),
                ("modified", "family"),
            ]
        );
        assert_eq!(commits[0].author, "Test Author");
        assert_eq!(commits[0].title, "Empty commit");
        assert_eq!(commits[0].id.len(), 40);
        // A naive local timestamp, as Python's `datetime.isoformat()` produces.
        assert_eq!(commits[0].date.len(), 19);
        assert_eq!(commits[0].date[4..5].to_string(), "-");
        assert_eq!(commits[0].date[10..11].to_string(), "T");
    }
}
