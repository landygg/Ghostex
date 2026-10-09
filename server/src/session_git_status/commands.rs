use super::*;

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const GH_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long the parent waits for the output-draining thread after the command
/// has exited successfully. A command cannot exit before it has written its
/// output, so at most a pipe buffer is ever still in flight and the thread
/// finishes at once; the grace only expires when something outlived the command
/// and its process group and still holds the write end of the pipe.
const COMMAND_READER_GRACE: Duration = Duration::from_secs(2);

/// Draining threads abandoned because their pipe outlived the command that
/// owned it. See `run_command_bounded`.
pub(super) static ABANDONED_COMMAND_READERS: AtomicUsize = AtomicUsize::new(0);

/// The same fact for the per-project `origin` probe, counted apart so the two
/// passes are distinguishable in the log: a leaked helper under
/// `project_git_remote` points at a different command set (and a different
/// user-visible symptom) than one under the session git-status probe, and one
/// shared counter would report both under the other's name.
pub(super) static ABANDONED_PROJECT_GIT_REMOTE_READERS: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// the real prober
// ---------------------------------------------------------------------------

pub struct SystemSessionGitStatusProber;

impl SessionGitStatusProber for SystemSessionGitStatusProber {
    fn supports_pull_requests(&self) -> bool {
        gh_cli_is_available()
    }

    fn probe_git(&self, cwd: &str) -> Option<SessionGitProbe> {
        let owned_cwd = cwd.to_string();
        probe_git_status_with(&move |args: &[&str]| run_git_probe_command(&owned_cwd, args))
    }

    fn probe_pull_request(&self, cwd: &str, branch: &str) -> Option<SessionPullRequest> {
        let output = run_gh_command(
            Some(cwd),
            &["pr", "view", branch, "--json", "number,state,url,isDraft"],
        )?;
        parse_gh_pull_request_json(&output)
    }
}

/*
`gh` detection is a whole-machine fact, so it is cached process-wide instead of
per cwd. `gh auth status` is the only check that answers both halves of the
question ("installed" and "logged in") and it exits non-zero for either failure,
so a machine without `gh` degrades to no PR badges with one cheap spawn every
`GH_AVAILABILITY_TTL`.
*/
pub(crate) fn gh_cli_is_available() -> bool {
    static CACHE: OnceLock<Mutex<Option<(Instant, bool)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(cache) = cache.lock() {
        if let Some((probed_at, is_available)) = *cache {
            if probed_at.elapsed() < GH_AVAILABILITY_TTL {
                return is_available;
            }
        }
    }
    let is_available = run_gh_command(None, &["auth", "status"]).is_some();
    if let Ok(mut cache) = cache.lock() {
        *cache = Some((Instant::now(), is_available));
    }
    is_available
}

/*
The shared git-probe runner: time-boxed, process-group killed on timeout, output
drained by a helper thread (see `run_command_bounded`). `project_git_remote` runs
its `origin` probe through this same plumbing (via
`run_project_git_remote_probe_command`) rather than mirroring the hardening, so
there is exactly one place where a background git spawn's safety rules live.

The only thing the two surfaces do NOT share is the abandoned-reader counter:
each passes its own, so a stranded pipe is reported under the pass that caused
it instead of under whichever pass happened to own the counter.
*/
pub fn run_git_probe_command(cwd: &str, args: &[&str]) -> Option<String> {
    run_git_probe_command_counted(cwd, args, &ABANDONED_COMMAND_READERS)
}

/// The `project_git_remote` probe's entry point into the shared runner. Same
/// hardening, own abandoned-reader counter (see `run_git_probe_command`).
pub fn run_project_git_remote_probe_command(cwd: &str, args: &[&str]) -> Option<String> {
    run_git_probe_command_counted(cwd, args, &ABANDONED_PROJECT_GIT_REMOTE_READERS)
}

fn run_git_probe_command_counted(
    cwd: &str,
    args: &[&str],
    abandoned_readers: &'static AtomicUsize,
) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        /*
        `git diff` normally refreshes the index, which takes `.git/index.lock`.
        A background probe must never contend with the user's own git, so it
        reads without taking optional locks and never blocks on a credential
        prompt.
        */
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    run_command_with_timeout(command, GIT_COMMAND_TIMEOUT, abandoned_readers)
}

pub(crate) fn run_gh_command(cwd: Option<&str>, args: &[&str]) -> Option<String> {
    let mut command = Command::new("gh");
    command
        .args(args)
        .env("PATH", crate::managed_tools::run::job_path(&[]))
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    run_command_with_timeout(command, GH_COMMAND_TIMEOUT, &ABANDONED_COMMAND_READERS)
}

/*
Time-boxed capture. stdout is drained by a helper thread for the whole lifetime
of the child: `git diff --numstat` on a large branch easily exceeds a pipe
buffer, and a poll-then-read loop would deadlock against a child blocked writing
into a full pipe. On timeout the child's whole process GROUP is killed and the
child is reaped, and the caller sees the same `None` as any other failure —
probes degrade to "no git status", never to a wedged worker.

Killing the group rather than only the child is what makes the time box real: a
killed `git` can leave a grandchild behind (an external diff driver, a
credential helper) still holding the write end of the pipe, and the draining
thread's `read_to_end` then never returns. For anything that survives even that
— a helper that detached itself out of the group — the parent waits no longer
than `COMMAND_READER_GRACE` for the drained output and abandons the thread
otherwise, so one pathological checkout can never wedge the refresh pass (a
tokio blocking worker) permanently. Abandonments are counted so the leak shows
up in the log instead of being silent.
*/
fn run_command_with_timeout(
    command: Command,
    timeout: Duration,
    abandoned_readers: &'static AtomicUsize,
) -> Option<String> {
    run_command_bounded(command, timeout, COMMAND_READER_GRACE, abandoned_readers)
}

pub(super) fn run_command_bounded(
    mut command: Command,
    timeout: Duration,
    reader_grace: Duration,
    abandoned_readers: &'static AtomicUsize,
) -> Option<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::platform::process::NoConsoleWindow::no_console_window(&mut command);
    configure_command_process_group(&mut command);
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (output_tx, output_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        let _ = output_tx.send(buffer);
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_command_process_group(&child);
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(COMMAND_POLL_INTERVAL);
            }
            Err(_) => {
                kill_command_process_group(&child);
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    /*
    A failed or timed-out command has no output worth waiting for, so the
    receiver is dropped here: the draining thread ends with the pipe the group
    kill just closed, and nothing blocks in the meantime.
    */
    if !status?.success() {
        return None;
    }
    let Ok(output) = output_rx.recv_timeout(reader_grace) else {
        abandoned_readers.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    Some(String::from_utf8_lossy(&output).trim().to_string())
}

/*
Probe commands run in their own process group so a timeout can take out
everything the command started, not just the command.
*/
fn configure_command_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    #[cfg(not(unix))]
    let _ = command;
}

#[cfg(unix)]
fn kill_command_process_group(child: &std::process::Child) {
    let process_group_id = child.id() as libc::pid_t;
    if process_group_id <= 0 {
        return;
    }
    unsafe {
        libc::kill(-process_group_id, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_command_process_group(_child: &std::process::Child) {}

/// Drains the abandoned-reader count so the refresh pass can log it. Zero on a
/// healthy machine, and a non-zero value means some probe command left output
/// readers stranded rather than that the pass itself failed.
pub fn take_abandoned_command_readers() -> usize {
    ABANDONED_COMMAND_READERS.swap(0, Ordering::Relaxed)
}

/// The same drain for the per-project `origin` probe, so its leaks are logged
/// under their own event instead of being attributed to the git-status pass.
pub fn take_abandoned_project_git_remote_readers() -> usize {
    ABANDONED_PROJECT_GIT_REMOTE_READERS.swap(0, Ordering::Relaxed)
}
