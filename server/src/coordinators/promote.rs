//! Making an existing Claude, Codex, ZCode or Empryo session a coordinator (`/api/promoteCoordinator`, the
//! sidebar's Advanced > Make Coordinator, `ghostex coordinator promote`).

use std::path::Path;

use rusqlite::{Connection, Transaction, TransactionBehavior};
use serde_json::{json, Map, Value};

use super::endpoint::session_title_of;
use super::records::{
    insert_coordinator, read_coordinator, read_thread, sql_error, SessionKey,
    COORDINATOR_GOAL_MAX_CHARS,
};
use super::role::{
    coordinator_agent_family_supported, coordinator_role_queued_command, with_coordinator_role,
    COORDINATOR_AGENT_FAMILIES_TEXT, COORDINATOR_ROLE_PROMPT,
};
use crate::domain::{DomainRepository, DomainStateError};

/// The session commands a resume, an account switch or a fork rebuilds the agent from; the role
/// flag goes into each one the session has.
const SAVED_COMMAND_KEYS: [&str; 3] = ["agentCommand", "accountBaseCommand", "accountCommand"];

/// What a promotion did: the session it turned into a coordinator, and the message that hands the
/// running agent its playbook.
pub struct CoordinatorPromotion {
    pub key: SessionKey,
    pub title: String,
    /// The line that hands the session its role, queued ahead of the playbook, for a family whose
    /// role arrives that way (`coordinator_role_queued_command`).
    pub role_command: Option<String>,
    pub playbook_message: String,
}

/// CDXC:Coordinators 2026-10-04 DECISION:
/// User: "Make Coordinator" sits in the session context menu's Advanced submenu, "but I wanna be able to switch to coordinator without affecting the running thread at all". So promoting never restarts, reloads or interrupts the session: it stores the coordinator record (the crown, the thread tree, reports and `ghostex coordinator status` follow from it), writes the role flag into the session's saved commands so the next start or resume the session makes for its own reasons (sleep and wake, a Full Reload, an app restart) launches it with the role exactly like a created coordinator, and queues the playbook as a chat message that the queue delivers only once the agent is idle. Ghostex never resumes the session to apply the role.
/// SEE-ALSO: server/src/server/coordinator_runtime.rs (`promote_coordinator`, which queues the message), server/src/agents/launch_plan.rs (`apply_coordinator_role`, the same flag at creation), packages/gx-core/src/sidebar_menu/session.rs (the menu row).
pub fn promote_session_to_coordinator(
    db: &Connection,
    server_id: &str,
    params: &Map<String, Value>,
    role_file: &Path,
) -> Result<CoordinatorPromotion, DomainStateError> {
    let text = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    let (project_id, session_id) = (text("projectId"), text("sessionId"));
    if project_id.is_empty() || session_id.is_empty() {
        return Err(DomainStateError::bad_request(
            "Making an orchestrator needs projectId and sessionId.",
        ));
    }
    let goal = text("goal");
    if goal.chars().count() > COORDINATOR_GOAL_MAX_CHARS {
        return Err(DomainStateError::bad_request(format!(
            "Keep the goal under {COORDINATOR_GOAL_MAX_CHARS} characters; set standing instructions later with ghostex orchestrator set-instructions."
        )));
    }
    // One writer reservation over the read and both writes, so a hook updating the session in
    // between cannot be overwritten by the row read here.
    let transaction =
        Transaction::new_unchecked(db, TransactionBehavior::Immediate).map_err(sql_error)?;
    let repository = DomainRepository::new(&transaction, server_id);
    let session = repository
        .get_session(&project_id, &session_id)?
        .ok_or_else(|| DomainStateError::not_found("That session does not exist."))?;
    let title = session_title_of(&session);
    refuse_ineligible(
        &transaction,
        &repository,
        &session,
        &project_id,
        &session_id,
    )?;
    let project = repository
        .get_project(&project_id)?
        .ok_or_else(|| DomainStateError::not_found("The session's project does not exist."))?;
    let family = crate::agents::session_agent_family_id(&project, &session)
        .filter(|family| coordinator_agent_family_supported(family))
        .ok_or_else(|| {
            DomainStateError::bad_request(format!(
                "An orchestrator runs on {COORDINATOR_AGENT_FAMILIES_TEXT}, and this session runs another agent. Start a New Orchestrator instead."
            ))
        })?;

    let mut runtime = session
        .get("runtimeSettings")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for key in SAVED_COMMAND_KEYS {
        let Some(command) = runtime
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        if command_has_coordinator_role(&command, &family) {
            continue;
        }
        runtime.insert(
            key.to_string(),
            json!(with_coordinator_role(&command, &family, role_file)?),
        );
    }
    // A session that never saved its command resumes from its launcher's command (else the
    // agent's default), which has no role; save that command with the role so the next resume
    // carries it.
    if !runtime.contains_key("agentCommand") {
        let agent_id = crate::agents::read_text_value(&session, "agentId").unwrap_or_default();
        let launch_settings = crate::agents::object_field(&session, "launchSettings");
        let config = crate::agents::resolve_project_agent_config(
            &project,
            &agent_id,
            Some(&launch_settings),
        );
        let base = crate::agents::read_text_from_map(&config, "command")
            .or_else(|| crate::agents::default_agent_command(&family).map(str::to_string))
            .unwrap_or_else(|| family.clone());
        runtime.insert(
            "agentCommand".to_string(),
            json!(with_coordinator_role(&base, &family, role_file)?),
        );
    }
    let mut update = Map::new();
    update.insert("projectId".to_string(), json!(project_id));
    update.insert("sessionId".to_string(), json!(session_id));
    update.insert("runtimeSettings".to_string(), Value::Object(runtime));
    repository.update_session(&update)?;
    insert_coordinator(&transaction, &project_id, &session_id, &goal, "")?;
    transaction.commit().map_err(sql_error)?;

    let role_command = coordinator_role_queued_command(&family);
    let playbook_message = playbook_message(&goal, role_command.is_some());
    Ok(CoordinatorPromotion {
        key: (project_id, session_id),
        title,
        role_command,
        playbook_message,
    })
}

fn refuse_ineligible(
    db: &Connection,
    repository: &DomainRepository<'_>,
    session: &Value,
    project_id: &str,
    session_id: &str,
) -> Result<(), DomainStateError> {
    if read_coordinator(db, project_id, session_id)?.is_some() {
        return Err(DomainStateError::bad_request(
            "That session is already an orchestrator.",
        ));
    }
    if let Some(thread) = read_thread(db, project_id, session_id)? {
        let coordinator = repository
            .get_session(
                &thread.coordinator_project_id,
                &thread.coordinator_session_id,
            )?
            .map(|coordinator| session_title_of(&coordinator))
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "another orchestrator".to_string());
        return Err(DomainStateError::bad_request(format!(
            "That session is a thread of \"{coordinator}\". A thread reports to its orchestrator, so it cannot also lead threads of its own; make another session the orchestrator instead."
        )));
    }
    if session.get("kind").and_then(Value::as_str) != Some("agent") {
        return Err(DomainStateError::bad_request(
            "Only an agent session can become an orchestrator.",
        ));
    }
    if crate::agentbox::is_agentbox_session(session) {
        return Err(DomainStateError::bad_request(
            "An orchestrator runs on this computer, so a session in a box cannot become one.",
        ));
    }
    // A draft can still switch its agent before its first prompt, and the role would not follow.
    if crate::agents::session_is_draft(session) {
        return Err(DomainStateError::bad_request(
            "This session has not started its conversation yet. Send it a first message first, or start a New Orchestrator instead.",
        ));
    }
    Ok(())
}

/// True when a saved command already carries the coordinator role flag of its family.
fn command_has_coordinator_role(command: &str, family: &str) -> bool {
    match family {
        "claude" => command.contains("--append-system-prompt-file"),
        "codex" => command.contains("developer_instructions="),
        _ => false,
    }
}

/// The chat message that hands a promoted session its role. It carries the whole playbook, so it
/// works without the CLI, and stays in the conversation until the next resume adds the role as a
/// system prompt. A session handed its role by a queued line (Empryo's `/agent`) already runs by
/// the playbook when it reads this, so its message leaves the playbook out.
pub fn playbook_message(goal: &str, role_in_system_prompt: bool) -> String {
    let mut message = String::from(if role_in_system_prompt {
        "Ghostex: this session is now an orchestrator. You keep this conversation; from now on you work by the orchestrator playbook, which is now part of your instructions."
    } else {
        "Ghostex: this session is now an orchestrator. You keep this conversation; from now on you work by the orchestrator playbook below."
    });
    if !goal.trim().is_empty() {
        message.push_str(&format!("\n\nYour goal: {}", goal.trim()));
    }
    if !role_in_system_prompt {
        message.push_str(
            "\n\nGhostex adds this playbook to your system prompt the next time this session starts or resumes. Until then, re-read it with `ghostex orchestrator guide` whenever your context was compacted.",
        );
    }
    message.push_str(
        "\n\nSessions you started earlier in this conversation are not your threads yet. If there are any, list them for the user and, once they confirm, adopt each one with `ghostex orchestrator link <session ref> --task \"<what it works on>\"`.\
\n\nNow run `ghostex orchestrator status`, then tell the user in a line or two that you are their orchestrator.",
    );
    if !role_in_system_prompt {
        message.push_str("\n\n---\n\n");
        message.push_str(COORDINATOR_ROLE_PROMPT.trim());
    }
    message
}
