use serde_json::{json, Map, Value};

use super::*;
use crate::domain::DomainStateError;
use crate::session_status::normalize_agent_activity_value;
use rusqlite::Connection;

pub(crate) fn build_project_agent_launch_plan(
    project: &Value,
    agent_id: &str,
    agent_session_id: Option<String>,
    settings: &Map<String, Value>,
) -> Value {
    let agent_config = resolve_project_agent_config(project, agent_id, None);
    build_agent_launch_plan(AgentLaunchInput {
        accept_all_mode: read_text_from_map(&agent_config, "acceptAllMode"),
        agent_id: agent_id.to_string(),
        agent_session_id,
        command: read_text_from_map(&agent_config, "command"),
        delayed_send_deadline_at: None,
        first_user_message: None,
        global_accept_all_enabled: settings
            .get("agentAcceptAllEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        icon: read_text_from_map(&agent_config, "icon"),
    })
}

/*
CDXC:ServerApi 2026-06-22-05:39:
`createAgentSession` is a CRUD endpoint, but its durable row is shaped by the same project agent config and persisted agent settings as TypeScript gxserver. Build the launch plan before repository insertion so listSessions/readProjectStatus return the same launchSettings and runtimeSettings immediately after creation.
*/
pub(crate) fn create_agent_session_params_for_project(
    db: &Connection,
    project: &Value,
    params: &Map<String, Value>,
) -> Result<Map<String, Value>, DomainStateError> {
    let settings = read_agent_settings(db)?;
    let project_id = project
        .get("projectId")
        .and_then(Value::as_str)
        .ok_or_else(|| DomainStateError::corrupt_state("Project missing projectId."))?;
    let agent_id =
        read_text(params, "agentId").unwrap_or_else(|| DEFAULT_PROMPT_AGENT_ID.to_string());
    let mut launch_settings = params
        .get("launchSettings")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut runtime_settings = params
        .get("runtimeSettings")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let agent_config = resolve_project_agent_config(project, &agent_id, Some(&launch_settings));
    let agent_icon = read_text_from_map(&agent_config, "icon")
        .or_else(|| read_text_from_map(&launch_settings, "icon"));
    let configured_command = read_text_from_map(&agent_config, "command")
        .or_else(|| read_text_from_map(&launch_settings, "agentCommand"));
    let (configured_command, empryo_session_id) = apply_requested_agent_model(
        project,
        &agent_id,
        &agent_config,
        &launch_settings,
        params,
        configured_command,
        &mut runtime_settings,
    )?;
    let agentbox_provider = crate::agentbox::requested_agentbox_provider(params)?;
    // Only a box create writes a box record; a client cannot make a session a box session.
    if agentbox_provider.is_none() {
        runtime_settings.remove("agentbox");
    }
    let agentbox_family = match agentbox_provider {
        Some(_) => {
            if crate::coordinators::coordinator_create_request(params)?.is_some() {
                return Err(DomainStateError::bad_request(
                    "An orchestrator runs on this computer. Start it without a box.",
                ));
            }
            Some(crate::agentbox::box_agent_family(
                resume_agent_family_id(Some(agent_id.clone()), &agent_config, &launch_settings)
                    .as_deref(),
            )?)
        }
        None => None,
    };
    let configured_command = if agentbox_provider.is_none() {
        pin_remembered_agent_model(
            &agent_id,
            &agent_config,
            &launch_settings,
            params,
            configured_command,
            &mut runtime_settings,
        )?
    } else {
        configured_command
    };
    let configured_command = apply_coordinator_role(
        &agent_id,
        &agent_config,
        &launch_settings,
        params,
        configured_command,
    )?;
    let account_command = if agentbox_provider.is_some() {
        None
    } else {
        if let Some(command) = configured_command.as_ref() {
            runtime_settings
                .entry("accountBaseCommand")
                .or_insert(json!(command));
        }
        let workspace_account_id = crate::workspaces::project_claude_account_id(db, project)?;
        crate::accounts::launch::apply_new_session(
            db,
            &agent_id,
            agent_icon.as_deref(),
            workspace_account_id.as_deref(),
            &mut runtime_settings,
        )?
    };
    /*
    CDXC:SessionIdentity 2026-09-30 WHY: A create that names no agent Ghostex knows and only carries a command (Find's resume sends `os-integration-terminal` with `claude --resume <id>`, as `ghostex://terminal` does) locked the row to that placeholder id, so `launch_agent_mismatch` refused every hook the agent sent and only the ~20s live-process scan could name it. Chat View cannot open before that, so a session resumed from Find sat in the terminal (observed 2026-09-30, session S90:P3lv0:G41ci: its SessionStart hook arrived 1.5s after launch and was dropped). The agent the command starts is the session's agent from creation; the launch command and account are still resolved under the requested id, so the command runs exactly as given.
    */
    // A new local Pi session runs on an id Ghostex chose (CDXC:SessionIdentity in pi_session_id.rs).
    let pi_session_id = (agentbox_provider.is_none()
        && read_text_from_map(&runtime_settings, "agentSessionId").is_none()
        && resume_agent_family_id(Some(agent_id.clone()), &agent_config, &launch_settings)
            .as_deref()
            == Some("pi"))
    .then(|| {
        let base = configured_command
            .clone()
            .or_else(|| default_agent_command("pi").map(str::to_string))
            .unwrap_or_else(|| "pi".to_string());
        super::pi_session_id::mint_pi_session_id(&base)
    })
    .flatten();
    if let Some(id) = pi_session_id.as_deref() {
        runtime_settings.insert("agentSessionId".to_string(), json!(id));
    }
    /*
    CDXC:SessionIdentity 2026-10-07 WHY:
    A plain Empryo launch ran bare `empryo`, which reopens the folder's latest session: the row never learned an agent session id (its chat had no transcript), and a second launch in the same repo shared the first one's session (seen live on 3.9.1-beta). Every new local Empryo session gets a seeded folder of its own, as a launch model's does, on the folder's current default model. Resume and fork arrive with their id and keep it.
    */
    let empryo_session_id = match empryo_session_id {
        None if agentbox_provider.is_none()
            && read_text_from_map(&runtime_settings, "agentSessionId").is_none()
            && resume_agent_family_id(Some(agent_id.clone()), &agent_config, &launch_settings)
                .as_deref()
                == Some("empryo") =>
        {
            let id = crate::session_chat_empryo_launch_selection::seed_empryo_launch_session(
                &empryo_cwd(params, project)?,
                None,
                &mut runtime_settings,
            )?;
            Some(id)
        }
        seeded => seeded,
    };
    let session_agent_id = (agent_icon.is_none()
        && !agent_id.starts_with("custom-")
        && default_agent_command(&agent_id).is_none()
        && resolve_project_agent_config(project, &agent_id, None).is_empty())
    .then(|| {
        configured_command
            .as_deref()
            .and_then(infer_agent_id_from_command_executable)
    })
    .flatten()
    .unwrap_or_else(|| agent_id.clone());
    let launch_plan = match (agentbox_provider.as_deref(), agentbox_family.as_deref()) {
        (Some(provider), Some(family)) => {
            let project_path = read_text(params, "cwd")
                .or_else(|| read_text_value(project, "path"))
                .unwrap_or_default();
            let claude_sign_in_first = crate::accounts::launch::home()
                .is_ok_and(|home| !crate::agentbox::claude_signed_in_for_boxes(&home));
            crate::agentbox::prepare_box_launch(
                provider,
                family,
                &project_path,
                configured_command.as_deref(),
                claude_sign_in_first,
                &mut runtime_settings,
            )
            .launch_plan
        }
        _ => {
            let mut plan = build_agent_launch_plan(AgentLaunchInput {
                accept_all_mode: read_text_from_map(&agent_config, "acceptAllMode")
                    .or_else(|| read_text_from_map(&launch_settings, "acceptAllMode")),
                agent_id: agent_id.clone(),
                agent_session_id: read_text_from_map(&runtime_settings, "agentSessionId"),
                command: account_command.or(configured_command),
                delayed_send_deadline_at: read_text_from_map(
                    &launch_settings,
                    "delayedSendDeadlineAt",
                ),
                first_user_message: read_text_from_map(&runtime_settings, "firstUserMessage"),
                global_accept_all_enabled: settings
                    .get("agentAcceptAllEnabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                icon: agent_icon.clone(),
            });
            // Only the command this launch runs names the id; `agentCommand` stays the base that
            // resume, fork and account wrapping rebuild from. Pi's id is minted here; Empryo's names
            // the session folder seeded for a launch model (session_chat_empryo_launch_selection.rs).
            let session_command = |command: &str| match (&pi_session_id, &empryo_session_id) {
                (Some(id), _) => Some(super::pi_session_id::with_pi_session_id(command, id)),
                (None, Some(id)) => Some(format!("{} --session {id}", command.trim_end())),
                (None, None) => None,
            };
            if let Some(plan) = plan.as_object_mut() {
                if let Some(command) = plan
                    .get("command")
                    .and_then(Value::as_str)
                    .filter(|command| !command.trim().is_empty())
                    .and_then(session_command)
                {
                    plan.insert(
                        "startupText".to_string(),
                        Value::String(as_atuin_ignored_shell_input(&command)),
                    );
                    plan.insert("command".to_string(), Value::String(command));
                }
            }
            plan
        }
    };
    let launch_plan_object = launch_plan.as_object().cloned().unwrap_or_default();
    let has_launch_command = launch_plan_object
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    /*
    CDXC:AgentLauncher 2026-09-25 WHY: Every agent session gxserver creates must have a command to launch, whether or not the client sends `requireLaunchCommand`. A commandless row can never start: `startSessionProvider` declines it, so every attach surfaced as "gxserver did not confirm the zmx provider exists" (`ghostex create-agent hermes` resolved no command because the built-in id is `hermes-agent`). This supersedes the 2026-06-24 opt-in guard, which only remote GPUI starts, drafts, automations and board work set.
    */
    if !has_launch_command {
        return Err(DomainStateError::bad_request(format!(
            "Ghostex has no launch command for agent \"{agent_id}\", so it did not create the session. Use a built-in agent id (for example claude, codex or hermes-agent) or one configured for this project."
        )));
    }
    /*
    CDXC:SessionStatus 2026-09-27 WHY: A new session starts "working" only when its launch submits a first prompt (`firstUserMessage`). The launch plan's startup text is the agent command alone and every agent launch has one, so keying on it started every session "working". Claude and Codex hide that by projection and settle it with their own titles and hooks, but an agent that fires nothing before its first turn (Hermes runs `on_session_start` from its first conversation turn) read "working" at an empty prompt until the user typed.
    */
    // A box session starts idle: its box takes a minute to come up, and the box activity poller
    // reports working once the agent's screen shows it (see agentbox/activity.rs).
    let launch_submits_prompt = agentbox_provider.is_none()
        && read_text_from_map(&launch_plan_object, "firstUserMessage").is_some();
    let agent_activity = if runtime_settings.get("agentActivity").is_some() {
        normalize_agent_activity_value(runtime_settings.get("agentActivity"), "idle")
    } else {
        default_activity(
            Some(&session_agent_id),
            launch_submits_prompt.then_some("working"),
        )
    };
    runtime_settings.insert("agentActivity".to_string(), agent_activity);
    runtime_settings.insert(
        "agentCommand".to_string(),
        Value::String(
            launch_plan_object
                .get("agentCommand")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
    );
    runtime_settings.insert(
        "launchAgentId".to_string(),
        Value::String(session_agent_id.clone()),
    );
    if let Some(first_user_message) = launch_plan_object
        .get("firstUserMessage")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        runtime_settings.insert(
            "firstUserMessage".to_string(),
            Value::String(first_user_message.to_string()),
        );
    }
    /*
    CDXC:Drafts 2026-08-20:
    Arm the draft here, at creation, so only sessions that were created with one
    can ever consume it. A draft that is missing or blank leaves no marker and
    no key behind.
    */
    match runtime_settings
        .get(FIRST_USER_INPUT_DRAFT_KEY)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
    {
        Some(draft) => {
            runtime_settings.insert(FIRST_USER_INPUT_DRAFT_KEY.to_string(), Value::String(draft));
            runtime_settings.insert(
                FIRST_USER_INPUT_DRAFT_STATUS_KEY.to_string(),
                json!("pending"),
            );
        }
        None => {
            runtime_settings.remove(FIRST_USER_INPUT_DRAFT_KEY);
            runtime_settings.remove(FIRST_USER_INPUT_DRAFT_STATUS_KEY);
        }
    }
    /*
    CDXC:Drafts 2026-08-28:
    Arm the draft marker in the same place the first-input draft above is armed,
    and for the same reason: only a session created as a draft can ever be one.
    See `agents/drafts.rs` for what the marker means and where it is removed.
    */
    apply_draft_session_create_param(params, &mut runtime_settings);
    crate::empty_session_cleanup::apply_new_session_marker(params, &mut runtime_settings);

    let mut runtime_relevant = Map::new();
    if let Some(deadline_at) = launch_plan_object
        .get("delayedSend")
        .and_then(Value::as_object)
        .and_then(|delayed| delayed.get("deadlineAt"))
        .and_then(Value::as_str)
    {
        runtime_relevant.insert(
            "delayedSendDeadlineAt".to_string(),
            Value::String(deadline_at.to_string()),
        );
    }
    runtime_relevant.insert(
        "queueProviderStartupText".to_string(),
        Value::Bool(
            launch_plan_object
                .get("startupTextDisposition")
                .and_then(Value::as_str)
                == Some("queueAfterTerminalReady"),
        ),
    );
    if let Some(agent_icon) = agent_icon {
        launch_settings.insert("icon".to_string(), Value::String(agent_icon));
    }
    launch_settings.insert("agentLaunchPlan".to_string(), launch_plan);
    launch_settings.insert(
        "runtimeRelevant".to_string(),
        Value::Object(runtime_relevant),
    );

    let mut normalized = params.clone();
    normalized.insert("agentId".to_string(), Value::String(session_agent_id));
    normalized.insert("kind".to_string(), Value::String("agent".to_string()));
    normalized.insert("launchSettings".to_string(), Value::Object(launch_settings));
    normalized.insert(
        "projectId".to_string(),
        Value::String(project_id.to_string()),
    );
    normalized
        .entry("lifecycleState".to_string())
        .or_insert_with(|| Value::String("running".to_string()));
    if read_text(&normalized, "title").is_none() {
        let default_title =
            project_agent_session_default_title(project, &Value::Object(normalized.clone()));
        normalized.insert("title".to_string(), Value::String(default_title));
        /*
        CDXC:SessionTitles 2026-09-09 WHY:
        Custom-agent defaults previously used configured names such as "Claude 71 Session", which the first-prompt auto-title gates mistook for user-chosen titles.
        Stamp every launcher default as a placeholder so auto-naming depends on its source rather than a list of display names.
        */
        if !runtime_settings
            .get("titleSource")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            runtime_settings.insert(
                "titleSource".to_string(),
                Value::String("placeholder".to_string()),
            );
        }
    }
    normalized.insert(
        "runtimeSettings".to_string(),
        Value::Object(runtime_settings),
    );
    Ok(normalized)
}

/// CDXC:SessionTitles 2026-09-09 DECISION:
/// User: empty Claude/Codex sessions show "∗ Claude Session" / "∗ Codex Session", never the custom agent name or ID created for an account.
/// Identity updates used the custom configuration ID as a display name and overwrote the launcher's placeholder title.
/// SEE-ALSO: agents/identity/, agents/session_state_ingest.rs, agents/drafts.rs, presentation/session_attributes.rs.
pub(crate) fn project_agent_session_default_title(project: &Value, session: &Value) -> String {
    let agent_id = read_text_value(session, "agentId");
    let family = session_agent_family_id(project, session);
    if matches!(family.as_deref(), Some("claude" | "codex")) {
        return create_agent_session_default_title(None, family.as_deref());
    }
    let agent_config = resolve_project_agent_config(
        project,
        agent_id.as_deref().unwrap_or_default(),
        Some(&object_field(session, "launchSettings")),
    );
    create_agent_session_default_title(
        read_text_from_map(&agent_config, "name").as_deref(),
        agent_id.as_deref(),
    )
}

/// The folder an Empryo create starts in: the requested `cwd`, else the project's.
fn empryo_cwd(
    params: &Map<String, Value>,
    project: &Value,
) -> Result<std::path::PathBuf, DomainStateError> {
    read_text(params, "cwd")
        .or_else(|| read_text_value(project, "path"))
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            DomainStateError::bad_request("The project has no folder to start Empryo in.")
        })
}

/// CDXC:AgentProviders 2026-10-06 DECISION:
/// User: "yes, Ghostex chooses Pi's model and thinking level at launch, like Claude and Codex." This extends the 2026-09-17 decision (an agent spawning another agent sets that worker's model and effort for the session only, and a resumed worker keeps them) from Claude and Codex to Pi, whose model is `provider/id` (`--model`) and whose effort is its thinking level (`--thinking`).
/// Typing `/model` or `/effort` into Claude Code saves the choice as the default for every new session, so the choice travels as launch flags instead.
/// The flags live in the session's saved base command, which resume, fork and account wrapping all rebuild from.
/// Sven extended it to Empryo on 2026-10-06 (Empryo harness spec). Empryo's terminal app ignores launch flags, so the model goes into a session folder Ghostex seeds and launches with `--session`, and the effort is typed with `/effort` once Empryo is up; neither changes Empryo's default. A resume keeps the model (it reopens that tab), but Empryo 3.9.0-beta drops a tab's effort when it resumes it (seen 2026-10-06), so the effort lasts until the session restarts (session_chat_empryo_launch_selection.rs).
/// SEE-ALSO: server/src/ghostex_cli/actions/create.rs (create-agent), server/src/ghostex_cli/board.rs and server/src/board_start_work.rs (board start-work).
fn apply_requested_agent_model(
    project: &Value,
    agent_id: &str,
    agent_config: &Map<String, Value>,
    launch_settings: &Map<String, Value>,
    params: &Map<String, Value>,
    command: Option<String>,
    runtime_settings: &mut Map<String, Value>,
) -> Result<(Option<String>, Option<String>), DomainStateError> {
    let model = requested_agent_model_option(params, "agentModel")?;
    let effort = requested_agent_model_option(params, "agentEffort")?;
    if model.is_none() && effort.is_none() {
        return Ok((command, None));
    }
    let family = resume_agent_family_id(Some(agent_id.to_string()), agent_config, launch_settings)
        .filter(|family| {
            matches!(
                family.as_str(),
                "claude" | "codex" | "pi" | "zcode" | "empryo"
            )
        })
        .ok_or_else(|| {
            DomainStateError::bad_request(
                "A launch model or effort can only be set for Claude, Codex, Pi, ZCode and Empryo agents.",
            )
        })?;
    if family == "empryo" {
        use crate::session_chat_empryo_launch_selection as launch;
        let model = launch::empryo_launch_model(model.as_deref())?;
        let session_id = launch::seed_empryo_launch_session(
            &empryo_cwd(params, project)?,
            Some(model),
            runtime_settings,
        )?;
        if let Some(effort) = effort.as_deref() {
            launch::record_empryo_launch_effort(runtime_settings, model, effort);
        }
        // Only the launch plan's command names the seeded session (as Pi's `--session-id`), so
        // the saved base command that resume and fork rebuild from stays plain.
        return Ok((command, Some(session_id)));
    }
    if family == "zcode" {
        // CDXC:Coordinators 2026-10-04 WHY:
        // ZCode has no launch model flag, so a coordinator create's chosen model reaches the
        // session as a `/model` line the create flow queues before the first request. There is
        // no such delivery for a spawned thread yet, so a thread's model request stays refused
        // rather than silently dropped, and ZCode takes no effort choice at all.
        if effort.is_some() {
            return Err(DomainStateError::bad_request(
                "ZCode agents take no effort choice.",
            ));
        }
        if crate::coordinators::coordinator_create_request(params)?.is_none() {
            return Err(DomainStateError::bad_request(
                "A ZCode thread keeps its configured model; only an orchestrator's own model can be set.",
            ));
        }
        return Ok((command, None));
    }
    if family == "pi"
        && effort
            .as_deref()
            .is_some_and(|effort| !crate::session_chat_pi_models::is_pi_thinking_level(effort))
    {
        return Err(DomainStateError::bad_request(
            "A Pi effort is its thinking level: off, minimal, low, medium, high, xhigh or max.",
        ));
    }
    let base = command
        .or_else(|| default_agent_command(&family).map(str::to_string))
        .unwrap_or_else(|| family.clone());
    with_agent_model_options(&base, &family, model.as_deref(), effort.as_deref())
        .map(|command| (Some(command), None))
}

/// A new Claude, Codex or Cursor session starts on the model the user last chose as the agent's
/// default (CDXC:AgentProviders 2026-10-07 in agent_model_pins.rs). A model the create names, or
/// one the configured command already pins, is this session's own choice and is never taught to
/// the default; with no default known, or after the agent's settings changed outside Ghostex, the
/// session runs the agent's own default and its first report becomes the remembered one.
fn pin_remembered_agent_model(
    agent_id: &str,
    agent_config: &Map<String, Value>,
    launch_settings: &Map<String, Value>,
    params: &Map<String, Value>,
    command: Option<String>,
    runtime_settings: &mut Map<String, Value>,
) -> Result<Option<String>, DomainStateError> {
    use crate::agent_model_pins::{
        launch_default, pin_family, session_marker, ModelPin, ORIGIN_DEFAULT, ORIGIN_LEARN,
        ORIGIN_SESSION, SESSION_PIN_KEY,
    };
    let Some(family) =
        resume_agent_family_id(Some(agent_id.to_string()), agent_config, launch_settings)
            .as_deref()
            .and_then(pin_family)
    else {
        return Ok(command);
    };
    let requested_model = requested_agent_model_option(params, "agentModel")?;
    let requested_effort = requested_agent_model_option(params, "agentEffort")?;
    let base = command
        .clone()
        .or_else(|| default_agent_command(family).map(str::to_string))
        .unwrap_or_else(|| family.to_string());
    if requested_model.is_some() || requested_effort.is_some() || command_names_model(&base, family)
    {
        let requested = requested_model.map(|model| ModelPin {
            model,
            effort: requested_effort,
            ..ModelPin::default()
        });
        runtime_settings.insert(
            SESSION_PIN_KEY.to_string(),
            session_marker(ORIGIN_SESSION, requested.as_ref()),
        );
        return Ok(command);
    }
    let pinned = launch_default(family).and_then(|pin| {
        let effort = pin.effort.as_deref().filter(|_| family != "cursor");
        with_agent_model_options(&base, family, Some(&pin.model), effort)
            .ok()
            .map(|command| (command, pin))
    });
    Ok(match pinned {
        Some((pinned_command, pin)) => {
            runtime_settings.insert(
                SESSION_PIN_KEY.to_string(),
                session_marker(ORIGIN_DEFAULT, Some(&pin)),
            );
            Some(pinned_command)
        }
        None => {
            runtime_settings.insert(
                SESSION_PIN_KEY.to_string(),
                session_marker(ORIGIN_LEARN, None),
            );
            command
        }
    })
}

/// CDXC:Coordinators 2026-09-30 WHY:
/// A coordinator's role is a system prompt flag in the saved base command (see coordinators/role.rs), added here beside the per-session model flags so resume, fork and account wrapping keep it. Claude carries the whole role file this way; Codex and ZCode carry only the guide pointer (ZCode has no such flag, so its role arrives through the SessionStart hook instead); Empryo has no such flag either and gets its role from a queued `/agent` line (`coordinator_role_queued_command`). An agent outside the supported families is refused rather than started without its role.
fn apply_coordinator_role(
    agent_id: &str,
    agent_config: &Map<String, Value>,
    launch_settings: &Map<String, Value>,
    params: &Map<String, Value>,
    command: Option<String>,
) -> Result<Option<String>, DomainStateError> {
    let Some(request) = crate::coordinators::coordinator_create_request(params)? else {
        return Ok(command);
    };
    let role_file = request.role_file.ok_or_else(|| {
        DomainStateError::corrupt_state("The orchestrator role file was not prepared.")
    })?;
    let family = resume_agent_family_id(Some(agent_id.to_string()), agent_config, launch_settings)
        .filter(|family| crate::coordinators::coordinator_agent_family_supported(family))
        .ok_or_else(|| {
            DomainStateError::bad_request(format!(
                "An orchestrator runs on {}. Pick one of those agents.",
                crate::coordinators::COORDINATOR_AGENT_FAMILIES_TEXT
            ))
        })?;
    let base = command
        .or_else(|| default_agent_command(&family).map(str::to_string))
        .unwrap_or_else(|| family.clone());
    crate::coordinators::with_coordinator_role(&base, &family, std::path::Path::new(&role_file))
        .map(Some)
}

pub(crate) fn create_agent_session_default_title(
    agent_name: Option<&str>,
    agent_id: Option<&str>,
) -> String {
    let title_name = normalize_agent_session_title_name(agent_name)
        .or_else(|| {
            default_agent_session_title_name(agent_id.unwrap_or_default()).map(str::to_string)
        })
        .or_else(|| normalize_agent_session_title_name(agent_id));
    title_name
        .map(|name| format!("{name} Session"))
        .unwrap_or_else(|| "Terminal Session".to_string())
}

pub(crate) fn normalize_agent_session_title_name(value: Option<&str>) -> Option<String> {
    let normalized = value?
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!normalized.is_empty()).then_some(normalized)
}

pub(crate) fn default_agent_session_title_name(agent_id: &str) -> Option<&'static str> {
    match agent_id.trim().to_ascii_lowercase().as_str() {
        "amp" => Some("Amp CLI"),
        "antigravity" => Some("Antigravity CLI"),
        "claude" => Some("Claude"),
        "codebuddy" => Some("CodeBuddy"),
        "codex" => Some("Codex"),
        "command-code" => Some("Command Code"),
        "copilot" => Some("Copilot"),
        "cursor" => Some("Cursor CLI"),
        "mastra" => Some("Mastra Code"),
        "devin" => Some("Devin"),
        "droid" => Some("Factory Droid"),
        "empryo" => Some("Empryo"),
        "freebuff" => Some("Freebuff"),
        "gemini" => Some("Gemini"),
        "grok" => Some("Grok Build"),
        "hermes-agent" => Some("Hermes Agent"),
        "kimi" => Some("Kimi Code"),
        "kiro" => Some("Kiro"),
        "omp" => Some("OMP"),
        "openclaude" => Some("OpenClaude"),
        "opencode" => Some("OpenCode"),
        "pi" => Some("Pi"),
        "qoder" => Some("Qoder"),
        "rovodev" => Some("Rovo Dev"),
        _ => None,
    }
}

pub(crate) struct AgentLaunchInput {
    pub(crate) accept_all_mode: Option<String>,
    pub(crate) agent_id: String,
    pub(crate) agent_session_id: Option<String>,
    pub(crate) command: Option<String>,
    pub(crate) delayed_send_deadline_at: Option<String>,
    pub(crate) first_user_message: Option<String>,
    pub(crate) global_accept_all_enabled: bool,
    pub(crate) icon: Option<String>,
}

pub(crate) fn build_agent_launch_plan(input: AgentLaunchInput) -> Value {
    let base_command = input
        .command
        .or_else(|| default_agent_command(&input.agent_id).map(str::to_string))
        .unwrap_or_default();
    let launch_command = resolve_agent_launch_command(
        &input.agent_id,
        &base_command,
        input.accept_all_mode.as_deref(),
        input.global_accept_all_enabled,
        input.icon.as_deref(),
    );
    let command = if input.agent_id == "cursor" {
        input
            .agent_session_id
            .filter(|value| !value.trim().is_empty())
            .and_then(|session_id| get_cursor_chat_session_id(Some(&session_id)))
            .map(|chat_id| {
                format!(
                    "{launch_command} --resume {}",
                    quote_shell_double_arg(&chat_id)
                )
            })
            .unwrap_or(launch_command)
    } else {
        launch_command
    };
    let mut plan = Map::new();
    plan.insert("agentCommand".to_string(), Value::String(base_command));
    plan.insert("command".to_string(), Value::String(command.clone()));
    if let Some(deadline) = input.delayed_send_deadline_at {
        plan.insert(
            "delayedSend".to_string(),
            json!({ "deadlineAt": deadline, "disposition": "scheduled" }),
        );
    }
    if let Some(message) = input.first_user_message {
        plan.insert("firstUserMessage".to_string(), Value::String(message));
    }
    plan.insert(
        "startupText".to_string(),
        Value::String(if command.is_empty() {
            String::new()
        } else {
            as_atuin_ignored_shell_input(&command)
        }),
    );
    plan.insert(
        "startupTextDisposition".to_string(),
        Value::String(if command.is_empty() {
            "none".to_string()
        } else {
            "queueAfterTerminalReady".to_string()
        }),
    );
    Value::Object(plan)
}

pub(crate) fn resolve_project_agent_config(
    project: &Value,
    agent_id: &str,
    launch_settings: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let normalized_agent_id = agent_id.trim().to_ascii_lowercase();
    if let Some(bot) = crate::bot_projects::bot_agent_config(project, agent_id) {
        return bot;
    }
    if let Some(agent) = project
        .get("customAgents")
        .and_then(Value::as_array)
        .and_then(|agents| {
            agents.iter().find(|candidate| {
                candidate
                    .as_object()
                    .and_then(|agent| agent.get("agentId").and_then(Value::as_str))
                    .map(|id| id.trim().eq_ignore_ascii_case(&normalized_agent_id))
                    .unwrap_or(false)
            })
        })
        .and_then(Value::as_object)
    {
        return agent.clone();
    }
    launch_settings
        .filter(|settings| read_text_from_map(settings, "agentCommand").is_some())
        .cloned()
        .unwrap_or_default()
}

/// CDXC:AgentProviders 2026-09-04 DECISION:
/// User: Agent approvals defaults to Ask first, so interactive Claude and Codex launches must not force permission-bypass flags unless the global or per-agent policy explicitly selects Run without asking.
/// SEE-ALSO: packages/shared/ghostex-settings/defaults.ts, packages/find/src/agent.rs, apps/history-cli/src/ui/resume.rs.
pub(crate) fn resolve_agent_launch_command(
    agent_id: &str,
    command: &str,
    accept_all_mode: Option<&str>,
    global_accept_all_enabled: bool,
    icon: Option<&str>,
) -> String {
    let enabled = match accept_all_mode {
        Some("enabled") => true,
        Some("disabled") => false,
        _ => global_accept_all_enabled,
    };
    let command = apply_accept_all_spec(
        command,
        agent_id,
        enabled,
        icon,
        accept_all_mode == Some("disabled"),
    );
    let command = with_codex_no_daemon(agent_id, icon, &command);
    #[cfg(windows)]
    let command = native_cli_command(&command);
    command
}

/// CDXC:AgentProviders 2026-09-28 WHY:
/// PowerShell picks an agent's .ps1 shim before its vendor-supplied .cmd launcher, which fails under the default Restricted policy. Resolve known bare agent commands before adding launch or resume wrappers so both paths use the native launcher selected by CLI discovery, preserving custom shell commands and arguments.
#[cfg(windows)]
fn native_cli_command(command: &str) -> String {
    // Unit tests assert exact commands, which must not depend on the CLIs installed on the machine.
    if cfg!(test) {
        return command.to_string();
    }
    let trimmed = command.trim_start();
    let binary = trimmed.split_whitespace().next().unwrap_or_default();
    if crate::agent_cli::catalog::CATALOG
        .iter()
        .any(|agent| agent.binary == binary)
    {
        if let Some(path) = crate::platform::live_path::find(binary, &[]) {
            if let Some(extension @ ("cmd" | "bat")) =
                path.extension().and_then(|extension| extension.to_str())
            {
                return format!(
                    "{}{binary}.{extension}{}",
                    &command[..command.len() - trimmed.len()],
                    &trimmed[binary.len()..]
                );
            }
        }
    }
    command.to_string()
}

/// Validate supplied launch options before an empty or non-string value can be mistaken for an omitted option.
pub(crate) fn requested_agent_model_option(
    params: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, DomainStateError> {
    let value = match params.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => value.trim(),
        _ => {
            return Err(DomainStateError::bad_request(format!(
                "{key} needs a non-empty string value."
            )))
        }
    };
    // `@` names a pinned model version in some Pi providers (`vertex/<model>@<date>`); a value
    // with it is shell-quoted when the launch command is built.
    if value.len() > 160
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._[]():/@".contains(&byte))
    {
        return Err(DomainStateError::bad_request(format!(
            "\"{value}\" is not a valid model or effort."
        )));
    }
    Ok(Some(value.to_string()))
}
