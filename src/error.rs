use std::fmt;

#[derive(Debug)]
#[allow(dead_code)]
pub enum SnagError {
    SessionNotFound(String),
    SessionNameConflict(String),
    SessionAmbiguousTarget(String, Vec<String>),
    SessionExited(String, i32),
    InvalidSessionName(String),

    DaemonNotRunning,
    DaemonStartFailed(String),
    DaemonAlreadyRunning,

    AdoptionFailed(String),
    KernelTooOld {
        required: &'static str,
        found: String,
    },
    PermissionDenied(String),

    Io(std::io::Error),
    Nix(nix::Error),

    ProtocolError(String),
    ConnectionLost,
}

impl fmt::Display for SnagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound(t) => write!(f, "session not found: {t}"),
            Self::SessionNameConflict(n) => write!(f, "session name already in use: {n}"),
            Self::SessionAmbiguousTarget(t, matches) => {
                write!(f, "ambiguous target '{t}', matches: {}", matches.join(", "))
            }
            Self::SessionExited(id, code) => {
                write!(f, "session {id} has exited with code {code}")
            }
            Self::InvalidSessionName(n) => {
                write!(
                    f,
                    "invalid session name '{n}': must match [a-zA-Z0-9._-] and be at most 64 characters"
                )
            }
            Self::DaemonNotRunning => write!(f, "daemon is not running"),
            Self::DaemonStartFailed(msg) => write!(f, "failed to start daemon: {msg}"),
            Self::DaemonAlreadyRunning => write!(f, "daemon is already running"),
            Self::AdoptionFailed(msg) => write!(f, "session adoption failed: {msg}"),
            Self::KernelTooOld { required, found } => {
                write!(f, "kernel too old: {required} required, found {found}")
            }
            Self::PermissionDenied(msg) => write!(f, "permission denied: {msg}"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Nix(e) => write!(f, "system error: {e}"),
            Self::ProtocolError(msg) => write!(f, "protocol error: {msg}"),
            Self::ConnectionLost => write!(f, "connection to daemon lost"),
        }
    }
}

impl std::error::Error for SnagError {}

impl From<std::io::Error> for SnagError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<nix::Error> for SnagError {
    fn from(e: nix::Error) -> Self {
        Self::Nix(e)
    }
}

pub type Result<T> = std::result::Result<T, SnagError>;
