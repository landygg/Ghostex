//! `/api/readSessionText` marks Claude Code's prompt suggestion, which a plain capture prints as if
//! it had been typed into the input box.

/// What the screen read appends to the input row that only holds Claude's suggestion.
const SUGGESTION_NOTE: &str = "  (faint suggestion from Claude, not typed: the input box is empty)";

/// CDXC:AgentScreenDetection 2026-10-10 WHY:
/// Claude Code shows its guess at the user's next message in an empty input box in faint text (`❯ yes delete the branch, and wipe the A1 result`). A plain capture drops the faint style, so a coordinator reading `ghostex read-text` took two such guesses for the user's unsent messages, pressed Enter into empty boxes and reported that Enter no longer submits. Sends already read the VT capture and treat an all-faint input as empty (session_chat_composer_input.rs); the screen read now says so on that row and returns the suggestion as `inputSuggestion`.
pub(super) fn mark_claude_input_suggestion(
    agent: Option<&str>,
    zmx_name: &str,
    text: String,
) -> (String, Option<String>) {
    let Some(agent @ ("claude" | "openclaude")) = agent else {
        return (text, None);
    };
    let Ok(capture) = super::screen_capture::read_zmx_session_screen_capture_vt(zmx_name) else {
        return (text, None);
    };
    let Some(input) = crate::session_chat_composer::session_chat_composer_input(agent, &capture.text)
    else {
        return (text, None);
    };
    let suggestion = input.text.trim();
    if suggestion.is_empty() || input.shell_mode || input.rows != 1 || !input.text_is_empty() {
        return (text, None);
    }
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let Some(row) = lines.iter().rposition(|line| {
        line.trim()
            .strip_prefix('❯')
            .is_some_and(|rest| rest.trim() == suggestion)
    }) else {
        return (text, None);
    };
    let line = &mut lines[row];
    let carriage_return = line.ends_with('\r');
    *line = format!(
        "{}{SUGGESTION_NOTE}{}",
        line.trim_end(),
        if carriage_return { "\r" } else { "" }
    );
    (lines.join("\n"), Some(suggestion.to_string()))
}
