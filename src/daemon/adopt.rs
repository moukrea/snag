use crate::error::{Result, SnagError};
use crate::protocol::DiscoveredSession;
use nix::libc;
use nix::unistd::getuid;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

/// Scan /proc for PTY master fds held by processes owned by the current user
pub fn scan_pty_sessions() -> Result<Vec<DiscoveredSession>> {
    let uid = getuid();
    let mut sessions = Vec::new();

    let entries = std::fs::read_dir("/proc")
        .map_err(|e| SnagError::AdoptionFailed(format!("cannot read /proc: {e}")))?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };

        // Check if this process belongs to our user
        let stat_path = format!("/proc/{pid}/status");
        let Ok(status) = std::fs::read_to_string(&stat_path) else {
            continue;
        };
        let proc_uid = status
            .lines()
            .find(|l| l.starts_with("Uid:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u32>().ok());

        if proc_uid != Some(uid.as_raw()) {
            continue;
        }

        // Check each fd for PTY masters
        let fd_dir = format!("/proc/{pid}/fd");
        let Ok(fd_entries) = std::fs::read_dir(&fd_dir) else {
            continue;
        };

        for fd_entry in fd_entries.flatten() {
            let fd_name = fd_entry.file_name();
            let Some(fd_num) = fd_name.to_str().and_then(|s| s.parse::<i32>().ok()) else {
                continue;
            };

            let Ok(link) = std::fs::read_link(fd_entry.path()) else {
                continue;
            };
            let link_str = link.to_string_lossy();

            // PTY master fds link to /dev/ptmx or /dev/pts/ptmx
            if !link_str.contains("ptmx") {
                continue;
            }

            // Find which PTS this master controls by checking /proc/<pid>/fdinfo/<fd>
            // The pts number can be found via TIOCGPTN-like info in fdinfo
            let pts_num = get_pts_number_for_fd(pid, fd_num);
            let Some(pts_num) = pts_num else {
                continue;
            };

            let pts_path = format!("/dev/pts/{pts_num}");

            // Find the shell process on the slave side
            let (shell_pid, command, cwd) = find_slave_process(pts_num);

            sessions.push(DiscoveredSession {
                pts: pts_path,
                holder_pid: pid,
                holder_fd: fd_num,
                shell_pid,
                command,
                cwd,
                adoptable: true,
            });
        }
    }

    // Deduplicate by PTS
    sessions.sort_by(|a, b| a.pts.cmp(&b.pts));
    sessions.dedup_by(|a, b| a.pts == b.pts);

    Ok(sessions)
}

fn get_pts_number_for_fd(pid: u32, fd: i32) -> Option<u32> {
    // Read /proc/<pid>/fdinfo/<fd> and look for tty-index
    let fdinfo_path = format!("/proc/{pid}/fdinfo/{fd}");
    if let Ok(info) = std::fs::read_to_string(&fdinfo_path) {
        for line in info.lines() {
            if let Some(rest) = line.strip_prefix("tty-index:") {
                return rest.trim().parse().ok();
            }
        }
    }

    // Fallback: try to resolve via /proc/<pid>/fd/<fd> -> /dev/pts/<N> symlink.
    // Some PTY masters (e.g., opened via /dev/pts/ptmx) have a sibling slave fd
    // pointing to /dev/pts/<N>. Scan adjacent fds for one linked to /dev/pts/<N>.
    let fd_dir = format!("/proc/{pid}/fd");
    if let Ok(entries) = std::fs::read_dir(&fd_dir) {
        for entry in entries.flatten() {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                let s = target.to_string_lossy();
                if let Some(rest) = s.strip_prefix("/dev/pts/") {
                    if rest != "ptmx" {
                        if let Ok(n) = rest.parse::<u32>() {
                            return Some(n);
                        }
                    }
                }
            }
        }
    }
    None
}

fn find_slave_process(pts_num: u32) -> (Option<u32>, String, String) {
    let expected_tty_nr = (136u32 << 8) | (pts_num & 0xff);

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return (None, String::new(), String::new());
    };

    let mut best_pid: Option<u32> = None;
    let mut best_comm = String::new();
    let mut best_cwd = String::new();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };

        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };

        let Some(after_comm) = stat.rfind(')').map(|i| &stat[i + 2..]) else {
            continue;
        };
        let fields: Vec<&str> = after_comm.split_whitespace().collect();

        // field 4 (0-based after ')') = tty_nr
        if fields.len() > 4 {
            if let Ok(tty_nr) = fields[4].parse::<u32>() {
                if tty_nr == expected_tty_nr {
                    // Check if this is a session leader (field 3 = pgrp matches field 1 = session)
                    let is_session_leader = fields.len() > 3 && fields[1] == fields[3];

                    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();

                    // Prefer session leaders (shells) over child processes
                    if best_pid.is_none() || is_session_leader {
                        best_pid = Some(pid);
                        best_comm = comm;
                        best_cwd = cwd;
                    }
                }
            }
        }
    }

    (best_pid, best_comm, best_cwd)
}

/// Adopt a PTY master fd from another process using pidfd_getfd
pub fn adopt_pty(holder_pid: u32, holder_fd: i32) -> Result<OwnedFd> {
    // pidfd_open: syscall 434 on x86_64
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, holder_pid as libc::pid_t, 0) };
    if pidfd < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EPERM) {
            return Err(SnagError::AdoptionFailed(format!(
                "permission denied: cannot open pidfd for PID {holder_pid}. \
                 Ensure kernel.yama.ptrace_scope=0: sudo sysctl kernel.yama.ptrace_scope=0"
            )));
        }
        return Err(SnagError::AdoptionFailed(format!(
            "pidfd_open failed for PID {holder_pid}: {err}"
        )));
    }

    // pidfd_getfd: syscall 438 on x86_64
    let our_fd =
        unsafe { libc::syscall(libc::SYS_pidfd_getfd, pidfd as libc::c_int, holder_fd, 0u32) };

    // Close pidfd
    unsafe { libc::close(pidfd as libc::c_int) };

    if our_fd < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EPERM) {
            let ptrace_scope = std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope")
                .unwrap_or_default()
                .trim()
                .to_string();
            return Err(SnagError::AdoptionFailed(format!(
                "permission denied: cannot access fd {holder_fd} of PID {holder_pid}. \
                 kernel.yama.ptrace_scope is currently {ptrace_scope} (needs 0). \
                 Fix with: sudo sysctl kernel.yama.ptrace_scope=0"
            )));
        }
        return Err(SnagError::AdoptionFailed(format!(
            "pidfd_getfd failed for PID {holder_pid} fd {holder_fd}: {err}"
        )));
    }

    Ok(unsafe { OwnedFd::from_raw_fd(our_fd as RawFd) })
}
