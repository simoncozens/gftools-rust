//! The GitHub-ish data source the packager needs, behind a trait.
//!
//! Python builds a `GitHubClient(base_repo, "fonts")` from
//! `metadata.source.repository_url` deep inside `save_metadata` and `download_assets`,
//! which makes the whole flow untestable without a token. Here the operations are:
//!
//! - `commit(branch)` — resolve a branch/branch-name ref to a commit sha
//! - `file_contents(path, branch)` — a file's bytes
//! - `latest_release_asset()` — a release asset url (for `--latest-release`)
//! - `open_prs(head, base)`, `create_pr`, `update_pr`, `create_issue_comment`,
//!   `add_labels`, `get_labels`
//! - `add_to_traffic_jam(project_id, content_id)` — the `addProjectV2ItemById` mutation
//!
//! Two implementations: `octocrab` for production, and a local-git one used by the
//! offline end-to-end tests (and by anyone whose "upstream" is a local clone).
//! See PORTING_PLAN.md §8.1.
