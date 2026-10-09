//! Linear's GraphQL API: issue status, team keys and key checks, behind TTL caches the
//! presentation projection only reads.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

const LINEAR_GRAPHQL_URL: &str = "https://api.linear.app/graphql";
const LINEAR_TIMEOUT: Duration = Duration::from_secs(10);
/// Issue status changes on human timescales; two minutes keeps cards fresh without hammering
/// Linear's rate limit.
const LINEAR_ISSUE_TTL: Duration = Duration::from_secs(120);
const LINEAR_TEAM_KEYS_TTL: Duration = Duration::from_secs(60 * 60);
/// After a failed call (bad key, no network) a key is left alone for this long.
const LINEAR_FAILURE_BACKOFF: Duration = Duration::from_secs(5 * 60);
const MAX_LINEAR_ISSUES_PER_REQUEST: usize = 25;
/// Requests one background pass may send to Linear.
pub(crate) const MAX_LINEAR_REQUESTS_PER_PASS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LinearIssueInfo {
    pub(crate) identifier: String,
    pub(crate) title: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) state_type: Option<String>,
    pub(crate) state_name: Option<String>,
    pub(crate) project_name: Option<String>,
    pub(crate) project_url: Option<String>,
    /// GitHub PRs Linear's GitHub integration attached to the issue.
    pub(crate) pull_request_urls: Vec<String>,
}

struct Cached<T> {
    value: T,
    fetched_at: Instant,
}

#[derive(Default)]
struct LinearCache {
    team_keys: HashMap<u64, Cached<Vec<String>>>,
    failed_at: HashMap<u64, Instant>,
    /// `None` = Linear said there is no such issue.
    issues: HashMap<(u64, String), Cached<Option<LinearIssueInfo>>>,
    /// Which key each project's calls use, refreshed by the background pass so the projection
    /// never reads the credentials file.
    project_keys: HashMap<String, u64>,
}

fn cache() -> &'static Mutex<LinearCache> {
    static CACHE: OnceLock<Mutex<LinearCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(LinearCache::default()))
}

/// A stable stand-in for a key, so caches never hold the key itself.
pub(crate) fn linear_key_fingerprint(api_key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    api_key.trim().hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn remember_project_linear_key(project_id: &str, fingerprint: Option<u64>) {
    if let Ok(mut cache) = cache().lock() {
        match fingerprint {
            Some(fingerprint) => {
                cache
                    .project_keys
                    .insert(project_id.to_string(), fingerprint);
            }
            None => {
                cache.project_keys.remove(project_id);
            }
        }
    }
}

pub(crate) fn project_linear_key(project_id: &str) -> Option<u64> {
    cache().lock().ok()?.project_keys.get(project_id).copied()
}

pub(crate) fn cached_linear_team_keys(fingerprint: u64) -> Option<Vec<String>> {
    let cache = cache().lock().ok()?;
    cache
        .team_keys
        .get(&fingerprint)
        .map(|cached| cached.value.clone())
}

pub(crate) fn cached_linear_issue(fingerprint: u64, identifier: &str) -> Option<LinearIssueInfo> {
    let cache = cache().lock().ok()?;
    cache
        .issues
        .get(&(fingerprint, identifier.to_string()))
        .and_then(|cached| cached.value.clone())
}

fn in_backoff(cache: &LinearCache, fingerprint: u64) -> bool {
    cache
        .failed_at
        .get(&fingerprint)
        .is_some_and(|failed_at| failed_at.elapsed() < LINEAR_FAILURE_BACKOFF)
}

/// One GraphQL call. `Err` carries a sentence fit to show a person.
pub(super) fn linear_graphql(
    api_key: &str,
    query: &str,
    variables: Value,
) -> Result<Value, String> {
    let api_key = api_key.trim();
    // Personal API keys go in as they are; OAuth tokens need the Bearer scheme.
    let authorization = if api_key.starts_with("lin_oauth_") {
        format!("Bearer {api_key}")
    } else {
        api_key.to_string()
    };
    let agent = ureq::AgentBuilder::new().timeout(LINEAR_TIMEOUT).build();
    let response = agent
        .post(LINEAR_GRAPHQL_URL)
        .set("Authorization", &authorization)
        .set("Content-Type", "application/json")
        .send_json(json!({ "query": query, "variables": variables }));
    let body: Value = match response {
        Ok(response) => response
            .into_json()
            .map_err(|error| format!("Linear sent an unreadable answer: {error}"))?,
        Err(ureq::Error::Status(400, response)) => response
            .into_json()
            .map_err(|error| format!("Linear sent an unreadable answer: {error}"))?,
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => {
            return Err("Linear did not accept this API key.".to_string());
        }
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("Linear answered with HTTP {code}."));
        }
        Err(error) => return Err(format!("Could not reach Linear: {error}")),
    };
    if body.get("data").is_none_or(Value::is_null) {
        let message = body
            .pointer("/errors/0/message")
            .and_then(Value::as_str)
            .unwrap_or("Linear returned no data.");
        return Err(message.to_string());
    }
    Ok(body)
}

/// Checks a key and says whose it is: `{ id, name, organization }` (`id` is the Linear user's).
pub(crate) fn verify_linear_api_key(api_key: &str) -> Result<Value, String> {
    let body = linear_graphql(
        api_key,
        "query { viewer { id name email } organization { name urlKey } }",
        json!({}),
    )?;
    Ok(json!({
        "id": body.pointer("/data/viewer/id").cloned().unwrap_or(Value::Null),
        "name": body.pointer("/data/viewer/name").cloned().unwrap_or(Value::Null),
        "organization": body.pointer("/data/organization/name").cloned().unwrap_or(Value::Null),
    }))
}

/// Refreshes the team keys behind a key when they are stale. Returns whether a request was sent.
pub(crate) fn refresh_linear_team_keys(api_key: &str) -> bool {
    let fingerprint = linear_key_fingerprint(api_key);
    {
        let Ok(cache) = cache().lock() else {
            return false;
        };
        let fresh = cache
            .team_keys
            .get(&fingerprint)
            .is_some_and(|cached| cached.fetched_at.elapsed() < LINEAR_TEAM_KEYS_TTL);
        if fresh || in_backoff(&cache, fingerprint) {
            return false;
        }
    }
    let result = linear_graphql(
        api_key,
        "query { teams(first: 250) { nodes { key } } }",
        json!({}),
    );
    let Ok(mut cache) = cache().lock() else {
        return true;
    };
    match result {
        Ok(body) => {
            let keys = body
                .pointer("/data/teams/nodes")
                .and_then(Value::as_array)
                .map(|nodes| {
                    nodes
                        .iter()
                        .filter_map(|node| node.get("key").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            cache.team_keys.insert(
                fingerprint,
                Cached {
                    value: keys,
                    fetched_at: Instant::now(),
                },
            );
            cache.failed_at.remove(&fingerprint);
        }
        Err(_) => {
            cache.failed_at.insert(fingerprint, Instant::now());
        }
    }
    true
}

/// Fetches the stale ones among `identifiers`, at most `request_budget` requests. Returns how
/// many requests were sent.
pub(crate) fn refresh_linear_issues(
    api_key: &str,
    identifiers: &[String],
    request_budget: usize,
) -> usize {
    let fingerprint = linear_key_fingerprint(api_key);
    let stale: Vec<String> = {
        let Ok(cache) = cache().lock() else {
            return 0;
        };
        if in_backoff(&cache, fingerprint) {
            return 0;
        }
        let mut stale: Vec<String> = identifiers
            .iter()
            .filter(|identifier| {
                cache
                    .issues
                    .get(&(fingerprint, (*identifier).clone()))
                    .is_none_or(|cached| cached.fetched_at.elapsed() >= LINEAR_ISSUE_TTL)
            })
            .cloned()
            .collect();
        stale.sort();
        stale.dedup();
        stale
    };
    let mut sent = 0;
    for chunk in stale.chunks(MAX_LINEAR_ISSUES_PER_REQUEST) {
        if sent >= request_budget {
            break;
        }
        sent += 1;
        let mut declarations = Vec::new();
        let mut selections = Vec::new();
        let mut variables = Map::new();
        for (index, identifier) in chunk.iter().enumerate() {
            declarations.push(format!("$i{index}: String!"));
            selections.push(format!("i{index}: issue(id: $i{index}) {{ ...WorkIssue }}"));
            variables.insert(format!("i{index}"), json!(identifier));
        }
        let query = format!(
            "query({}) {{ {} }} fragment WorkIssue on Issue {{ identifier title url state {{ name type }} project {{ name url }} attachments(first: 20) {{ nodes {{ url }} }} }}",
            declarations.join(", "),
            selections.join(" ")
        );
        let result = linear_graphql(api_key, &query, Value::Object(variables));
        let Ok(mut cache) = cache().lock() else {
            return sent;
        };
        match result {
            Ok(body) => {
                for (index, identifier) in chunk.iter().enumerate() {
                    let info = body
                        .pointer(&format!("/data/i{index}"))
                        .filter(|issue| issue.is_object())
                        .map(|issue| parse_issue(identifier, issue));
                    cache.issues.insert(
                        (fingerprint, identifier.clone()),
                        Cached {
                            value: info,
                            fetched_at: Instant::now(),
                        },
                    );
                }
                cache.failed_at.remove(&fingerprint);
            }
            Err(_) => {
                cache.failed_at.insert(fingerprint, Instant::now());
                break;
            }
        }
    }
    sent
}

fn parse_issue(identifier: &str, issue: &Value) -> LinearIssueInfo {
    let text = |pointer: &str| {
        issue
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    LinearIssueInfo {
        identifier: text("/identifier").unwrap_or_else(|| identifier.to_string()),
        title: text("/title"),
        url: text("/url"),
        state_type: text("/state/type"),
        state_name: text("/state/name"),
        project_name: text("/project/name"),
        project_url: text("/project/url"),
        pull_request_urls: issue
            .pointer("/attachments/nodes")
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|node| node.get("url").and_then(Value::as_str))
                    .filter(|url| url.contains("github.com/") && url.contains("/pull/"))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    }
}
