use super::{
    arguments::Arguments,
    identity::{self, text},
};
use crate::ghostex_cli::{
    rpc::{call_gxserver_rpc, CliError, CliResult},
    selector, sessions,
};
use serde_json::{json, Value};

pub(super) fn types(args: &Arguments) -> CliResult<Value> {
    let hud = call_gxserver_rpc("/api/readSidebarHud", &json!({}), &args.flags)?;
    let agents = hud
        .get("agents")
        .and_then(Value::as_array)
        .ok_or_else(|| CliError::Other("The server did not return agent types.".into()))?;
    Ok(
        json!({"ok": true, "agents": agents.iter().map(|agent| json!({
        "agentId": agent["agentId"], "name": agent["name"], "command": agent["command"],
    })).collect::<Vec<_>>()}),
    )
}

/// CDXC:Cli 2026-09-17 DECISION:
/// User: agents help must explain how to choose and create agents and close other sessions. Reuse configured launcher types and the existing provider startup and close operations.
pub(super) fn create(args: &Arguments) -> CliResult<Value> {
    let body = match &args.body_file {
        Some(path) => Some(std::fs::read_to_string(path).map_err(|error| {
            CliError::Other(format!("Could not read task file {path}: {error}"))
        })?),
        None => args.task.clone(),
    };
    if body.as_ref().is_some_and(|body| body.trim().is_empty()) {
        return Err(CliError::Other("Task must not be empty.".into()));
    }
    let caller = if args.project_id.is_none() || body.is_some() {
        Some(identity::caller()?)
    } else {
        None
    };
    let flags = if args.project_id.is_none() {
        identity::inventory_flags(&args.flags, text(caller.as_ref().unwrap(), "globalRef"))?
    } else {
        args.flags.clone()
    };
    let project = args
        .project_id
        .as_deref()
        .unwrap_or_else(|| text(caller.as_ref().unwrap(), "projectId"));
    let message = body
        .as_ref()
        .map(|body| identity::message(caller.as_ref().unwrap(), body));
    if message
        .as_ref()
        .is_some_and(|text| text.len() > crate::zmx::GXSERVER_ZMX_SEND_TEXT_LIMIT_BYTES)
    {
        return Err(CliError::Other(
            "Task including sender header exceeds the send limit.".into(),
        ));
    }
    let agent_id = &args.positional[0];
    let hud = call_gxserver_rpc("/api/readSidebarHud", &json!({}), &flags)?;
    if !hud["agents"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| text(row, "agentId") == agent_id))
    {
        return Err(CliError::Other(format!("Unknown or hidden agent type: {agent_id}. Run ghostex agents types and use an agentId from that list.")));
    }
    let mut payload = json!({"projectId": project, "agentId": agent_id});
    if let Some(title) = &args.title {
        payload["title"] = json!(title);
    }
    let created = call_gxserver_rpc("/api/createAgentSession", &payload, &flags)?;
    let session = created.get("session").ok_or_else(|| {
        CliError::Other(
            "Create response has no session. Inspect agents list before retrying.".into(),
        )
    })?;
    let reference = text(session, "globalRef");
    if text(session, "sessionId").is_empty() || reference.is_empty() {
        return Err(CliError::Other(
            "Create response lacks a session reference. Inspect agents list before retrying."
                .into(),
        ));
    }
    // Retain the created identity if startup fails so a retry does not create a duplicate.
    let provider = call_gxserver_rpc("/api/startSessionProvider", &json!({
        "globalRef": reference, "projectId": session["projectId"], "sessionId": session["sessionId"],
    }), &flags).map_err(|error| CliError::Other(format!("Created {reference}, but provider startup failed or is uncertain: {error}. Inspect this session before retrying; do not create another session.")))?;
    let live_session = provider
        .get("session")
        .filter(|value| value.is_object())
        .unwrap_or(session);
    let mut result = json!({"ok": true, "status": "created", "globalRef": reference, "session": identity::summary(live_session)});
    // CDXC:Coordinators 2026-09-30 WHY: a coordinator that starts a worker the `$ghostex-agents` way still gets it as a tracked thread; for any other caller the link is a no-op.
    if let Some(caller) = caller.clone().or_else(|| identity::caller().ok()) {
        let linked = call_gxserver_rpc(
            "/api/linkCoordinatorThread",
            &json!({
                "coordinatorProjectId": caller["projectId"], "coordinatorSessionId": caller["sessionId"],
                "projectId": session["projectId"], "sessionId": session["sessionId"],
                "task": body.as_deref().unwrap_or_default(), "onlyIfCoordinator": true,
                "pendingMessage": body.as_deref().unwrap_or_default(),
            }),
            &flags,
        );
        if linked.is_ok_and(|linked| linked["linked"] == json!(true)) {
            result["coordinatorThread"] = json!(true);
        }
    }
    if let Some(message) = message {
        let receipt = call_gxserver_rpc("/api/queueSessionChatPrompt", &json!({
            "globalRef": reference, "projectId": session["projectId"], "sessionId": session["sessionId"],
            "text": message, "startupSend": true, "sendRequestId": uuid::Uuid::new_v4().to_string(),
        }), &flags).map_err(|error| CliError::Other(format!("Created {reference}, but task delivery failed or is uncertain: {error}. Inspect this session and its queue; do not create another session to retry.")))?;
        result["taskStatus"] = json!("queued");
        // The sender's chat card shows the task from here (see `message` in delivery.rs).
        result["task"] = json!(body);
        result["receipt"] = super::delivery::receipt(&receipt);
    }
    Ok(result)
}

pub(super) fn close(args: &Arguments) -> CliResult<Value> {
    let reference = &args.positional[0];
    let flags = identity::inventory_flags(&args.flags, reference)?;
    let rows = sessions::fetch_session_list(&flags, false)?;
    let recipient = selector::resolve_one_listed_session(reference, &rows, &flags)?;
    if !identity::is_agent(&recipient) {
        return Err(CliError::Other(
            "The target is not an agent session.".into(),
        ));
    }
    let result = call_gxserver_rpc(
        "/api/killSession",
        &json!({
            "globalRef": recipient["globalRef"], "projectId": recipient["projectId"], "sessionId": recipient["sessionId"],
        }),
        &flags,
    )?;
    Ok(
        json!({"ok": true, "status": "closed", "session": identity::summary(&recipient), "receipt": super::delivery::receipt(&result)}),
    )
}
