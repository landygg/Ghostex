//! The coordinator's role: the playbook it runs by, and how it reaches the agent at launch.

use std::path::{Path, PathBuf};

use crate::domain::DomainStateError;
use crate::paths::GxserverPaths;

/// CDXC:Coordinators 2026-09-30 WHY:
/// The role reaches the agent as a system prompt, not as a chat message: a first message scrolls away, is summarised by compaction, and shows in the chat as if the user typed it. The flag lives in the session's saved base command, which resume, fork and account wrapping rebuild from, so it survives all three (the same mechanism as per-session model flags). Dynamic state (goal, instructions, memory, threads) is read through `ghostex coordinator status` instead, because it changes while the session runs.
/// SEE-ALSO: server/src/agents/launch_plan.rs (applies the flags), server/src/ghostex_cli/coordinator/ (the verbs this playbook names).
///
/// CDXC:Coordinators 2026-10-09 DECISION:
/// User: "If a tool call got rejected with ("STOP and wait") just as a thread report arrives then this is Ghostex's delivery Escape, not the owner pausing your work: retry the tool call and carry on." The playbook's "Waiting means ending your turn" section says so.
///
/// CDXC:Coordinators 2026-10-10 DECISION:
/// User: "let's please rename "Coordinator" to "Orchestrator" everywhere so it's clearer to everyone." The playbook and the guide pointer call the role an orchestrator and name `ghostex orchestrator` verbs; the role file's name on disk and every internal name keep "coordinator".
pub const COORDINATOR_ROLE_PROMPT: &str = include_str!("role.md");

/// CDXC:Coordinators 2026-10-03 WHY:
/// Codex takes its extra instructions inline (`-c developer_instructions=...`), and ZCode has no
/// system-prompt or config flag a launch command could carry, so both get this short pointer —
/// Codex inline at launch, ZCode as SessionStart hook context — and read the full playbook from
/// `ghostex coordinator guide`. Claude is the only family that carries the whole role file.
pub const GUIDE_POINTER_COORDINATOR_INSTRUCTIONS: &str = "You are a Ghostex orchestrator: the user talks only to you, and you hand real work to thread sessions instead of doing it yourself, so you stay free to talk. Before your first reply, and again after any context compaction, run `ghostex orchestrator guide` and follow it for the whole session. Run `ghostex orchestrator status` at the start of every request.";

/// Where gxserver keeps the role file Claude coordinators load at launch.
pub fn coordinator_role_file(paths: &GxserverPaths) -> PathBuf {
    paths
        .root_dir
        .join("coordinators")
        .join("coordinator-role.md")
}

/// Writes the role file when it is missing or out of date, so an upgrade reaches every
/// coordinator at its next launch or resume.
pub fn ensure_coordinator_role_file(paths: &GxserverPaths) -> std::io::Result<PathBuf> {
    let path = coordinator_role_file(paths);
    write_if_changed(&path, COORDINATOR_ROLE_PROMPT)?;
    Ok(path)
}

fn write_if_changed(path: &Path, content: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(content) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
    }
    Ok(())
}

/// The agent families a coordinator can run on, in the order `ghostex coordinator create`
/// prefers them when no agent is named.
pub const COORDINATOR_AGENT_FAMILIES: [&str; 4] = ["claude", "codex", "zcode", "empryo"];

/// What gxserver answers when a coordinator is asked to run on another agent.
pub const COORDINATOR_AGENT_FAMILIES_TEXT: &str = "Claude, Codex, ZCode or Empryo";

/// The custom agent profile an Empryo coordinator runs as.
pub const EMPRYO_COORDINATOR_AGENT_NAME: &str = "ghostex-coordinator";

/// The line queued ahead of everything else a new or promoted coordinator of this family is
/// sent, when its role arrives that way instead of at launch.
///
/// CDXC:Coordinators 2026-10-06 WHY:
/// Empryo 3.9.0-beta's interactive app reads neither `--agent` nor `--system` (its argv parser returns early without `--headless`) and ignores hook `additionalContext`, so a launch flag cannot carry the role. The role spike (card agent-bo-95422941) proved the TUI's `/agent <name>` command does: the profile's body joins the tab's system prompt (35,520 to 44,990 characters), and the tab keeps it across `--session` resume. So gxserver writes the playbook as the global profile `~/.empryo/agents/ghostex-coordinator.md` and queues `/agent ghostex-coordinator` once, at create and at promote.
/// SEE-ALSO: server/src/server/coordinator_runtime.rs (`queue_coordinator_role_command`, the queueing), server/src/server/route_http/sessions.rs (create).
pub fn coordinator_role_queued_command(family: &str) -> Option<String> {
    (family == "empryo").then(|| format!("/agent {EMPRYO_COORDINATOR_AGENT_NAME}"))
}

/// Writes the Empryo coordinator profile when it is missing or out of date, like the Claude role
/// file. The description keeps Empryo from delegating ordinary tasks to it.
pub fn ensure_empryo_coordinator_agent_file(paths: &GxserverPaths) -> std::io::Result<PathBuf> {
    let hook_paths = crate::agent_hooks::config::HookPaths::from_paths(paths);
    // Empryo 3.9.1 reads global profiles from `<home>/.empryo/agents` on every platform, also on
    // Windows, where its config and hooks live in `%LOCALAPPDATA%\Empryo` (`empryo_home`).
    let path = hook_paths
        .home_dir
        .join(".empryo")
        .join("agents")
        .join(format!("{EMPRYO_COORDINATOR_AGENT_NAME}.md"));
    write_if_changed(
        &path,
        &format!(
            "---\nname: {EMPRYO_COORDINATOR_AGENT_NAME}\ndescription: The Ghostex orchestrator role. Ghostex starts an orchestrator session with it; never delegate a task to it.\n---\n\n{}\n",
            COORDINATOR_ROLE_PROMPT.trim()
        ),
    )?;
    Ok(path)
}

/// True for the agent families a coordinator can run on.
pub fn coordinator_agent_family_supported(family: &str) -> bool {
    COORDINATOR_AGENT_FAMILIES.contains(&family)
}

/// Appends the role flags to an agent command of the given family. ZCode's and Empryo's commands
/// stay unchanged: ZCode's role reaches the agent through the SessionStart hook
/// (`ingest_agent_hook_event` answers it with the guide pointer), Empryo's through
/// [`coordinator_role_queued_command`].
pub fn with_coordinator_role(
    command: &str,
    family: &str,
    role_file: &Path,
) -> Result<String, DomainStateError> {
    let command = command.trim();
    match family {
        "claude" => Ok(format!(
            "{command} --append-system-prompt-file {}",
            crate::agents::quote_shell_arg(&role_file.to_string_lossy())
        )),
        "codex" => {
            let value = serde_json::to_string(GUIDE_POINTER_COORDINATOR_INSTRUCTIONS)
                .unwrap_or_else(|_| "\"\"".to_string());
            Ok(format!(
                "{command} -c {}",
                crate::agents::quote_shell_arg(&format!("developer_instructions={value}"))
            ))
        }
        "zcode" | "empryo" => Ok(command.to_string()),
        _ => Err(DomainStateError::bad_request(format!(
            "An orchestrator runs on {COORDINATOR_AGENT_FAMILIES_TEXT}. Pick one of those agents."
        ))),
    }
}
