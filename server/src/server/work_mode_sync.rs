//! Running work mode (crate::work_mode): the background pass that refreshes Linear and `gh`
//! status for work-mode sessions and publishes the rows whose links changed, and publishing a
//! project whose Work mode switch flipped.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

use super::{schedule_presentation_project_delta, schedule_presentation_session_delta, AppState};
use crate::domain::{DomainRepository, DomainStateError};
use crate::logging::{GxserverLogInput, LogLevel};
use crate::storage::open_gxserver_database;
use crate::work_mode::{
    cached_project_work_tracker, presentation_session_work, project_has_linear_key,
    project_work_mode, refresh_work_caches, work_display_title, WorkTracker,
};

/// What each work-mode project last published as `workLinear` and `workTracker`, so a pass
/// republishes a project whose Linear key was set or removed or whose tracker changed.
fn published_linear_projects() -> &'static Mutex<HashMap<String, (bool, WorkTracker)>> {
    static PUBLISHED: OnceLock<Mutex<HashMap<String, (bool, WorkTracker)>>> = OnceLock::new();
    PUBLISHED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What each work-mode session last published, so a pass only sends the rows that changed.
fn published_work() -> &'static Mutex<HashMap<(String, String), String>> {
    static PUBLISHED: OnceLock<Mutex<HashMap<(String, String), String>>> = OnceLock::new();
    PUBLISHED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn work_fingerprint(project: &Value, session: &Value) -> String {
    let title_source = crate::presentation::project_session_title(session)
        .get("titleSource")
        .and_then(Value::as_str)
        .map(str::to_string);
    json!({
        "work": presentation_session_work(project, session),
        "title": work_display_title(project, session, title_source.as_deref()),
    })
    .to_string()
}

/// One pass: refresh the caches every work-mode session reads, then publish the rows whose links,
/// statuses or branch title changed. Blocking; runs on the 60s background worker.
pub(crate) fn run_work_mode_refresh_once(state: &Arc<AppState>) -> Result<(), DomainStateError> {
    let db = open_gxserver_database(&state.paths).map_err(|error| DomainStateError {
        code: "internalError",
        message: format!("SQLite gxserver state error: {error}"),
    })?;
    let repository = DomainRepository::new(&db, state.metadata.server_id.as_str());
    let projects: Vec<Value> = repository
        .list_projects()?
        .into_iter()
        .filter(project_work_mode)
        .collect();
    if projects.is_empty() {
        return Ok(());
    }
    let mut sessions = Vec::new();
    for project in &projects {
        if let Some(project_id) = project.get("projectId").and_then(Value::as_str) {
            sessions.extend(repository.list_sessions(Some(project_id))?);
        }
    }
    refresh_work_caches(&state.paths, &projects, &sessions);

    let mut linear_changed = Vec::new();
    if let Ok(mut published) = published_linear_projects().lock() {
        for project in &projects {
            let Some(project_id) = project.get("projectId").and_then(Value::as_str) else {
                continue;
            };
            let has_key = (
                project_has_linear_key(project),
                cached_project_work_tracker(project_id),
            );
            if published.insert(project_id.to_string(), has_key) != Some(has_key) {
                linear_changed.push(project_id.to_string());
            }
        }
    }
    for project_id in linear_changed {
        schedule_presentation_project_delta(
            state,
            &db,
            &repository,
            &project_id,
            "projectUpdated",
        )?;
    }

    let projects_by_id: HashMap<&str, &Value> = projects
        .iter()
        .filter_map(|project| Some((project.get("projectId")?.as_str()?, project)))
        .collect();
    let mut changed = Vec::new();
    if let Ok(mut published) = published_work().lock() {
        for session in &sessions {
            let (Some(project_id), Some(session_id)) = (
                session.get("projectId").and_then(Value::as_str),
                session.get("sessionId").and_then(Value::as_str),
            ) else {
                continue;
            };
            let Some(project) = projects_by_id.get(project_id) else {
                continue;
            };
            let fingerprint = work_fingerprint(project, session);
            let key = (project_id.to_string(), session_id.to_string());
            if published.get(&key) != Some(&fingerprint) {
                published.insert(key.clone(), fingerprint);
                changed.push(key);
            }
        }
    }
    for (project_id, session_id) in changed {
        schedule_presentation_session_delta(state, &db, &repository, &project_id, &session_id)?;
    }
    Ok(())
}

/// Publishes a project whose Work mode switch flipped, and every one of its sessions, whose cards
/// gain or lose their links and branch titles with it.
pub(crate) fn publish_project_work_mode_change(
    state: &AppState,
    db: &rusqlite::Connection,
    repository: &DomainRepository<'_>,
    project_id: &str,
) -> Result<(), DomainStateError> {
    schedule_presentation_project_delta(state, db, repository, project_id, "projectUpdated")?;
    if let Ok(mut published) = published_work().lock() {
        published.retain(|(published_project, _), _| published_project != project_id);
    }
    for session in repository.list_sessions(Some(project_id))? {
        if let Some(session_id) = session.get("sessionId").and_then(Value::as_str) {
            schedule_presentation_session_delta(state, db, repository, project_id, session_id)?;
        }
    }
    Ok(())
}

/// Runs a pass right away (off the request path), so a link or switch someone just set shows its
/// status without waiting for the next minute.
pub(crate) fn spawn_work_mode_refresh(state: &Arc<AppState>) {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(error) = run_work_mode_refresh_once(&state) {
            log_work_mode_refresh_failure(&state, &error.message);
        }
    });
}

pub(crate) fn log_work_mode_refresh_failure(state: &AppState, message: &str) {
    let _ = state.logger.log(GxserverLogInput {
        level: LogLevel::Warn,
        event: "workModeRefreshFailed".to_string(),
        server_id: Some(state.metadata.server_id.clone()),
        request_id: None,
        client: None,
        duration_ms: None,
        error: Some(message.to_string()),
        details: None,
    });
}
