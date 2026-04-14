use crate::error::{Result, SnagError};
use nix::libc;
use nix::pty::openpty;
use nix::sys::wait::WaitStatus;
use nix::unistd::{close, dup2, fork, setsid, ForkResult, Pid};
use std::ffi::CString;
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};

pub struct SpawnResult {
    pub master_fd: OwnedFd,
    pub child_pid: Pid,
    pub pts_path: PathBuf,
}

pub fn spawn_shell(shell: &str, cwd: &Path) -> Result<SpawnResult> {
    let pty = openpty(None, None)?;
    let master_raw = pty.master.as_raw_fd();
    let slave_raw = pty.slave.as_raw_fd();

    // Get pts path before fork
    let pts_path = get_pts_path(master_raw)?;

    match unsafe { fork()? } {
        ForkResult::Child => {
            // Close master in child
            drop(pty.master);

            // Create new session
            setsid().ok();

            // Set controlling terminal
            unsafe {
                libc::ioctl(slave_raw, libc::TIOCSCTTY, 0);
            }

            // Dup slave to stdin/stdout/stderr
            let _ = dup2(slave_raw, libc::STDIN_FILENO);
            let _ = dup2(slave_raw, libc::STDOUT_FILENO);
            let _ = dup2(slave_raw, libc::STDERR_FILENO);
            if slave_raw > libc::STDERR_FILENO {
                let _ = close(slave_raw);
            }

            // Change directory
            if std::env::set_current_dir(cwd).is_err() {
                let _ = std::env::set_current_dir("/");
            }

            // Ensure terminal environment is set for proper color and feature support.
            // The daemon may have been started from a non-terminal context.
            std::env::set_var("TERM", "xterm-256color");
            std::env::set_var("COLORTERM", "truecolor");

            // Exec shell
            let shell_cstr =
                CString::new(shell).unwrap_or_else(|_| CString::new("/bin/sh").unwrap());
            let arg_l = CString::new("-l").unwrap();
            let args = [shell_cstr.clone(), arg_l];
            let _ = nix::unistd::execvp(&shell_cstr, &args);

            // If exec fails, exit
            unsafe { libc::_exit(127) };
        }
        ForkResult::Parent { child } => {
            // Close slave in parent
            drop(pty.slave);

            Ok(SpawnResult {
                master_fd: pty.master,
                child_pid: child,
                pts_path,
            })
        }
    }
}

pub fn set_winsize(fd: RawFd, rows: u16, cols: u16) -> Result<()> {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let ret = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
    if ret < 0 {
        Err(SnagError::Io(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

pub fn get_pts_path(master_fd: RawFd) -> Result<PathBuf> {
    // Read from /proc/self/fd/<master_fd> to find the pts device
    let link = std::fs::read_link(format!("/proc/self/fd/{master_fd}"));
    if let Ok(path) = link {
        if path.to_string_lossy().starts_with("/dev/pts/") {
            return Ok(path);
        }
    }

    // Fallback: use ptsname via libc
    let ptsname_ptr = unsafe { libc::ptsname(master_fd) };
    if ptsname_ptr.is_null() {
        return Err(SnagError::Io(std::io::Error::last_os_error()));
    }
    let ptsname = unsafe { std::ffi::CStr::from_ptr(ptsname_ptr) };
    Ok(PathBuf::from(ptsname.to_str().map_err(|_| {
        SnagError::ProtocolError("invalid pts name".into())
    })?))
}

pub fn reap_child(pid: Pid) -> Option<i32> {
    match nix::sys::wait::waitpid(pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::Exited(_, code)) => Some(code),
        Ok(WaitStatus::Signaled(_, sig, _)) => Some(128 + sig as i32),
        _ => None,
    }
}

pub fn kill_session(pid: Pid) {
    let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGHUP);
}

pub fn read_cwd(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

pub fn read_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Get the foreground process group for a PTY
pub fn fg_process(pts_path: &Path) -> Vec<(u32, String)> {
    let pts_str = pts_path.to_string_lossy();
    let pts_num: Option<u32> = pts_str
        .strip_prefix("/dev/pts/")
        .and_then(|s| s.parse().ok());

    let Some(pts_num) = pts_num else {
        return Vec::new();
    };

    // Calculate expected tty_nr for /dev/pts/N
    // PTS devices have major 136 + (N / 256), minor N % 256
    let expected_tty_nr = (136 << 8) | (pts_num & 0xff);

    let mut results = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return results;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };

        // Read /proc/<pid>/stat to check tty_nr (field 7, 0-indexed)
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };

        // Parse stat — fields after comm (in parens) are space-separated
        let Some(after_comm) = stat.rfind(')').map(|i| &stat[i + 2..]) else {
            continue;
        };
        let fields: Vec<&str> = after_comm.split_whitespace().collect();
        // field index 4 (0-based after comm close) = tty_nr
        if fields.len() > 4 {
            if let Ok(tty_nr) = fields[4].parse::<u32>() {
                if tty_nr == expected_tty_nr {
                    if let Some(comm) = read_comm(pid) {
                        results.push((pid, comm));
                    }
                }
            }
        }
    }

    results
}

/// Get the foreground process name for a PTY via tcgetpgrp.
/// Returns "idle" if the shell is in the foreground, the command name
/// if another process is running, or None on error.
pub fn fg_process_name(master_fd: &impl AsFd, shell_pid: Option<Pid>) -> Option<String> {
    let pgid = nix::unistd::tcgetpgrp(master_fd).ok()?;
    if shell_pid.is_some_and(|sp| sp == pgid) {
        return Some("idle".to_string());
    }
    read_comm(pgid.as_raw() as u32).or(Some("idle".to_string()))
}
