use serde::{Deserialize, Serialize};

pub const MSG_SESSION_NEW: u8 = 0x01;
pub const MSG_SESSION_KILL: u8 = 0x02;
pub const MSG_SESSION_RENAME: u8 = 0x03;
pub const MSG_SESSION_LIST: u8 = 0x04;
pub const MSG_SESSION_INFO: u8 = 0x05;
pub const MSG_SESSION_ATTACH: u8 = 0x06;
pub const MSG_SESSION_DETACH: u8 = 0x07;
pub const MSG_SESSION_SEND: u8 = 0x08;
pub const MSG_SESSION_OUTPUT: u8 = 0x09;
pub const MSG_SESSION_CWD: u8 = 0x0A;
pub const MSG_SESSION_PS: u8 = 0x0B;
pub const MSG_SESSION_SCAN: u8 = 0x0C;
pub const MSG_SESSION_ADOPT: u8 = 0x0D;
pub const MSG_SESSION_RELEASE: u8 = 0x0F;
pub const MSG_SESSION_GREP: u8 = 0x11;
pub const MSG_RESIZE: u8 = 0x0E;
pub const MSG_PTY_INPUT: u8 = 0x10;
pub const MSG_DAEMON_STATUS: u8 = 0xF0;
pub const MSG_DAEMON_STOP: u8 = 0xF1;

pub const MSG_OK: u8 = 0x80;
pub const MSG_ERROR: u8 = 0x81;
pub const MSG_PTY_OUTPUT: u8 = 0x82;
pub const MSG_SESSION_EVENT: u8 = 0x83;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    SessionNew {
        shell: Option<String>,
        name: Option<String>,
        cwd: Option<String>,
    },
    SessionKill {
        target: String,
    },
    SessionRename {
        target: String,
        new_name: String,
    },
    SessionList {
        all: bool,
        #[serde(default)]
        discover: bool,
    },
    SessionInfo {
        target: String,
    },
    SessionAttach {
        target: String,
        read_only: bool,
    },
    SessionDetach,
    SessionSend {
        target: String,
        input: String,
    },
    SessionOutput {
        target: String,
        lines: Option<u32>,
        follow: bool,
    },
    SessionCwd {
        target: String,
    },
    SessionPs {
        target: String,
    },
    SessionScan,
    SessionAdopt {
        pts_or_pid: String,
        name: Option<String>,
    },
    SessionRelease {
        target: String,
    },
    SessionGrep {
        pattern: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    PtyInput(Vec<u8>),
    DaemonStatus,
    DaemonStop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Ok(ResponseData),
    Error { code: u16, message: String },
    PtyOutput(Vec<u8>),
    SessionEvent { event: String, session_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseData {
    SessionCreated {
        id: String,
    },
    SessionList(Vec<SessionInfo>),
    SessionListDiscovered {
        sessions: Vec<SessionInfo>,
        discovered: Vec<DiscoveredSession>,
    },
    SessionInfo(SessionInfo),
    Output(String),
    Cwd(String),
    ProcessInfo(Vec<ProcessEntry>),
    ScanResult(Vec<DiscoveredSession>),
    DaemonStatus {
        pid: u32,
        uptime_secs: u64,
        session_count: usize,
    },
    GrepResult(Vec<GrepMatch>),
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub session_id: String,
    pub session_name: Option<String>,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: Option<String>,
    pub shell: String,
    pub cwd: String,
    pub state: String,
    pub fg_process: Option<String>,
    pub attached: usize,
    pub adopted: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessEntry {
    pub pid: u32,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredSession {
    pub pts: String,
    pub holder_pid: u32,
    pub holder_fd: i32,
    pub shell_pid: Option<u32>,
    pub command: String,
    pub cwd: String,
    pub adoptable: bool,
}
