use crate::protocol::SessionInfo;

pub struct App {
    pub sessions: Vec<SessionInfo>,
    pub selected: usize,
    pub show_adopted: bool,
    pub should_quit: bool,
    pub preview_lines: Vec<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            show_adopted: false,
            should_quit: false,
            preview_lines: Vec::new(),
        }
    }

    pub fn select_next(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = (self.selected + 1) % self.sessions.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.sessions.len() - 1);
        }
    }

    pub fn selected_session(&self) -> Option<&SessionInfo> {
        self.sessions.get(self.selected)
    }

    pub fn selected_id(&self) -> Option<String> {
        self.selected_session().map(|s| s.id.clone())
    }
}
