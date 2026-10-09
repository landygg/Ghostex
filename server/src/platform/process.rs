use std::{ffi::OsStr, process::Command};

/// The child gets a console of its own that is never shown.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The child leads its own Ctrl+C group, so a console signal to the parent does not reach it.
#[cfg(windows)]
pub(crate) const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// CDXC:PlatformSupport 2026-10-09 WHY:
/// Every console program Ghostex starts for its own work (git, gh, wmx helpers, PowerShell probes, agent CLIs, tailcat) runs in the user's desktop session. A console program whose parent has no console (the desktop app is a GUI program; a daemon started without one) gets a fresh console, and with Windows Terminal as the default terminal that console opens as a Terminal window that flashes on the user's screen; `-WindowStyle Hidden` hides it only after it has appeared, and piping the output does not stop it. So every background start opts out when the process is created, through this one helper. Starts the user asked to see (UAC prompts, installers, an editor or terminal they opened) do not use it.
pub(crate) trait NoConsoleWindow {
    /// Starts the child with CREATE_NO_WINDOW on Windows; a no-op elsewhere. `creation_flags` replaces earlier flags, so a caller that needs more passes `CREATE_NO_WINDOW | …` itself.
    fn no_console_window(&mut self) -> &mut Self;
}

impl NoConsoleWindow for Command {
    fn no_console_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

impl NoConsoleWindow for tokio::process::Command {
    fn no_console_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }
}

/// A `Command` for background work that never shows a console window (`NoConsoleWindow`).
pub(crate) fn background_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.no_console_window();
    command
}

/// CDXC:PlatformSupport 2026-10-06 WHY:
/// Every helper the daemon starts (wmx `watch-title`/`history`/`list`, git, probes) inherits its error mode. While Windows shuts down, a helper that starts can no longer initialise user32 and fails with 0xc0000142, and with the default mode Windows showed an "unable to start correctly" dialog for each one that the user had to click through. Helpers are invisible background work, so their load failures and crashes end quietly instead; session shells opt back into the default mode (`process` in zmx/scripts_windows.rs).
#[cfg(windows)]
pub(crate) fn suppress_helper_error_dialogs() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        GetErrorMode, SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX,
    };
    unsafe {
        SetErrorMode(GetErrorMode() | SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
}

/// The process's creation time as a Windows FILETIME (100ns ticks since 1601), or `None` once it has exited or cannot be opened.
#[cfg(windows)]
pub(crate) fn process_creation_filetime(process_id: i64) -> Option<u64> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    let pid = u32::try_from(process_id).ok().filter(|pid| *pid != 0)?;
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut created: FILETIME = std::mem::zeroed();
        let mut exited: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let read = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
        CloseHandle(process);
        (read != 0)
            .then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
}

/// Whether `pid` names a process that has not exited: `runtime::is_process_running` on Unix, and
/// on Windows (where that answers `false` for every pid) the process's exit code still being
/// STILL_ACTIVE.
pub(crate) fn process_is_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, STILL_ACTIVE},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        if pid == 0 {
            return false;
        }
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut code = 0u32;
            let read = GetExitCodeProcess(process, &mut code);
            CloseHandle(process);
            read != 0 && code == STILL_ACTIVE as u32
        }
    }
    #[cfg(not(windows))]
    {
        crate::runtime::is_process_running(pid)
    }
}

/// Whether this process runs an executable image that is no longer the installed file at its path.
///
/// CDXC:ServerDaemon 2026-09-28 WHY:
/// The desktop refreshes the managed Windows package by renaming the old binaries into `.retired-native` while they may still be running. A gxserver started during that refresh kept executing the retired image, but `build-identity.json` already named the new build, so health reported the new identity and the app never restarted it: a day of installed fixes never ran (the chat Subagents card stayed empty). Compare the file mapped as this image with the installed path so a superseded daemon reports a mismatched identity and the app restarts it.
/// SEE-ALSO: `refresh` in apps/desktop/src/windows_terminal_backend/native_package.rs; `gpui_probe_local_gxserver_health_with_diagnostics` in apps/desktop/src/app/helpers/board_gxserver/gxserver_health_and_daemon.rs.
#[cfg(windows)]
pub(crate) fn running_image_superseded() -> bool {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::{
        ProcessStatus::K32GetMappedFileNameW, Threading::GetCurrentProcess,
    };
    // `current_exe` is the path the loader recorded at startup, not where the image lives now.
    let Ok(installed) = std::env::current_exe().and_then(std::fs::canonicalize) else {
        return false;
    };
    let mut name = vec![0u16; 32_768];
    // Any address inside this executable names the file its image section maps, renames included.
    let length = unsafe {
        K32GetMappedFileNameW(
            GetCurrentProcess(),
            running_image_superseded as *const std::ffi::c_void,
            name.as_mut_ptr(),
            name.len() as u32,
        )
    } as usize;
    if length == 0 {
        return false;
    }
    let mut mapped = std::ffi::OsString::from(r"\\?\GLOBALROOT");
    mapped.push(std::ffi::OsString::from_wide(&name[..length]));
    std::fs::canonicalize(mapped).is_ok_and(|running| running != installed)
}

/// CDXC:ServerDaemon 2026-09-29 WHY:
/// Linux marks the executable of a process whose file was replaced (a reinstall or update that kept gxserver running) as `<path> (deleted)`, and `current_exe` returns that path. gxserver writes its own path into session restore scripts and agent hooks, so a wake ran `…/gxserver (deleted) resume-lookup` and failed with "Unable to restore Codex session". Such a daemon reports a superseded identity, like Windows above, so the app restarts it on the installed binary.
#[cfg(target_os = "linux")]
pub(crate) fn running_image_superseded() -> bool {
    std::fs::read_link("/proc/self/exe")
        .is_ok_and(|path| path.to_string_lossy().ends_with(" (deleted)"))
}

/// CDXC:RemoteMachines 2026-09-14 WHY:
/// A server started over Windows SSH must outlive the exec channel without keeping
/// that channel's inheritable handles open. Otherwise the CLI exits but the phone
/// waits forever for EOF. Spawn this detached control plane without handle inheritance.
///
/// CDXC:PlatformSupport 2026-09-28 WHY:
/// The server gets its own windowless console (CREATE_NO_WINDOW), not DETACHED_PROCESS: with no console at all, every console child it started without its own flag opened a Windows Terminal window. The new console is still separate from the launcher's, so it outlives an SSH channel or terminal too.
///
/// CDXC:PlatformSupport 2026-10-04 WHY:
/// Since the user's "Prevent and cure" decision (`server_placement` in platform/desktop_session.rs), a caller outside the desktop spawns gxserver itself only when nobody is signed in to the desktop; a caller in the desktop session (an administrator terminal, still given standard rights) spawns it here too.
/// SEE-ALSO: `gpui_spawn_local_gxserver_daemon` in apps/desktop/src/app/helpers/board_gxserver/gxserver_health_and_daemon.rs.
#[cfg(windows)]
pub(crate) fn spawn_server(executable: &OsStr) -> std::io::Result<u32> {
    use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            CreateProcessAsUserW, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
        },
    };
    let standard_user = super::standard_user::standard_user_token_if_elevated()?;
    let application: Vec<u16> = executable.encode_wide().chain(Some(0)).collect();
    let mut arguments = vec![u16::from(b'"')];
    arguments.extend(executable.encode_wide());
    arguments.extend("\" --foreground".encode_utf16());
    arguments.push(0);
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // CREATE_BREAKAWAY_FROM_JOB (dropped when the job denies it) | CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP.
    for flags in [0x0900_0200, 0x0800_0200] {
        let mut command_line = arguments.clone();
        let created = unsafe {
            match &standard_user {
                Some(token) => CreateProcessAsUserW(
                    token.as_raw_handle(),
                    application.as_ptr(),
                    command_line.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    flags,
                    std::ptr::null(),
                    std::ptr::null(),
                    &startup,
                    &mut process,
                ),
                None => CreateProcessW(
                    application.as_ptr(),
                    command_line.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    flags,
                    std::ptr::null(),
                    std::ptr::null(),
                    &startup,
                    &mut process,
                ),
            }
        };
        if created != 0 {
            unsafe {
                CloseHandle(process.hThread);
                CloseHandle(process.hProcess);
            }
            return Ok(process.dwProcessId);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(5) {
            return Err(error);
        }
    }
    Err(std::io::Error::last_os_error())
}
