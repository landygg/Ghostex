use std::{
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Result;

/// The child gets a console of its own that is never shown.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The child leads its own Ctrl+C group.
#[cfg(windows)]
pub(crate) const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// CDXC:PlatformSupport 2026-10-09 WHY:
/// The desktop app is a GUI program with no console, so every console program it starts (wsl, PowerShell, git, node, the `ghostex` CLI, gxserver) gets a fresh console, which Windows Terminal (the default terminal on Windows 11) shows as a window that flashes on the user's screen. Piped output does not stop it and `-WindowStyle Hidden` hides it only after it has appeared, so every background start opts out with CREATE_NO_WINDOW when the process is created, through this one helper (or its constants when a start needs more flags). Starts the user asked to see (installers, an editor or app they chose, Explorer) do not use it.
/// SEE-ALSO: `NoConsoleWindow` in server/src/platform/process.rs, the same rule for gxserver and the `ghostex` CLI.
pub(crate) fn gpui_background_command(
    program: impl AsRef<std::ffi::OsStr>,
) -> std::process::Command {
    let mut process = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        process.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = &mut process;
    process
}

pub(crate) fn gpui_run_command_with_timeout(
    command: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<bool, String> {
    let mut process = gpui_background_command(command);
    let mut child = process
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| "process spawn failed".to_string())?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(false);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(false);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GpuiCapturedCommandOutput {
    pub(crate) stderr: String,
    pub(crate) stdout: String,
    pub(crate) success: bool,
}

impl GpuiCapturedCommandOutput {
    pub(crate) fn combined_text(&self) -> String {
        if self.stderr.trim().is_empty() {
            return self.stdout.clone();
        }
        if self.stdout.trim().is_empty() {
            return self.stderr.clone();
        }
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

pub(crate) fn gpui_run_command_with_captured_output_timeout(
    command: &Path,
    args: &[&str],
    timeout: Duration,
    max_capture_bytes: usize,
) -> Result<GpuiCapturedCommandOutput, String> {
    let mut process = gpui_background_command(command);
    let mut child = process
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| "process spawn failed".to_string())?;
    let stdout_reader = child
        .stdout
        .take()
        .map(|stream| gpui_capture_child_output_stream(stream, max_capture_bytes));
    let stderr_reader = child
        .stderr
        .take()
        .map(|stream| gpui_capture_child_output_stream(stream, max_capture_bytes));
    let started = Instant::now();
    let mut success = false;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                success = status.success();
                break;
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    let stdout = gpui_join_captured_output(stdout_reader);
    let stderr = gpui_join_captured_output(stderr_reader);
    Ok(GpuiCapturedCommandOutput {
        stderr,
        stdout,
        success,
    })
}

pub(crate) fn gpui_capture_child_output_stream<R>(
    mut stream: R,
    max_capture_bytes: usize,
) -> std::thread::JoinHandle<Vec<u8>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut captured = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(byte_count) => {
                    let remaining = max_capture_bytes.saturating_sub(captured.len());
                    if remaining > 0 {
                        captured.extend_from_slice(&buffer[..byte_count.min(remaining)]);
                    }
                }
                Err(_) => break,
            }
        }
        captured
    })
}

pub(crate) fn gpui_join_captured_output(
    reader: Option<std::thread::JoinHandle<Vec<u8>>>,
) -> String {
    reader
        .and_then(|reader| reader.join().ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}
