use crate::daemon::pty;
use crate::daemon::ringbuf::RingBuffer;
use crate::error::{Result, SnagError};
use crate::protocol::SessionInfo;
use nix::unistd::Pid;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::Instant;

pub type SessionId = String;
pub type ClientId = u64;

#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    Running,
    Exited(i32),
}

#[allow(dead_code)]
pub struct Session {
    pub id: SessionId,
    pub name: Option<String>,
    pub master_fd: OwnedFd,
    pub child_pid: Option<Pid>,
    pub shell: String,
    pub pts_path: PathBuf,
    pub state: SessionState,
    pub created_at: Instant,
    pub created_at_utc: String,
    pub scrollback: RingBuffer,
    pub attached_clients: Vec<ClientId>,
    pub adopted: bool,
}

impl Session {
    pub fn new_spawned(
        id: SessionId,
        name: Option<String>,
        master_fd: OwnedFd,
        child_pid: Pid,
        shell: String,
        pts_path: PathBuf,
        scrollback_bytes: usize,
    ) -> Self {
        Self {
            id,
            name,
            master_fd,
            child_pid: Some(child_pid),
            shell,
            pts_path,
            state: SessionState::Running,
            created_at: Instant::now(),
            created_at_utc: chrono_now(),
            scrollback: RingBuffer::new(scrollback_bytes),
            attached_clients: Vec::new(),
            adopted: false,
        }
    }

    pub fn new_adopted(
        id: SessionId,
        name: Option<String>,
        master_fd: OwnedFd,
        shell_pid: Option<u32>,
        shell: String,
        pts_path: PathBuf,
        scrollback_bytes: usize,
    ) -> Self {
        Self {
            id,
            name,
            master_fd,
            child_pid: shell_pid.map(|p| Pid::from_raw(p as i32)),
            shell,
            pts_path,
            state: SessionState::Running,
            created_at: Instant::now(),
            created_at_utc: chrono_now(),
            scrollback: RingBuffer::new(scrollback_bytes),
            attached_clients: Vec::new(),
            adopted: true,
        }
    }

    pub fn to_info(&self) -> SessionInfo {
        let cwd = self
            .child_pid
            .and_then(|pid| pty::read_cwd(pid.as_raw() as u32))
            .unwrap_or_else(|| "?".to_string());

        let fg = pty::fg_process(&self.pts_path);
        let fg_process = fg
            .iter()
            .find(|(pid, _)| {
                self.child_pid
                    .map(|cp| *pid != cp.as_raw() as u32)
                    .unwrap_or(true)
            })
            .map(|(_, cmd)| cmd.clone())
            .or_else(|| {
                if fg.is_empty() {
                    None
                } else {
                    Some("idle".to_string())
                }
            });

        SessionInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            shell: self.shell.clone(),
            cwd,
            state: match &self.state {
                SessionState::Running => "running".to_string(),
                SessionState::Exited(code) => format!("exited({code})"),
            },
            fg_process,
            attached: self.attached_clients.len(),
            adopted: self.adopted,
            created_at: self.created_at_utc.clone(),
        }
    }

    pub fn raw_fd(&self) -> i32 {
        self.master_fd.as_raw_fd()
    }
}

pub fn generate_session_id() -> SessionId {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).expect("getrandom failed");
    hex::encode(bytes)
}

pub fn validate_session_name(name: &str) -> Result<()> {
    if name.len() > 64 {
        return Err(SnagError::InvalidSessionName(name.to_string()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(SnagError::InvalidSessionName(name.to_string()));
    }
    if name.is_empty() {
        return Err(SnagError::InvalidSessionName(name.to_string()));
    }
    Ok(())
}

fn chrono_now() -> String {
    // Simple UTC timestamp without chrono dependency
    use std::time::SystemTime;
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    // Format as ISO 8601
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let s = time_secs % 60;

    // Simple date calculation (good enough for display)
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{mins:02}:{s:02}Z")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    days += 719468;
    let era = days / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
