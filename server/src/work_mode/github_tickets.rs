//! Creating a GitHub issue with `gh issue create`, for a workspace whose primary tracker is GitHub:
//! the project header's Create ticket dialog, the Work page's New ticket button,
//! `ghostex work-mode create-ticket`, and the Slack flow's "no ticket in the thread" step (which
//! runs on the requester's Ghostex because Convex has no GitHub token).
//!
//! CDXC:WorkMode 2026-10-09 DECISION:
//! User: with GitHub as the primary tracker, Create ticket makes a GitHub issue in the project's
//! repo, assigned to the person (`@me`), and starts work the same way as a Linear ticket (a new
//! worktree on `<user>/<number>-<slug>`, linked, nothing sent).

use std::time::Duration;

use super::github_projects::run_gh_full;

/// `gh issue create` makes one API call; this leaves room for a slow network.
const CREATE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_TITLE_CHARS: usize = 256;

/// What `gh issue create` made.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CreatedGithubIssue {
    pub(crate) number: u64,
    pub(crate) url: String,
    /// `owner/repo`, lowercased.
    pub(crate) repo: String,
}

pub(crate) struct NewGithubIssue<'a> {
    pub(crate) title: &'a str,
    pub(crate) body: Option<&'a str>,
    pub(crate) assign_to_me: bool,
    /// `owner/repo`; `None` creates it in the repo of `cwd`.
    pub(crate) repo: Option<&'a str>,
}

/// Runs `gh issue create` in `cwd` (the project's checkout). The error is `gh`'s own words.
pub(crate) fn create_github_issue(
    cwd: &str,
    issue: &NewGithubIssue<'_>,
) -> Result<CreatedGithubIssue, String> {
    let title = issue.title.trim();
    if title.is_empty() {
        return Err("An issue needs a title.".to_string());
    }
    let title: String = title.chars().take(MAX_TITLE_CHARS).collect();
    let body = issue.body.map(str::trim).unwrap_or_default();
    let mut args = vec!["issue", "create", "--title", title.as_str(), "--body", body];
    if issue.assign_to_me {
        args.extend(["--assignee", "@me"]);
    }
    if let Some(repo) = issue.repo {
        args.extend(["--repo", repo]);
    }
    let run = run_gh_full(Some(cwd), &args, CREATE_TIMEOUT)
        .ok_or("The GitHub CLI (gh) did not answer. Is it installed and signed in (gh auth login)?")?;
    if !run.success {
        return Err(format!(
            "GitHub did not create the issue: {}",
            run.error_text()
        ));
    }
    parse_created_issue_url(&run.stdout)
        .ok_or_else(|| "GitHub created the issue but gh did not say which one.".to_string())
}

/// The last `https://github.com/<owner>/<repo>/issues/<n>` line `gh issue create` prints.
pub(crate) fn parse_created_issue_url(output: &str) -> Option<CreatedGithubIssue> {
    output.lines().rev().find_map(|line| {
        let url = line.trim();
        let rest = url.strip_prefix("https://github.com/")?;
        let mut parts = rest.split('/');
        let owner = parts.next()?;
        let repo = parts.next()?;
        if parts.next()? != "issues" {
            return None;
        }
        let number = parts.next()?.parse().ok()?;
        Some(CreatedGithubIssue {
            number,
            url: url.to_string(),
            repo: format!("{owner}/{repo}").to_ascii_lowercase(),
        })
    })
}
