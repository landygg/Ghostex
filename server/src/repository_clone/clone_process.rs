use std::{collections::HashMap, env, sync::Arc, time::Duration};

use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    sync::{mpsc, Mutex},
    time::{sleep, Instant},
};

use super::*;

pub(super) async fn run_git_clone_process(
    args: Vec<String>,
    cwd: String,
    jobs: Arc<Mutex<HashMap<String, Value>>>,
    job_id: String,
) -> Result<CloneRunOutput, RepositoryCloneError> {
    run_clone_process("git", args, cwd, jobs, job_id).await
}

pub(super) async fn run_clone_process(
    executable: &str,
    args: Vec<String>,
    cwd: String,
    jobs: Arc<Mutex<HashMap<String, Value>>>,
    job_id: String,
) -> Result<CloneRunOutput, RepositoryCloneError> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(repository_clone_environment())
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::platform::process::NoConsoleWindow::no_console_window(&mut command);
    let mut child = command.spawn().map_err(|error| {
        RepositoryCloneError::dependency_unavailable(format!("Could not start git clone: {error}"))
    })?;
    let (limit_sender, mut limit_receiver) = mpsc::unbounded_channel();
    let stdout_task = child
        .stdout
        .take()
        .map(|stdout| tokio::spawn(read_capped_output(stdout, "stdout", limit_sender.clone())));
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(read_capped_output(stderr, "stderr", limit_sender)));
    let deadline = Instant::now() + Duration::from_millis(REPOSITORY_CLONE_TIMEOUT_MS);
    let mut terminal_error: Option<RepositoryCloneError> = None;
    let mut sigkill_deadline: Option<Instant> = None;
    let mut sigkill_sent = false;
    let mut termination_requested = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                return Err(RepositoryCloneError::dependency_unavailable(format!(
                    "git clone failed: {error}"
                )));
            }
        }
        while let Ok(limit_error) = limit_receiver.try_recv() {
            if terminal_error.is_none() {
                terminal_error = Some(limit_error);
                if !termination_requested {
                    sigkill_deadline = request_child_termination(&mut child);
                    termination_requested = true;
                }
            }
        }
        if terminal_error.is_none() && Instant::now() >= deadline {
            terminal_error = Some(RepositoryCloneError::dependency_unavailable(format!(
                "git clone timed out after {REPOSITORY_CLONE_TIMEOUT_MS}ms."
            )));
            if !termination_requested {
                sigkill_deadline = request_child_termination(&mut child);
                termination_requested = true;
            }
        }
        if terminal_error.is_none()
            && !termination_requested
            && job_is_canceled(&jobs, &job_id).await
        {
            sigkill_deadline = request_child_termination(&mut child);
            termination_requested = true;
        }
        if let Some(deadline) = sigkill_deadline {
            if !sigkill_sent && Instant::now() >= deadline {
                let _ = child.start_kill();
                sigkill_sent = true;
            }
        }
        tokio::select! {
            Some(limit_error) = limit_receiver.recv() => {
                if terminal_error.is_none() {
                    terminal_error = Some(limit_error);
                    if !termination_requested {
                        sigkill_deadline = request_child_termination(&mut child);
                        termination_requested = true;
                    }
                }
            }
            _ = sleep(Duration::from_millis(25)) => {}
        }
    };
    let stdout = join_output_task(stdout_task).await;
    let stderr = join_output_task(stderr_task).await;
    while let Ok(limit_error) = limit_receiver.try_recv() {
        if terminal_error.is_none() {
            terminal_error = Some(limit_error);
        }
    }
    if let Some(error) = terminal_error {
        return Err(error);
    }
    let was_canceled = job_is_canceled(&jobs, &job_id).await;
    Ok(CloneRunOutput {
        exit_code: status.code().unwrap_or(if was_canceled { 130 } else { 1 }),
        stderr,
        stdout,
    })
}

async fn read_capped_output<R>(
    mut reader: R,
    stream_name: &'static str,
    limit_sender: mpsc::UnboundedSender<RepositoryCloneError>,
) -> String
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut total_bytes = 0usize;
    let mut reported_limit = false;
    let mut buffer = [0u8; 8192];
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        let next_bytes = total_bytes.saturating_add(read);
        let remaining = REPOSITORY_CLONE_OUTPUT_LIMIT_BYTES.saturating_sub(total_bytes);
        if remaining > 0 {
            output.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        if next_bytes > REPOSITORY_CLONE_OUTPUT_LIMIT_BYTES && !reported_limit {
            let _ = limit_sender.send(RepositoryCloneError::dependency_unavailable(format!(
                "git clone {stream_name} exceeded {REPOSITORY_CLONE_OUTPUT_LIMIT_BYTES} bytes."
            )));
            reported_limit = true;
        }
        total_bytes = next_bytes;
    }
    String::from_utf8_lossy(&output).trim().to_string()
}

async fn join_output_task(task: Option<tokio::task::JoinHandle<String>>) -> String {
    match task {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    }
}

fn request_child_termination(child: &mut Child) -> Option<Instant> {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
            return Some(Instant::now() + Duration::from_millis(1_000));
        }
    }
    let _ = child.start_kill();
    None
}

fn repository_clone_environment() -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = env::vars().collect();
    environment.retain(|(key, _)| {
        !matches!(
            key.as_str(),
            "ANSI_COLORS_DISABLED" | "NO_COLOR" | "NODE_DISABLE_COLORS"
        )
    });
    environment
}
