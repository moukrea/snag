#[allow(unused_imports)]
use crate::daemon::session::{Session, SessionId, SessionState};
use crate::error::{Result, SnagError};
use std::collections::HashMap;

#[allow(dead_code)]
pub struct SessionRegistry {
    sessions: HashMap<SessionId, Session>,
    name_index: HashMap<String, SessionId>,
}

#[allow(dead_code)]
impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            name_index: HashMap::new(),
        }
    }

    pub fn insert(&mut self, session: Session) {
        if let Some(ref name) = session.name {
            self.name_index.insert(name.clone(), session.id.clone());
        }
        self.sessions.insert(session.id.clone(), session);
    }

    pub fn remove(&mut self, id: &str) -> Option<Session> {
        if let Some(session) = self.sessions.remove(id) {
            if let Some(ref name) = session.name {
                self.name_index.remove(name);
            }
            Some(session)
        } else {
            None
        }
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.get_mut(id)
    }

    /// Resolve a target string to a session ID
    /// Order: exact name match, exact ID match, ID prefix match (min 3 chars)
    pub fn resolve(&self, target: &str) -> Result<SessionId> {
        // 1. Exact name match
        if let Some(id) = self.name_index.get(target) {
            return Ok(id.clone());
        }

        // 2. Exact ID match
        if self.sessions.contains_key(target) {
            return Ok(target.to_string());
        }

        // 3. ID prefix match (minimum 3 characters)
        if target.len() >= 3 {
            let matches: Vec<&SessionId> = self
                .sessions
                .keys()
                .filter(|id| id.starts_with(target))
                .collect();

            match matches.len() {
                0 => {}
                1 => return Ok(matches[0].clone()),
                _ => {
                    return Err(SnagError::SessionAmbiguousTarget(
                        target.to_string(),
                        matches.into_iter().cloned().collect(),
                    ));
                }
            }
        }

        Err(SnagError::SessionNotFound(target.to_string()))
    }

    pub fn resolve_session(&self, target: &str) -> Result<&Session> {
        let id = self.resolve(target)?;
        self.sessions
            .get(&id)
            .ok_or_else(|| SnagError::SessionNotFound(target.to_string()))
    }

    pub fn resolve_session_mut(&mut self, target: &str) -> Result<&mut Session> {
        let id = self.resolve(target)?;
        self.sessions
            .get_mut(&id)
            .ok_or_else(|| SnagError::SessionNotFound(target.to_string()))
    }

    pub fn rename(&mut self, target: &str, new_name: String) -> Result<()> {
        // Check new name isn't taken
        if self.name_index.contains_key(&new_name) {
            return Err(SnagError::SessionNameConflict(new_name));
        }

        let id = self.resolve(target)?;
        let session = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| SnagError::SessionNotFound(target.to_string()))?;

        // Remove old name from index
        if let Some(ref old_name) = session.name {
            self.name_index.remove(old_name);
        }

        // Set new name
        self.name_index.insert(new_name.clone(), id);
        session.name = Some(new_name);
        Ok(())
    }

    pub fn has_name(&self, name: &str) -> bool {
        self.name_index.contains_key(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Session> {
        self.sessions.values()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Session> {
        self.sessions.values_mut()
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn session_ids(&self) -> Vec<SessionId> {
        self.sessions.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::ringbuf::RingBuffer;
    use crate::daemon::session::SessionState;
    use nix::unistd::Pid;
    use std::time::Instant;

    fn make_test_session(id: &str, name: Option<&str>) -> Session {
        // Create a dummy fd using a pipe
        let (read_fd, _write_fd) = nix::unistd::pipe().unwrap();
        Session {
            id: id.to_string(),
            name: name.map(|s| s.to_string()),
            master_fd: read_fd,
            child_pid: Some(Pid::from_raw(1)),
            shell: "/bin/sh".to_string(),
            pts_path: std::path::PathBuf::from("/dev/pts/0"),
            state: SessionState::Running,
            created_at: Instant::now(),
            created_at_utc: "2026-01-01T00:00:00Z".to_string(),
            scrollback: RingBuffer::new(1024),
            attached_clients: Vec::new(),
            registered: false,
            capture_path: None,
            capture_abort: None,
            in_alternate_screen: false,
        }
    }

    #[test]
    fn test_insert_and_get() {
        let mut reg = SessionRegistry::new();
        let session = make_test_session("abc123", Some("dev"));
        reg.insert(session);

        assert_eq!(reg.len(), 1);
        assert!(reg.get("abc123").is_some());
        assert_eq!(reg.get("abc123").unwrap().name.as_deref(), Some("dev"));
    }

    #[test]
    fn test_resolve_exact_name() {
        let mut reg = SessionRegistry::new();
        reg.insert(make_test_session("abc123def456", Some("dev")));
        reg.insert(make_test_session("xyz789ghi012", Some("ci")));

        let resolved = reg.resolve("dev").unwrap();
        assert_eq!(resolved, "abc123def456");
    }

    #[test]
    fn test_resolve_exact_id() {
        let mut reg = SessionRegistry::new();
        reg.insert(make_test_session("abc123def456", None));

        let resolved = reg.resolve("abc123def456").unwrap();
        assert_eq!(resolved, "abc123def456");
    }

    #[test]
    fn test_resolve_id_prefix() {
        let mut reg = SessionRegistry::new();
        reg.insert(make_test_session("abc123def456", None));

        let resolved = reg.resolve("abc").unwrap();
        assert_eq!(resolved, "abc123def456");
    }

    #[test]
    fn test_resolve_prefix_too_short() {
        let mut reg = SessionRegistry::new();
        reg.insert(make_test_session("abc123def456", None));

        // 2 chars is too short for prefix match
        assert!(reg.resolve("ab").is_err());
    }

    #[test]
    fn test_resolve_ambiguous() {
        let mut reg = SessionRegistry::new();
        reg.insert(make_test_session("abc123", None));
        reg.insert(make_test_session("abc456", None));

        let err = reg.resolve("abc").unwrap_err();
        assert!(matches!(err, SnagError::SessionAmbiguousTarget(_, _)));
    }

    #[test]
    fn test_resolve_not_found() {
        let mut reg = SessionRegistry::new();
        reg.insert(make_test_session("abc123", Some("dev")));

        let err = reg.resolve("nonexistent").unwrap_err();
        assert!(matches!(err, SnagError::SessionNotFound(_)));
    }

    #[test]
    fn test_resolve_name_takes_priority() {
        let mut reg = SessionRegistry::new();
        // Session with name "abc" but different id
        reg.insert(make_test_session("xyz789012345", Some("abc")));
        // Session with id starting with "abc"
        reg.insert(make_test_session("abc123456789", None));

        // Should resolve to the named session, not the id prefix
        let resolved = reg.resolve("abc").unwrap();
        assert_eq!(resolved, "xyz789012345");
    }

    #[test]
    fn test_rename() {
        let mut reg = SessionRegistry::new();
        reg.insert(make_test_session("abc123", Some("dev")));

        reg.rename("dev", "production".to_string()).unwrap();
        assert!(reg.resolve("production").is_ok());
        assert!(reg.resolve("dev").is_err());
    }

    #[test]
    fn test_rename_conflict() {
        let mut reg = SessionRegistry::new();
        reg.insert(make_test_session("abc123", Some("dev")));
        reg.insert(make_test_session("xyz789", Some("ci")));

        let err = reg.rename("dev", "ci".to_string()).unwrap_err();
        assert!(matches!(err, SnagError::SessionNameConflict(_)));
    }

    #[test]
    fn test_remove() {
        let mut reg = SessionRegistry::new();
        reg.insert(make_test_session("abc123", Some("dev")));

        let removed = reg.remove("abc123");
        assert!(removed.is_some());
        assert_eq!(reg.len(), 0);
        assert!(reg.resolve("dev").is_err());
    }

    #[test]
    fn test_has_name() {
        let mut reg = SessionRegistry::new();
        reg.insert(make_test_session("abc123", Some("dev")));

        assert!(reg.has_name("dev"));
        assert!(!reg.has_name("ci"));
    }

    #[test]
    fn test_is_empty() {
        let reg = SessionRegistry::new();
        assert!(reg.is_empty());
    }
}
