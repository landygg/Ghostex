use serde_json::{json, Value};

use super::command::{
    flag_text, launch_settings_for, read_view, resolve_session, resolve_thread_session,
    server_flags, target_coordinator, text_or_file,
};
use super::delivery::{confirm_brief_started, BriefOutcome};
use crate::coordinators::{agent_message, thread_brief, BriefContext, MessageSender};
use crate::ghostex_cli::{
    agents,
    args::ParsedArgs,
    output::print_json,
    rpc::{call_gxserver_rpc, CliError, CliResult},
};

fn sender(row: &Value) -> MessageSender {
    let summary = agents::summary(row);
    MessageSender {
        agent_name: agents::text(&summary, "agentName").to_string(),
        title: agents::text(&summary, "title").to_string(),
        session_id: agents::text(&summary, "sessionId").to_string(),
        agent_id: agents::text(&summary, "agentId").to_string(),
        agent_session_id: agents::text(&summary, "agentSessionId").to_string(),
        global_ref: agents::text(&summary, "globalRef").to_string(),
    }
}

/// CDXC:Coordinators 2026-09-30 WHY:
/// The task reaches the thread as a message from the coordinator (so its chat shows who sent it and `Reply to` names the coordinator), wrapped with the goal, standing instructions, memory and reporting rules. The thread is linked before its agent starts, so the supervisor sees its first turn from the beginning.
pub(super) fn start_thread(parsed: &ParsedArgs) -> CliResult<()> {
    let title = flag_text(&parsed.flags, "title")
        .ok_or_else(|| CliError::Other("start-thread needs --title \"<3 to 6 words>\".".into()))?;
    let task = text_or_file(parsed, "task")?
        .filter(|task| !task.trim().is_empty())
        .ok_or_else(|| {
            CliError::Other("start-thread needs --task \"<brief>\" or --body-file <path>.".into())
        })?;
    let (coordinator_row, flags) = target_coordinator(parsed)?;
    let view = read_view(&coordinator_row, &flags)?;
    let coordinator = &view["coordinator"];
    let notes = coordinator["memory"]
        .as_array()
        .map(|notes| {
            notes
                .iter()
                .map(|note| agents::text(note, "text").to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let brief = thread_brief(
        &BriefContext {
            goal: agents::text(coordinator, "goal"),
            instructions: agents::text(coordinator, "instructions"),
            notes: &notes,
            worktree: parsed.flags.truthy("worktree"),
        },
        &task,
    );
    let mut sender_row = coordinator_row.clone();
    agents::resolve_names(std::slice::from_mut(&mut sender_row), &flags);
    let message = agent_message(&sender(&sender_row), &brief);
    if message.len() > crate::zmx::GXSERVER_ZMX_SEND_TEXT_LIMIT_BYTES {
        return Err(CliError::Other(
            "The task with its standing instructions is too long to send. Put the details in a file and point the thread at it.".into(),
        ));
    }
    let project_id = flag_text(&parsed.flags, "projectId")
        .unwrap_or_else(|| agents::text(coordinator, "projectId").to_string());
    let agent_id = flag_text(&parsed.flags, "agent")
        .unwrap_or_else(|| agents::text(coordinator, "agentId").to_string());
    let hud = call_gxserver_rpc("/api/readSidebarHud", &json!({}), &flags)?;
    let agent_rows = hud["agents"].as_array().cloned().unwrap_or_default();
    if !agent_rows
        .iter()
        .any(|row| agents::text(row, "agentId") == agent_id)
    {
        return Err(CliError::Other(format!(
            "Unknown or hidden agent type: {agent_id}. Run ghostex agents types and use an agentId from that list."
        )));
    }
    let mut create = json!({
        "projectId": project_id,
        "agentId": agent_id,
        "launchSettings": launch_settings_for(&agent_rows, &agent_id),
        "title": title,
    });
    if let Some(model) = flag_text(&parsed.flags, "model") {
        create["agentModel"] = json!(model);
    }
    if let Some(effort) = flag_text(&parsed.flags, "effort") {
        create["agentEffort"] = json!(effort);
    }
    let worktree = parsed.flags.truthy("worktree");
    let (session_id, branch, worktree_path) = if worktree {
        if let Some(base) = flag_text(&parsed.flags, "baseBranch") {
            create["baseBranch"] = json!(base);
        }
        let created = call_gxserver_rpc("/api/createWorktreeSession", &create, &flags)?;
        (
            agents::text(&created, "sessionId").to_string(),
            Some(agents::text(&created, "branch").to_string()),
            Some(agents::text(&created, "worktreePath").to_string()),
        )
    } else {
        let created = call_gxserver_rpc("/api/createAgentSession", &create, &flags)?;
        (
            agents::text(&created["session"], "sessionId").to_string(),
            None,
            None,
        )
    };
    if session_id.is_empty() {
        return Err(CliError::Other(
            "The create response has no session id. Run ghostex orchestrator status before retrying.".into(),
        ));
    }
    let linked = call_gxserver_rpc(
        "/api/linkCoordinatorThread",
        &json!({
            "coordinatorProjectId": coordinator["projectId"],
            "coordinatorSessionId": coordinator["sessionId"],
            "projectId": project_id,
            "sessionId": session_id,
            "task": task.trim(),
            "pendingMessage": task.trim(),
        }),
        &flags,
    )?;
    let reference = agents::text(&linked, "globalRef").to_string();
    if !worktree {
        call_gxserver_rpc(
            "/api/startSessionProvider",
            &json!({"globalRef": reference, "projectId": project_id, "sessionId": session_id}),
            &flags,
        )
        .map_err(|error| {
            CliError::Other(format!(
                "Started thread {reference}, but its agent did not start: {error}. Check ghostex orchestrator status; do not start another."
            ))
        })?;
    }
    let queued = call_gxserver_rpc(
        "/api/queueSessionChatPrompt",
        &json!({
            "globalRef": reference, "projectId": project_id, "sessionId": session_id,
            "text": message, "startupSend": true, "sendRequestId": uuid::Uuid::new_v4().to_string(),
        }),
        &flags,
    )
    .map_err(|error| {
        CliError::Other(format!(
            "Started thread {reference}, but its task was not delivered: {error}. Send it with ghostex agents send {reference} --body-file <file>; do not start another."
        ))
    })?;
    let thread =
        json!({ "globalRef": reference, "projectId": project_id, "sessionId": session_id });
    let outcome = confirm_brief_started(
        &thread,
        &task,
        queued.pointer("/prompt/id").and_then(Value::as_str),
        &flags,
    )?;
    if matches!(outcome, BriefOutcome::Started) {
        agents::confirm_coordinator_delivery(coordinator, &thread, &flags);
    }
    let (status, note) = match &outcome {
        BriefOutcome::Started => ("started", "Its transcript shows the brief: it is working on it.".to_string()),
        BriefOutcome::Pending(reason) => ("pending", format!("Its brief has not started yet: {reason}. Ghostex keeps watching and reports to you if it never arrives; do not start another thread or resend the brief.")),
    };
    let mut result = json!({
        "ok": true,
        "status": status,
        "note": note,
        "thread": { "globalRef": reference, "projectId": project_id, "sessionId": session_id, "title": title },
    });
    if let Some(branch) = branch.filter(|branch| !branch.is_empty()) {
        result["thread"]["branch"] = json!(branch);
    }
    if let Some(path) = worktree_path.filter(|path| !path.is_empty()) {
        result["thread"]["worktreePath"] = json!(path);
    }
    if parsed.flags.truthy("json") {
        print_json(&result);
    } else {
        let where_ = result["thread"]["branch"]
            .as_str()
            .map(|branch| format!(" on branch {branch}"))
            .unwrap_or_default();
        match status {
            "started" => println!("Started thread \"{title}\" ({reference}){where_}. {note}"),
            _ => println!("Created thread \"{title}\" ({reference}){where_}, pending: {note}"),
        }
        println!("Its report comes back to you when it finishes a turn; end your turn now instead of waiting.");
    }
    Ok(())
}

pub(super) fn link(parsed: &ParsedArgs) -> CliResult<()> {
    let reference = parsed.rest.first().cloned().ok_or_else(|| {
        CliError::Other("Usage: ghostex orchestrator link <session-ref> [--task <text>]".into())
    })?;
    let (coordinator, _) = target_coordinator(parsed)?;
    let (thread, flags) = resolve_session(&reference, &server_flags(&parsed.flags))?;
    let result = call_gxserver_rpc(
        "/api/linkCoordinatorThread",
        &json!({
            "coordinatorProjectId": coordinator["projectId"],
            "coordinatorSessionId": coordinator["sessionId"],
            "projectId": thread["projectId"],
            "sessionId": thread["sessionId"],
            "task": flag_text(&parsed.flags, "task").unwrap_or_default(),
        }),
        &flags,
    )?;
    if parsed.flags.truthy("json") {
        print_json(&result);
    } else {
        println!(
            "Linked {} as a thread of {}.",
            agents::text(&thread, "globalRef"),
            agents::text(&coordinator, "title")
        );
    }
    Ok(())
}

/// `resolve` marks a thread done and closes its session (`--keep-open` parks it instead);
/// `reopen` resumes a closed thread's session, then puts it back in its coordinator's tree.
pub(super) fn set_resolved(parsed: &ParsedArgs, resolved: bool) -> CliResult<()> {
    let verb = if resolved { "resolve" } else { "reopen" };
    let reference = parsed.rest.first().cloned().ok_or_else(|| {
        CliError::Other(format!("Usage: ghostex orchestrator {verb} <thread-ref>"))
    })?;
    let (thread, flags) = resolve_thread_session(&reference, &server_flags(&parsed.flags))?;
    let global_ref = agents::text(&thread, "globalRef").to_string();
    let session = json!({
        "globalRef": global_ref, "projectId": thread["projectId"], "sessionId": thread["sessionId"],
    });
    let closed = agents::text(&thread, "lifecycleState") == "stopped";
    let close_session = resolved && !parsed.flags.truthy("keepOpen");
    // Woken before it is reopened: an open thread whose session is still closed is reported to the
    // coordinator as closed and marked done again.
    let resumed = !resolved && closed;
    if resumed {
        call_gxserver_rpc("/api/wakeSession", &session, &flags).map_err(|error| {
            CliError::Other(format!(
                "Could not resume {global_ref}: {error}. It was not reopened."
            ))
        })?;
    }
    let result = call_gxserver_rpc(
        "/api/setCoordinatorThreadResolved",
        &json!({
            "projectId": thread["projectId"],
            "sessionId": thread["sessionId"],
            "resolved": resolved,
            "closeSession": close_session,
        }),
        &flags,
    )?;
    if close_session && !closed {
        call_gxserver_rpc("/api/killSession", &session, &flags).map_err(|error| {
            CliError::Other(format!(
                "Marked {global_ref} done, but its session did not close: {error}. Close it with ghostex agents close {global_ref}."
            ))
        })?;
    }
    if parsed.flags.truthy("json") {
        let mut result = result;
        result["sessionClosed"] = json!(close_session);
        result["sessionResumed"] = json!(resumed);
        print_json(&result);
    } else if close_session {
        println!(
            "Marked {global_ref} done and closed its session. `ghostex orchestrator reopen {global_ref}` or a message resumes the same conversation."
        );
    } else if resolved {
        println!("Marked {global_ref} done; its session stays open (parked).");
    } else if resumed {
        println!("Reopened {global_ref} and resumed its session.");
    } else {
        println!("Reopened {global_ref}.");
    }
    Ok(())
}
