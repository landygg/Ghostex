//! A message the session's own agent sent another agent: `ghostex agents send` (or `agents create
//! --task`) run through its shell, or Claude's own `SendMessage` tool.
//!
//! CDXC:SessionChat 2026-09-30 DECISION:
//! User: "when the current agent chat that i'm in sends a message to another agent can we add a small card that shows the message being sent when expanded? i want it collapsed by default", looking like the card a received message gets. The send's tool row turns into that card the way an answered question does: a run standing alone draws the card instead of the row, a heading's disclosure or a turn's work fold keeps the row, and a finished turn lifts its cards out of the "Worked for" fold.
//! SEE-ALSO: agent_message.rs parses the received side; sent_message_script.rs reads the message out of variables and decides NOT SENT; server/src/ghostex_cli/agents/delivery.rs prints the result JSON read here; apps/desktop/src/app/native_chat/inter_agent_message.rs and apps/mobile/app/src/chat/native/transcript/SystemRows.tsx draw both cards.

use std::collections::HashMap;

use ghostex_gx_protocol::{ChatBlock, ChatMessage};
use serde_json::{json, Value};

use crate::document::TranscriptItem;
use crate::transcript::jsstr::{ascii_lower, js_trim, last_path_segment};
use crate::transcript::line_breaks::{agent_line_breaks, AgentLineBreaks};
use crate::transcript::markdown_links::markdown_references;
use crate::transcript::native_markdown::native_markdown;
use crate::transcript::sent_message_script::{Resolved, Script, SCRIPTED_BODY};
use crate::transcript::shell_script::ShellCommand;
use crate::transcript::tool_fold::ToolPair;
use crate::transcript::tool_rows::is_command_tool;
use crate::transcript::tool_summary::{command_detail, tool_file_path};

/// One message a tool call sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SentAgentMessage {
    /// The recipient as the sender addressed it: a global reference, a session id or title, an
    /// agent id for `create`, or the `to` of `SendMessage`.
    pub target: String,
    /// The recipient session's global reference, when the send's result named it.
    pub global_ref: String,
    pub agent_name: String,
    pub session_title: String,
    pub body: String,
    /// The send reported that it did not go out.
    pub failed: bool,
}

/// Every message one tool call sent, in the order it sent them. `written` is the files the same
/// message's tools wrote (`(path, content)`), where a `--body-file` body can be read back.
pub fn sent_agent_messages(pair: &ToolPair<'_>, written: &[(&str, &str)]) -> Vec<SentAgentMessage> {
    let Some(name) = pair.call_name() else {
        return Vec::new();
    };
    let input = pair.call_input().unwrap_or(&Value::Null);
    if ascii_lower(name) == "sendmessage" {
        return claude_send_message(input, pair).into_iter().collect();
    }
    if !is_command_tool(name) {
        return Vec::new();
    }
    let Some(command) = command_detail(input) else {
        return Vec::new();
    };
    if !command.contains("agents") {
        return Vec::new();
    }
    let script = Script::parse(&command, written);
    let mut sent: Vec<(usize, SentAgentMessage)> = script
        .commands
        .iter()
        .enumerate()
        .filter_map(|(at, shell)| cli_send(shell, at, &script).map(|sent| (at, sent)))
        .collect();
    if sent.is_empty() {
        return Vec::new();
    }
    let output = pair.result_output().unwrap_or_default();
    let results = send_results(output);
    for (index, (at, message)) in sent.iter_mut().enumerate() {
        match results.get(index) {
            Some(result) => {
                let recipient = result
                    .get("recipient")
                    .or_else(|| result.get("session"))
                    .unwrap_or(&Value::Null);
                let field = |key: &str| {
                    recipient
                        .get(key)
                        .and_then(Value::as_str)
                        .map(js_trim)
                        .unwrap_or_default()
                        .to_string()
                };
                message.global_ref = field("globalRef");
                message.agent_name = field("agentName");
                let title = field("title");
                if !title.is_empty() {
                    message.session_title = title;
                }
                message.failed = result.get("ok") == Some(&Value::Bool(false));
                // What the CLI echoes is what went out, whatever the script built it from.
                let echoed = result
                    .get("message")
                    .or_else(|| result.get("task"))
                    .and_then(Value::as_str)
                    .map(js_trim)
                    .unwrap_or_default();
                if !echoed.is_empty() {
                    message.body = echoed.to_string();
                }
            }
            // No result read back (the agent filtered it, or the call is still running).
            None => {
                message.failed =
                    results.is_empty() && script.send_failed(output, pair.result_is_error(), *at)
            }
        }
    }
    sent.into_iter().map(|(_, message)| message).collect()
}

/// Claude's `SendMessage`: `{to, message, summary}`. A structured `message` (a shutdown request,
/// a plan approval) is protocol, not a message the reader wrote to anyone.
fn claude_send_message(input: &Value, pair: &ToolPair<'_>) -> Option<SentAgentMessage> {
    let body = input.get("message")?.as_str()?;
    let target = input.get("to")?.as_str()?;
    let failed = pair.result_is_error()
        || pair
            .result_output()
            .and_then(|output| serde_json::from_str::<Value>(js_trim(output)).ok())
            .is_some_and(|result| result.get("success") == Some(&Value::Bool(false)));
    Some(SentAgentMessage {
        target: js_trim(target).to_string(),
        session_title: input
            .get("summary")
            .and_then(Value::as_str)
            .map(js_trim)
            .unwrap_or_default()
            .to_string(),
        body: js_trim(body).to_string(),
        failed,
        ..SentAgentMessage::default()
    })
}

/// One `ghostex agents send` or `ghostex agents create --task` in a script.
fn cli_send(shell: &ShellCommand, at: usize, script: &Script<'_>) -> Option<SentAgentMessage> {
    let start = shell.words.windows(3).position(|words| {
        matches!(last_path_segment(&words[0]), "ghostex" | "gx")
            && words[1] == "agents"
            && matches!(words[2].as_str(), "send" | "create")
    })?;
    // Only a command that runs the CLI: `echo ghostex agents send …` sends nothing. Environment
    // assignments and a `bash -lc` wrapper (how Codex records a shell call) may come first.
    let runs_cli = shell.words[..start].iter().all(|word| {
        (word.contains('=') && !word.starts_with('-'))
            || matches!(
                word.as_str(),
                "env" | "command" | "exec" | "bash" | "sh" | "zsh" | "-c" | "-lc" | "-l"
            )
    });
    if !runs_cli {
        return None;
    }
    let create = shell.words[start + 2] == "create";
    let mut positional: Vec<&str> = Vec::new();
    let mut body_file: Option<&str> = None;
    let mut task: Option<&str> = None;
    let mut title: Option<&str> = None;
    let mut words = shell.words[start + 3..].iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--" => {
                positional.extend(words.by_ref().map(String::as_str));
                break;
            }
            "--body-file" => body_file = words.next().map(String::as_str),
            "--task" => task = words.next().map(String::as_str),
            "--title" => title = words.next().map(String::as_str),
            "--server" | "--project-id" | "--request-id" => {
                words.next();
            }
            flag if flag.starts_with('-') => {}
            value => positional.push(value),
        }
    }
    let target = match script.value(positional.first()?, at, shell) {
        Resolved::Text(target) => target,
        Resolved::Unknown => positional[0].to_string(),
    };
    let body = if create {
        task
    } else {
        positional.get(1).copied()
    };
    let body = match (body, body_file) {
        (Some(word), _) => match script.value(word, at, shell) {
            Resolved::Text(body) => body,
            Resolved::Unknown => SCRIPTED_BODY.to_string(),
        },
        (None, Some(path)) => script.file_text(path, shell).unwrap_or_default(),
        // `create` without a task starts an agent; it sends it nothing.
        (None, None) if create => return None,
        (None, None) => String::new(),
    };
    Some(SentAgentMessage {
        target,
        session_title: title.unwrap_or_default().to_string(),
        body: js_trim(&body).to_string(),
        ..SentAgentMessage::default()
    })
}

/// The JSON objects `agents send` / `create` printed, in order. Other output (a `wake` line, a
/// filtered tail) is skipped; a result the agent cut short does not parse and is left out.
fn send_results(output: &str) -> Vec<Value> {
    let mut results = Vec::new();
    let mut offset = 0;
    while offset < output.len() {
        let line_end = output[offset..]
            .find('\n')
            .map_or(output.len(), |at| offset + at);
        if output[offset..line_end].trim_start().starts_with('{') {
            let mut stream =
                serde_json::Deserializer::from_str(&output[offset..]).into_iter::<Value>();
            if let Some(Ok(value)) = stream.next() {
                let consumed = stream.byte_offset();
                if value.get("recipient").is_some() || value.get("session").is_some() {
                    results.push(value);
                }
                offset += consumed.max(1);
                continue;
            }
        }
        offset = line_end + 1;
    }
    results
}

/// The files a message's tools wrote, for a `--body-file` sent later in the same message.
pub fn written_files(message: &ChatMessage) -> Vec<(&str, &str)> {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ChatBlock::ToolCall { input, .. } => {
                let content = input.get("content")?.as_str()?;
                Some((tool_file_path(input)?, content))
            }
            _ => None,
        })
        .collect()
}

/// The `sentMessages` a projected message carries: one card per message its tool calls sent, keyed
/// `<message id>:<n>`. The names are completed across the transcript by [`resolve_recipients`].
pub fn project_sent_messages(
    message: &ChatMessage,
    pairs: &[ToolPair<'_>],
    line_breaks: AgentLineBreaks,
) -> Vec<Value> {
    let written = written_files(message);
    pairs
        .iter()
        .enumerate()
        .flat_map(|(tool, pair)| {
            sent_agent_messages(pair, &written)
                .into_iter()
                .map(move |sent| (tool, sent))
        })
        .enumerate()
        .map(|(index, (tool, sent))| {
            let body = agent_line_breaks(&sent.body, line_breaks);
            let markdown = native_markdown(&body, false);
            json!({
                "key": format!("{}:{index}", message.id),
                "tool": tool,
                "target": sent.target,
                "globalRef": sent.global_ref,
                "agentName": sent.agent_name,
                "sessionTitle": sent.session_title,
                "body": body,
                "markdownReferences": markdown_references(&markdown),
                "markdown": markdown,
                "failed": sent.failed,
            })
        })
        .collect()
}

/// The recipient a reference names, as far as the transcript tells: the header of a message that
/// session sent here, or the result of an earlier send to it.
type Directory = HashMap<String, (String, String)>;

fn directory_key(reference: &str) -> String {
    js_trim(reference).to_lowercase()
}

fn remember(directory: &mut Directory, reference: &str, agent: &str, title: &str) {
    if js_trim(reference).is_empty() || (agent.is_empty() && title.is_empty()) {
        return;
    }
    directory
        .entry(directory_key(reference))
        .or_insert_with(|| (agent.to_string(), title.to_string()));
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn learn(directory: &mut Directory, message: &Value) {
    let inter = &message["interAgentMessage"];
    if inter.is_object() && inter["subagent"] != true {
        let (agent, title) = (text(inter, "agentName"), text(inter, "sessionTitle"));
        remember(directory, text(inter, "replyTo"), agent, title);
        remember(directory, text(inter, "sessionId"), agent, title);
    }
    // The call that started a background agent: its id, type and task.
    for tool in message["tools"].as_array().into_iter().flatten() {
        let subagent = &tool["subagent"];
        if subagent.is_object() && subagent["self"] != true {
            let agent = text(subagent, "agentType");
            remember(
                directory,
                text(subagent, "selector"),
                if agent.is_empty() { "subagent" } else { agent },
                text(subagent, "name"),
            );
        }
    }
    for sent in message["sentMessages"].as_array().into_iter().flatten() {
        let (agent, title) = (text(sent, "agentName"), text(sent, "sessionTitle"));
        if !agent.is_empty() {
            remember(directory, text(sent, "globalRef"), agent, title);
            remember(directory, text(sent, "target"), agent, title);
        }
    }
}

/// The background agents on the Subagents roster (`{agents: [{id, name, task}]}`).
fn learn_fleet(directory: &mut Directory, fleet: Option<&Value>) {
    let agents = fleet.and_then(|fleet| fleet["agents"].as_array());
    for agent in agents.into_iter().flatten() {
        let kind = text(agent, "name");
        remember(
            directory,
            text(agent, "id"),
            if kind.is_empty() { "subagent" } else { kind },
            text(agent, "task"),
        );
    }
}

/// A background agent's message names its sender by id only; the directory knows its type and task.
fn complete_received(directory: &Directory, message: &mut Value) {
    let inter = &mut message["interAgentMessage"];
    if inter["subagent"] != true {
        return;
    }
    if let Some((agent, title)) = directory.get(&directory_key(text(inter, "replyTo"))) {
        inter["agentName"] = agent.clone().into();
        inter["sessionTitle"] = title.clone().into();
    }
}

/// Fills in each card's recipient from the directory and writes its `title` and `detail`.
fn complete(directory: &Directory, cards: &mut [Value]) {
    for sent in cards.iter_mut() {
        let known = directory.get(&directory_key(text(sent, "target")));
        let mut agent = text(sent, "agentName").to_string();
        let mut session = text(sent, "sessionTitle").to_string();
        if let Some((known_agent, known_title)) = known {
            if agent.is_empty() {
                agent = known_agent.clone();
            }
            if session.is_empty() {
                session = known_title.clone();
            }
        }
        let name = if agent.is_empty() {
            text(sent, "target").to_string()
        } else {
            agent
        };
        sent["title"] = format!("Message to {name}").into();
        sent["detail"] = if session == name {
            String::new()
        } else {
            session
        }
        .into();
    }
}

fn messages_mut(item: &mut TranscriptItem) -> Vec<&mut Value> {
    match item {
        TranscriptItem::Message { message } => vec![message],
        TranscriptItem::Summary {
            user,
            final_message,
            earlier_replies,
            work,
            outcome,
            ..
        } => std::iter::once(user)
            .chain(final_message.iter_mut())
            .chain(earlier_replies.iter_mut())
            .chain(work.iter_mut())
            .chain(outcome.iter_mut())
            .collect(),
        TranscriptItem::CompletedWork {
            work,
            artifacts,
            final_message,
            ..
        } => work
            .iter_mut()
            .chain(artifacts.iter_mut())
            .chain(final_message.iter_mut())
            .collect(),
        TranscriptItem::Unknown(_) => Vec::new(),
    }
}

/// Names every sent-message card by what the whole transcript knows about its recipient: an agent
/// that filtered away its send's result, or addressed a session by reference only, still gets the
/// name and title that session's own messages carried.
pub fn resolve_recipients(items: &mut [TranscriptItem], agent_fleet: Option<&Value>) {
    let mut directory = Directory::new();
    learn_fleet(&mut directory, agent_fleet);
    for item in items.iter_mut() {
        for message in messages_mut(item) {
            learn(&mut directory, message);
        }
    }
    for item in items.iter_mut() {
        for message in messages_mut(item) {
            complete_received(&directory, message);
            if let Some(cards) = message
                .get_mut("sentMessages")
                .and_then(Value::as_array_mut)
            {
                complete(&directory, cards);
            }
        }
        // A finished turn's hoisted cards are copies of its work rows' own.
        if let TranscriptItem::CompletedWork { sent_messages, .. } = item {
            complete(&directory, sent_messages);
        }
    }
}
