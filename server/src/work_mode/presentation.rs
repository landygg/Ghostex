//! What a work-mode session publishes: which PR, issues and Linear project it is linked to, and
//! its branch title. Pure cache reads, so the presentation projection can call it per row.

use serde_json::{json, Map, Value};

use crate::session_git_status::{cached_session_git_status, effective_session_git_cwd};

use super::*;

/// What a session is linked to, before any status is looked up. The background pass fetches
/// exactly these; the projection reads the answers back from the caches.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct WorkTargets {
    pub(crate) cwd: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) linear_key: Option<u64>,
    pub(crate) linear_issues: Vec<String>,
    pub(crate) github_issues: Vec<u64>,
    /// A PR number (as text) or URL, as `gh pr view` takes it.
    pub(crate) pull_request: Option<String>,
    /// The branch's own PR from the git probe, which also carries its state.
    pub(crate) branch_pull_request: Option<u64>,
    pub(crate) linear_project: Option<Option<String>>,
    /// A hand-set GitHub Project (`owner/number`); `None` = automatic, from the issues' and the
    /// PR's project items.
    pub(crate) github_project: Option<Option<String>>,
}

/// Resolves a work-mode session's links: hand-set ones first, then what its branch names, then
/// (for the PR) what Linear's GitHub integration attached to its issue.
pub(crate) fn work_targets(project: &Value, session: &Value) -> WorkTargets {
    let project_id = project
        .get("projectId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let cwd = effective_session_git_cwd(session, Some(project));
    let git = cwd.as_deref().and_then(cached_session_git_status);
    let branch =
        work_branch(git.as_ref().and_then(|git| git.branch.as_deref())).map(str::to_string);
    let manual = manual_work_links(session);
    let linear_key = project_linear_key(project_id);

    let linear_issues = match manual.linear_issues {
        Some(identifiers) => identifiers,
        None => match (
            branch.as_deref(),
            linear_key.and_then(cached_linear_team_keys),
        ) {
            (Some(branch), Some(team_keys)) => linear_identifiers_in_branch(branch, &team_keys),
            _ => Vec::new(),
        },
    };
    let github_issues = match manual.github_issues {
        Some(numbers) => numbers,
        None => branch
            .as_deref()
            .and_then(github_issue_in_branch)
            .into_iter()
            .collect(),
    };
    let branch_pull_request = branch.as_ref().and_then(|_| {
        git.as_ref()
            .and_then(|git| git.pull_request.as_ref())
            .and_then(|pull_request| u64::try_from(pull_request.number).ok())
    });
    let pull_request = match manual.pull_request {
        Some(Some(ManualPullRequest::Number(number))) => Some(number.to_string()),
        Some(Some(ManualPullRequest::Url(url))) => Some(url),
        Some(None) => None,
        None => branch_pull_request
            .map(|number| number.to_string())
            .or_else(|| {
                linear_key.and_then(|key| {
                    linear_issues.iter().find_map(|identifier| {
                        cached_linear_issue(key, identifier)
                            .and_then(|issue| issue.pull_request_urls.first().cloned())
                    })
                })
            }),
    };
    WorkTargets {
        cwd,
        branch,
        linear_key,
        linear_issues,
        github_issues,
        pull_request,
        branch_pull_request,
        linear_project: manual.linear_project,
        github_project: manual.github_project,
    }
}

/// `PresentationSession.work` (packages/gx-protocol/src/presentation.rs); `None` outside work mode.
pub(crate) fn presentation_session_work(project: &Value, session: &Value) -> Option<Value> {
    if !project_work_mode(project) {
        return None;
    }
    let targets = work_targets(project, session);
    let mut output = Map::new();
    if let Some(branch) = &targets.branch {
        output.insert("branch".to_string(), json!(branch));
    }
    if let Some(pull_request) = presentation_pull_request(&targets) {
        if pull_request.get("state").and_then(Value::as_str) == Some("merged") {
            let key = work_cleanup_pull_request_key(
                pull_request
                    .get("number")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                pull_request.get("url").and_then(Value::as_str),
            );
            if offers_work_cleanup(session, &key) {
                output.insert("offerCleanup".to_string(), json!(true));
            }
        }
        output.insert("pullRequest".to_string(), pull_request);
    }
    // The menu's "Back to automatic" only means something when a link was set by hand.
    if manual_work_links(session) != ManualWorkLinks::default() {
        output.insert("handSet".to_string(), json!(true));
    }

    let issues: Vec<Option<LinearIssueInfo>> = targets
        .linear_issues
        .iter()
        .map(|identifier| {
            targets
                .linear_key
                .and_then(|key| cached_linear_issue(key, identifier))
        })
        .collect();
    let linear_issues: Vec<Value> = targets
        .linear_issues
        .iter()
        .zip(&issues)
        .map(|(identifier, info)| {
            let mut issue = Map::new();
            issue.insert("identifier".to_string(), json!(identifier));
            if let Some(info) = info {
                insert_text(&mut issue, "title", info.title.as_deref());
                insert_text(&mut issue, "stateType", info.state_type.as_deref());
                insert_text(&mut issue, "stateName", info.state_name.as_deref());
                insert_text(&mut issue, "url", info.url.as_deref());
            }
            Value::Object(issue)
        })
        .collect();
    if !linear_issues.is_empty() {
        output.insert("linearIssues".to_string(), Value::Array(linear_issues));
    }

    let github_issues: Vec<Value> = targets
        .github_issues
        .iter()
        .map(|number| {
            let mut issue = Map::new();
            issue.insert("number".to_string(), json!(number));
            if let Some(info) = targets
                .cwd
                .as_deref()
                .and_then(|cwd| cached_work_github_issue(cwd, *number))
            {
                insert_text(&mut issue, "title", info.title.as_deref());
                insert_text(&mut issue, "state", info.state.as_deref());
                insert_text(&mut issue, "url", info.url.as_deref());
            }
            Value::Object(issue)
        })
        .collect();
    if !github_issues.is_empty() {
        output.insert("githubIssues".to_string(), Value::Array(github_issues));
    }

    // CDXC:WorkMode 2026-10-09 DECISION:
    // User: the card shows the project kind of the workspace's primary tracker: a Linear project for Linear, a GitHub Project for GitHub (crate::work_mode::tracker).
    let tracker = cached_project_work_tracker(
        project
            .get("projectId")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let linear_project = match &targets.linear_project {
        _ if tracker != WorkTracker::Linear => None,
        Some(Some(name)) => Some(json!({ "name": name })),
        Some(None) => None,
        None => issues.iter().flatten().find_map(|issue| {
            issue.project_name.as_ref().map(|name| {
                let mut project = Map::new();
                project.insert("name".to_string(), json!(name));
                insert_text(&mut project, "url", issue.project_url.as_deref());
                Value::Object(project)
            })
        }),
    };
    if let Some(linear_project) = linear_project {
        output.insert("linearProject".to_string(), linear_project);
    }
    if tracker == WorkTracker::Github {
        if let Some(github_project) = session_github_project(&targets) {
            output.insert(
                "githubProject".to_string(),
                presentation_github_project(&github_project),
            );
        }
    }
    Some(Value::Object(output))
}

/// The GitHub Project a session links to: the hand-set one, else the first project item of its
/// GitHub issues, then of its PR.
pub(crate) fn session_github_project(targets: &WorkTargets) -> Option<GithubProjectInfo> {
    match &targets.github_project {
        Some(Some(reference)) => {
            let (owner, number) = parse_github_project_reference(reference)?;
            Some(
                cached_github_project(&owner, number).unwrap_or(GithubProjectInfo {
                    owner,
                    number,
                    title: None,
                    url: None,
                    status: None,
                }),
            )
        }
        Some(None) => None,
        None => {
            let cwd = targets.cwd.as_deref()?;
            targets
                .github_issues
                .iter()
                .copied()
                .chain(
                    targets
                        .pull_request
                        .as_deref()
                        .and_then(pull_request_number_of),
                )
                .find_map(|number| cached_github_project_items(cwd, number).into_iter().next())
        }
    }
}

/// `123` or `https://github.com/o/r/pull/123` → 123.
pub(crate) fn pull_request_number_of(selector: &str) -> Option<u64> {
    let selector = selector.trim().trim_end_matches('/');
    selector
        .parse()
        .ok()
        .or_else(|| selector.rsplit_once("/pull/")?.1.parse().ok())
}

fn presentation_github_project(project: &GithubProjectInfo) -> Value {
    let mut output = Map::new();
    output.insert("owner".to_string(), json!(project.owner));
    output.insert("number".to_string(), json!(project.number));
    insert_text(&mut output, "title", project.title.as_deref());
    insert_text(&mut output, "url", project.url.as_deref());
    insert_text(&mut output, "status", project.status.as_deref());
    Value::Object(output)
}

fn presentation_pull_request(targets: &WorkTargets) -> Option<Value> {
    let selector = targets.pull_request.as_deref()?;
    let cwd = targets.cwd.as_deref().unwrap_or_default();
    if let Some(info) = cached_work_pull_request(cwd, selector) {
        let mut pull_request = Map::new();
        pull_request.insert("number".to_string(), json!(info.number));
        pull_request.insert("state".to_string(), json!(info.state.as_wire()));
        insert_text(&mut pull_request, "url", info.url.as_deref());
        insert_text(&mut pull_request, "checks", info.checks);
        return Some(Value::Object(pull_request));
    }
    // Not read yet: the branch's own PR still has its state from the git probe.
    let number: u64 = selector.parse().ok()?;
    if targets.branch_pull_request != Some(number) {
        return None;
    }
    let git = targets.cwd.as_deref().and_then(cached_session_git_status)?;
    let branch_pull_request = git.pull_request?;
    let mut pull_request = Map::new();
    pull_request.insert("number".to_string(), json!(number));
    pull_request.insert(
        "state".to_string(),
        json!(branch_pull_request.state.as_wire()),
    );
    insert_text(&mut pull_request, "url", branch_pull_request.url.as_deref());
    Some(Value::Object(pull_request))
}

/// The title a work-mode session shows: its branch title, unless the person renamed it.
pub(crate) fn work_display_title(
    project: &Value,
    session: &Value,
    title_source: Option<&str>,
) -> Option<String> {
    if !project_work_mode(project) || title_source == Some("user") {
        return None;
    }
    let cwd = effective_session_git_cwd(session, Some(project))?;
    let git = cached_session_git_status(&cwd)?;
    work_branch(git.branch.as_deref()).and_then(branch_title)
}

fn insert_text(output: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        output.insert(key.to_string(), json!(value));
    }
}
