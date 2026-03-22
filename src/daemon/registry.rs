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
