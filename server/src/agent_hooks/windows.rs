use serde_json::{json, Value};
use std::{io::Read, path::Path, sync::mpsc, time::Duration};

/// CDXC:AgentHooks 2026-09-14 WHY:
/// Windows command hooks must read JSON from stdin directly; passing it through Windows PowerShell 5.1 native argv strips JSON quotes.
/// Installation still uses the existing explicit install and Codex trust flow.
///
/// CDXC:AgentHooks 2026-10-09 WHY:
/// Codex runs hooks through cmd.exe and caps Interrupt at 3 seconds. Its shell exits outside Ghostex before loading the executable: even a cold native launch exceeded that budget. Routed hooks use double-quoted native arguments. Claude Code and Grok use PowerShell or Git Bash and take the statusline's space-free executable and single-quoted arguments. PowerShell startup exceeded these hooks' budgets even outside Ghostex.
pub(crate) fn command(agent: &str, notify_path: &Path) -> String {
    let executable = std::env::current_exe().unwrap_or_default();
    let notify = notify_path.to_string_lossy();
    if agent == "codex" {
        return format!(
            "(if not defined GHOSTEX_GLOBAL_SESSION_REF if not defined GHOSTEX_SESSION_STATE_FILE if not defined VSMUX_SESSION_STATE_FILE exit /b 0) & \"{}\" agent-hook-notify-native \"{notify}\" codex",
            executable.display()
        );
    }
    if let Some(executable) = bare_command_path(&executable)
        .filter(|_| matches!(agent, "claude" | "grok") && !notify.contains('\''))
    {
        return format!("{executable} agent-hook-notify-native '{notify}' '{agent}'");
    }
    let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
    format!(
        "powershell.exe -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -Command \"& {} agent-hook-notify-native {} {}\"",
        quote(&executable.to_string_lossy()),
        quote(&notify_path.to_string_lossy()),
        quote(agent)
    )
}

/// `path` as a command word that Git Bash and PowerShell both run as it stands: its 8.3 short
/// form with forward slashes, when that has no space, quote or other character either shell
/// would read. `None` when the volume keeps no short names and the long path needs quoting.
pub(crate) fn bare_command_path(path: &Path) -> Option<String> {
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buffer = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated and `buffer` holds `buffer.len()` UTF-16 units.
    let length = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
            wide.as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return None;
    }
    let short = std::ffi::OsString::from_wide(&buffer[..length])
        .into_string()
        .ok()?
        .replace('\\', "/");
    short
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '/' | '.' | '_' | '~' | '-'))
        .then_some(short)
}

pub(crate) fn notify(args: Vec<String>) -> anyhow::Result<()> {
    let agent = args.get(1).map(String::as_str).unwrap_or("codex");
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut input = String::new();
        let result = std::io::stdin()
            .take(1024 * 1024)
            .read_to_string(&mut input)
            .map(|_| input);
        let _ = sender.send(result);
    });
    let input = receiver.recv_timeout(Duration::from_millis(1000));
    // CDXC:AgentHooks 2026-10-07 WHY: Cursor CLI runs Windows hooks as `$OutputEncoding = [System.Text.Encoding]::UTF8; Get-Content <payload> -Raw | & { $input | <command> }`, and that encoding writes a UTF-8 BOM before the JSON in pwsh and Windows PowerShell alike; serde rejected it ("expected value at line 1 column 1"), so no Cursor hook ever reached gxserver and Cursor chats never found their transcript.
    let parsed = input
        .as_ref()
        .ok()
        .and_then(|result| result.as_deref().ok())
        .and_then(|text| serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok());
    let state = [
        "VSMUX_SESSION_STATE_FILE",
        "GHOSTEX_SESSION_STATE_FILE",
        "ghostex_SESSION_STATE_FILE",
    ]
    .into_iter()
    .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
    .unwrap_or_default();
    if std::env::var("GHOSTEX_INTERNAL_PROMPT_GENERATION").as_deref() == Ok("1")
        || std::env::var("GHOSTEX_INTERNAL_TITLE_GENERATION").as_deref() == Ok("1")
        || (state.is_empty()
            && agent != "empryo"
            && [
                "GHOSTEX_GLOBAL_SESSION_REF",
                "GHOSTEX_GXSERVER_BASE_URL",
                "GHOSTEX_GXSERVER_AUTH_TOKEN_FILE",
            ]
            .iter()
            .any(|key| std::env::var(key).map_or(true, |value| value.is_empty())))
    {
        // Like the bash notify script, a hook outside Ghostex still gives its agent the canned response.
        print_canned_response(agent, parsed.as_ref().unwrap_or(&Value::Null));
        return Ok(());
    }
    let input = input??;
    let mut payload = match parsed {
        Some(payload) => payload,
        None => serde_json::from_str(input.trim_start_matches('\u{feff}'))?,
    };
    if let Some(object) = payload.as_object_mut() {
        object.entry("agent").or_insert_with(|| json!(agent));
    }
    let script = std::fs::read_to_string(
        args.first()
            .ok_or_else(|| anyhow::anyhow!("Missing hook path"))?,
    )?;
    let directory = super::install::notify_hook_state_directory(&script)
        .ok_or_else(|| anyhow::anyhow!("Missing hook state directory"))?;
    let answer = super::run_notify_hook(vec![
        state,
        payload.to_string(),
        directory.to_string_lossy().into_owned(),
    ])
    .ok()
    .flatten();
    if let Some(answer) = answer {
        // A ZCode coordinator's SessionStart answer replaces the canned response.
        println!("{answer}");
        return Ok(());
    }
    print_canned_response(agent, &payload);
    Ok(())
}

fn print_canned_response(agent: &str, payload: &Value) {
    if agent != "antigravity" {
        if payload["hook_event_name"] == "Interrupt" {
            println!("{{}}");
        } else {
            println!("{{\"continue\":true}}");
        }
    }
}

/// Resolved over the live registry PATH, so a CLI installed while gxserver runs is not reported missing.
pub(crate) fn resolve_command(command: &str) -> Option<String> {
    crate::platform::live_path::find(command, &[]).map(|path| path.to_string_lossy().into_owned())
}
