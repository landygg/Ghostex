use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

use crate::ghostex_cli::args::parse_args;
use crate::ghostex_cli::rpc::{
    request_gxserver_rpc, resolve_gxserver_server_target, CliError, CliResult,
};
use crate::visual_pages::{prepare_page, PUBLISH_VISUAL_PAGE_ENDPOINT};

const SOURCE_MAX_BYTES: u64 = 512 * 1024;

/// `ghostex show <file.html> [--title <text>] [--json]`: publishes a self-contained HTML page
/// through gxserver and prints the ```visual block that shows it as a card in the chat.
///
/// CDXC:SessionChat 2026-10-06 DECISION:
/// User: "both": the chat's native ```visual blocks (charts, stats, tables, text) and HTML pages ship together. A page covers what a block cannot express, such as a UI mockup or a small interactive tool; this verb publishes it and prints the page block the agent pastes into its reply.
pub fn show_command(args: &[String]) -> CliResult<()> {
    if matches!(args.first().map(String::as_str), Some("help")) {
        println!("{}", crate::ghostex_cli::usage::command_usage("show"));
        return Ok(());
    }
    let parsed = parse_args(args);
    // `--browser` takes no value, but the shared parser reads the next word as one when the file
    // comes after it.
    let browser_value = parsed
        .flags
        .string_value("browser")
        .filter(|value| !matches!(*value, "true" | "false"))
        .map(str::to_string);
    let file = parsed.rest.first().cloned().or(browser_value);
    let popup = !parsed.flags.contains("browser");
    let Some(file) = file.as_ref() else {
        return Err(CliError::Other(
            "show needs an HTML file: ghostex show <file.html> [--title <text>] [--browser] [--json]"
                .to_string(),
        ));
    };
    let path = absolute_path(file);
    let source = read_source(&path)?;
    let prepared = prepare_page(&source).map_err(CliError::Other)?;
    let title = parsed
        .flags
        .string_value("title")
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .or(prepared.title)
        .unwrap_or_else(|| file_stem(&path));
    let mut params = json!({ "title": title, "html": prepared.html });
    if let Some(session_ref) = calling_session_ref() {
        params["sessionRef"] = Value::String(session_ref);
    }
    let target = resolve_gxserver_server_target(&parsed.flags, &params)?;
    let result = request_gxserver_rpc(
        &target,
        PUBLISH_VISUAL_PAGE_ENDPOINT,
        &params,
        &parsed.flags,
    )?;
    let text = |key: &str| result.get(key).and_then(Value::as_str).unwrap_or_default();
    let (id, page_path) = (text("id"), text("path"));
    if id.is_empty() || page_path.is_empty() {
        return Err(CliError::Other(
            "gxserver did not return the published page's address. Update Ghostex and try again."
                .to_string(),
        ));
    }
    let title = Some(text("title"))
        .filter(|title| !title.is_empty())
        .unwrap_or(title.as_str());
    let url = format!("{}{page_path}", target.base_url.trim_end_matches('/'));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let block = visual_page_block(title, &url, &file_name, popup);
    if parsed.flags.truthy("json") {
        println!(
            "{{\"ok\":true,\"id\":{},\"title\":{},\"url\":{},\"block\":{}}}",
            json_string(id),
            json_string(title),
            json_string(&url),
            json_string(&block)
        );
    } else {
        println!(
            "Published \"{title}\".\nPut this block in your reply where the page should appear (the chat shows it as a card that {}):\n\n{block}",
            if popup {
                "opens the page in a floating window over the chat"
            } else {
                "opens the page in the browser"
            }
        );
    }
    Ok(())
}

/// CDXC:SessionChat 2026-10-06 SEE-ALSO:
/// The page block `{"page":{"title","url","file","open"}}` is a contract with the chat's visual parser in packages/gx-visual, which draws it as a card whose Open button opens the URL, and with skills/ghostex-visuals/SKILL.md, which tells agents to paste it exactly as printed.
/// CDXC:SessionChat 2026-10-09 DECISION:
/// User: the agent gives its HTML link "some mark so when we click to open it it's intended to be opened floating", so the agent can show a card for its page in the chat. The mark is `"open": "popup"`, printed by default; `--browser` leaves it out so the card opens the page in the browser instead.
fn visual_page_block(title: &str, url: &str, file: &str, popup: bool) -> String {
    let open = if popup { ",\"open\":\"popup\"" } else { "" };
    format!(
        "```visual\n{{\"page\":{{\"title\":{},\"url\":{},\"file\":{}{open}}}}}\n```",
        json_string(title),
        json_string(url),
        json_string(file)
    )
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

fn absolute_path(file: &str) -> PathBuf {
    let path = PathBuf::from(file);
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
}

fn read_source(path: &Path) -> CliResult<String> {
    let metadata = fs::metadata(path)
        .map_err(|_| CliError::Other(format!("Could not find {}.", path.display())))?;
    if !metadata.is_file() {
        return Err(CliError::Other(format!(
            "{} is not a file.",
            path.display()
        )));
    }
    if metadata.len() > SOURCE_MAX_BYTES {
        return Err(CliError::Other(format!(
            "The page is {} KiB; the limit is 512 KiB.",
            metadata.len().div_ceil(1024)
        )));
    }
    let bytes = fs::read(path)
        .map_err(|error| CliError::Other(format!("Could not read {}: {error}", path.display())))?;
    String::from_utf8(bytes).map_err(|_| {
        CliError::Other(
            "The page is not UTF-8 text. Save it as UTF-8 and run ghostex show again.".to_string(),
        )
    })
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_string())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "Page".to_string())
}

/// The session this runs in, from the environment every Ghostex pane exports; kept with the page
/// so it can be traced back to the conversation that made it.
fn calling_session_ref() -> Option<String> {
    ["GHOSTEX_GLOBAL_SESSION_REF", "GHOSTEX_NATIVE_SESSION_ID"]
        .iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}
