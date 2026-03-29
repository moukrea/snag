use crate::protocol::SessionInfo;

#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    Rename,
    Send,
}

pub struct App {
    pub sessions: Vec<SessionInfo>,
    pub selected: usize,
    pub should_quit: bool,
    pub preview_raw: String,
    pub input_mode: InputMode,
    pub input_buffer: String,
    pub hide_snagged: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            should_quit: false,
            preview_raw: String::new(),
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            hide_snagged: false,
        }
    }

    /// Returns the filtered list of sessions based on hide_snagged toggle.
    pub fn visible_sessions(&self) -> Vec<&SessionInfo> {
        self.sessions
            .iter()
            .filter(|s| !self.hide_snagged || s.snagged_by.is_none())
            .collect()
    }

    pub fn select_next(&mut self) {
        let count = self.visible_sessions().len();
        if count > 0 {
            self.selected = (self.selected + 1) % count;
        }
    }

    pub fn select_prev(&mut self) {
        let count = self.visible_sessions().len();
        if count > 0 {
            self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
        }
    }

    pub fn selected_session(&self) -> Option<&SessionInfo> {
        let visible = self.visible_sessions();
        visible.get(self.selected).copied()
    }

    pub fn selected_id(&self) -> Option<String> {
        self.selected_session().map(|s| s.id.clone())
    }

    pub fn toggle_hide_snagged(&mut self) {
        self.hide_snagged = !self.hide_snagged;
        let count = self.visible_sessions().len();
        if self.selected >= count && count > 0 {
            self.selected = count - 1;
        }
    }

    pub fn enter_input_mode(&mut self, mode: InputMode) {
        self.input_mode = mode;
        self.input_buffer.clear();
    }

    pub fn cancel_input(&mut self) {
        self.input_mode = InputMode::Normal;
        self.input_buffer.clear();
    }
}
