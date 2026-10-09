//! The script around a `ghostex agents send`: where its message text comes from when the call does
//! not spell it out, and whether a failed call means the send itself failed.
//!
//! CDXC:SessionChat 2026-10-10 WHY:
//! User: "NOT SENT isn't aligned center vertically in the header of the card ... and i see $msg instead of the message". Agents on Windows send with `$msg = @'…'@; ghostex agents send <ref> $msg 2>&1 | Select-String '"status"'`, and on POSIX with `"$(cat file)"`, so the card read the literal `$msg` and, finding no receipt in the filtered output, called a delivered message NOT SENT. The body now comes from the CLI's echoed `message`, else from the variable's assignment, here-string or file in the same script, and never shows a bare `$var` (it says the script set it). NOT SENT needs a printed send error or a failed exit status that belongs to the send itself; a filtered or chained command whose status says nothing about the send shows no status instead of a wrong one.
//! SEE-ALSO: sent_message.rs reads the cards; server/src/ghostex_cli/agents/delivery.rs and lifecycle.rs echo `message` / `task` in their JSON.

use crate::transcript::jsstr::ascii_lower;
use crate::transcript::shell_script::{parse_shell, ShellCommand};

/// What the card says for a message whose text only a script variable held.
pub const SCRIPTED_BODY: &str = "The message text was set by a script, so it isn't shown here.";

/// The word a lifted here-string reads as in the script the shell reader sees.
const HERE_STRING_MARK: &str = "__gx_here_string_";

/// A send's message as far as the script tells.
pub enum Resolved {
    Text(String),
    /// A variable or substitution whose value the script does not show.
    Unknown,
}

/// One tool call's command line, read for its sends.
pub struct Script<'a> {
    pub commands: Vec<ShellCommand>,
    here_strings: Vec<String>,
    /// The files the same message's tools wrote (`(path, content)`).
    written: &'a [(&'a str, &'a str)],
}

impl<'a> Script<'a> {
    pub fn parse(command: &str, written: &'a [(&'a str, &'a str)]) -> Self {
        let (script, here_strings) = lift_here_strings(command);
        Script {
            commands: parse_shell(&script),
            here_strings,
            written,
        }
    }

    /// The text a send's message `word` stands for, reading assignments made before command `at`.
    pub fn value(&self, word: &str, at: usize, send: &ShellCommand) -> Resolved {
        if let Some(body) = self.here_string(word) {
            return Resolved::Text(body.to_string());
        }
        if let Some(name) = variable_name(word) {
            let assigned = self.commands[..at.min(self.commands.len())]
                .iter()
                .enumerate()
                .rev()
                .find_map(|(index, shell)| {
                    assignment(shell)
                        .filter(|(assigned, _)| *assigned == name)
                        .map(|(_, value)| (index, value))
                });
            return match assigned {
                Some((index, words)) if words.len() == 1 => {
                    self.value(&words[0], index, &self.commands[index])
                }
                Some((_, words)) => self
                    .file_read(&words)
                    .and_then(|path| self.file_text(path, send))
                    .map_or(Resolved::Unknown, Resolved::Text),
                None => Resolved::Unknown,
            };
        }
        if let Some(inner) = word
            .strip_prefix("$(")
            .and_then(|inner| inner.strip_suffix(')'))
        {
            let read = parse_shell(inner);
            return read
                .first()
                .and_then(|shell| self.file_read(&shell.words))
                .and_then(|path| self.file_text(path, send))
                .map_or(Resolved::Unknown, Resolved::Text);
        }
        if is_expansion(word) {
            return Resolved::Unknown;
        }
        Resolved::Text(word.to_string())
    }

    fn here_string(&self, word: &str) -> Option<&str> {
        let index = word
            .strip_prefix(HERE_STRING_MARK)?
            .strip_suffix("__")?
            .parse::<usize>()
            .ok()?;
        self.here_strings.get(index).map(String::as_str)
    }

    /// The path a `cat path` / `Get-Content -Raw path` reads.
    fn file_read<'w>(&self, words: &'w [String]) -> Option<&'w str> {
        let (program, arguments) = words.split_first()?;
        if !matches!(
            ascii_lower(program).as_str(),
            "cat" | "get-content" | "gc" | "type"
        ) {
            return None;
        }
        let mut path = None;
        let mut arguments = arguments.iter();
        while let Some(word) = arguments.next() {
            match ascii_lower(word).as_str() {
                "-path" | "-literalpath" => path = arguments.next().map(String::as_str),
                "-encoding" | "-totalcount" | "-tail" | "-head" => {
                    arguments.next();
                }
                flag if flag.starts_with('-') => {}
                _ => path = path.or(Some(word.as_str())),
            }
        }
        path
    }

    /// What a file held, when the same call or message wrote it (`-` is the send's here-document).
    pub fn file_text(&self, path: &str, send: &ShellCommand) -> Option<String> {
        if matches!(path, "-" | "/dev/stdin") {
            return send.heredoc.clone();
        }
        self.commands
            .iter()
            .rev()
            .find_map(|shell| {
                let writes = match shell.words.first().map(String::as_str) {
                    Some("cat") => shell.stdout_file.as_deref() == Some(path),
                    Some("tee") => shell.words[1..].iter().any(|word| word == path),
                    _ => false,
                };
                writes.then(|| shell.heredoc.clone()).flatten()
            })
            .or_else(|| {
                self.written
                    .iter()
                    .rev()
                    .find(|(file, _)| *file == path)
                    .map(|(_, content)| content.to_string())
            })
    }

    /// Whether a call whose output holds no send result shows that the send at command `at`
    /// failed: the CLI printed a send error, or the call failed with the send's own exit status
    /// (nothing after it but PowerShell cmdlets, which leave the native command's status).
    pub fn send_failed(&self, output: &str, call_failed: bool, at: usize) -> bool {
        if output.contains("\"status\": \"") || output.contains("\"status\":\"") {
            return false;
        }
        const SEND_ERRORS: [&str; 7] = [
            "failed or is uncertain",
            "Message body must not be empty",
            "is not an agent session",
            "Could not read message file",
            "Could not read task file",
            "exceeds the send limit",
            "Unknown or hidden agent type",
        ];
        if SEND_ERRORS.iter().any(|error| output.contains(error)) {
            return true;
        }
        call_failed
            && self.commands[at.min(self.commands.len())..]
                .iter()
                .skip(1)
                .all(|shell| {
                    shell.words.first().is_some_and(|program| {
                        matches!(
                            ascii_lower(program).as_str(),
                            "select-string"
                                | "sls"
                                | "select-object"
                                | "select"
                                | "out-string"
                                | "out-host"
                                | "where-object"
                                | "where"
                                | "?"
                                | "foreach-object"
                                | "%"
                                | "format-list"
                                | "format-table"
                        )
                    })
                })
    }
}

/// PowerShell here-strings (`@'` … `'@`, `@"` … `"@`) lifted out of a script before the POSIX
/// reader sees it, since quotes inside them would throw its quoting off: each becomes the single
/// word `__gx_here_string_<n>__`, its body the `n`th of the returned texts.
fn lift_here_strings(script: &str) -> (String, Vec<String>) {
    let mut lifted = String::with_capacity(script.len());
    let mut bodies = Vec::new();
    let mut lines = script.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let content = line.trim_end_matches(['\r', '\n']);
        let quote = match content.get(content.len().saturating_sub(2)..) {
            Some("@'") => '\'',
            Some("@\"") => '"',
            _ => {
                lifted.push_str(line);
                continue;
            }
        };
        let closer = format!("{quote}@");
        let mut body: Vec<&str> = Vec::new();
        let mut rest = None;
        for next in lines.by_ref() {
            let text = next.trim_end_matches(['\r', '\n']);
            if let Some(after) = text.strip_prefix(closer.as_str()) {
                rest = Some((after, &next[text.len()..]));
                break;
            }
            body.push(next);
        }
        let Some((after, ending)) = rest else {
            // Unterminated: leave it as written.
            lifted.push_str(line);
            body.iter().for_each(|text| lifted.push_str(text));
            continue;
        };
        lifted.push_str(&content[..content.len() - 2]);
        lifted.push_str(&format!("{HERE_STRING_MARK}{}__", bodies.len()));
        lifted.push_str(after);
        lifted.push_str(ending);
        bodies.push(
            body.concat()
                .trim_end_matches(['\r', '\n'])
                .replace("\r\n", "\n"),
        );
    }
    (lifted, bodies)
}

/// The variable a whole word names: `$msg`, `${msg}`, `$env:MSG` (lowercased; PowerShell
/// variables ignore case).
fn variable_name(word: &str) -> Option<String> {
    let name = word.strip_prefix('$')?;
    let name = name
        .strip_prefix('{')
        .and_then(|name| name.strip_suffix('}'))
        .unwrap_or(name);
    let valid = !name.is_empty()
        && !name.starts_with(|first: char| first.is_ascii_digit())
        && name
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | ':'));
    valid.then(|| ascii_lower(name))
}

/// `$msg = …` (PowerShell), `$msg=…`, or `msg=…` / `export msg=…` (POSIX): the variable and the
/// words of its value.
fn assignment(shell: &ShellCommand) -> Option<(String, Vec<String>)> {
    let words = match shell.words.first().map(String::as_str) {
        Some("export" | "local" | "declare" | "readonly") => &shell.words[1..],
        _ => &shell.words[..],
    };
    let first = words.first()?;
    if words.get(1).map(String::as_str) == Some("=") {
        return Some((variable_name(first)?, words[2..].to_vec()));
    }
    // `msg=x cmd` sets the variable for `cmd` only.
    let (name, value) = first.split_once('=')?;
    if words.len() != 1 {
        return None;
    }
    let name = if name.starts_with('$') {
        variable_name(name)?
    } else {
        variable_name(&format!("${name}"))?
    };
    Some((name, vec![value.to_string()]))
}

/// A word that is wholly a variable, a substitution or a PowerShell expression.
fn is_expansion(word: &str) -> bool {
    let word = word.trim();
    word.starts_with('`')
        || (word.starts_with('(') && word.ends_with(')'))
        || word.strip_prefix('$').is_some_and(|rest| {
            rest.starts_with(|next: char| next.is_alphabetic() || matches!(next, '_' | '{' | '('))
        })
}
