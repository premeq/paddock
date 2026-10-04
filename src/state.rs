use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Paddock's own persisted state and settings: ~/.config/paddock/state.json.
#[derive(Debug, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub pinned: Vec<String>,
    /// Chord prefix inside a pane, e.g. "ctrl+b" or "ctrl+a".
    #[serde(default = "default_prefix")]
    pub prefix: String,
}

fn default_prefix() -> String {
    "ctrl+b".into()
}

impl Default for State {
    fn default() -> Self {
        State {
            pinned: Vec::new(),
            prefix: default_prefix(),
        }
    }
}

impl State {
    /// The prefix as a key event, or ctrl+b when the setting cannot be parsed.
    pub fn prefix_key(&self) -> KeyEvent {
        parse_key(&self.prefix).unwrap_or(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL))
    }

    fn path() -> PathBuf {
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
        base.join("paddock").join("state.json")
    }

    pub fn load() -> State {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn toggle_pin(&mut self, key: &str) {
        match self.pinned.iter().position(|k| k == key) {
            Some(i) => {
                self.pinned.remove(i);
            }
            None => self.pinned.push(key.to_owned()),
        }
        self.save();
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn parse_key(spec: &str) -> Option<KeyEvent> {
    let mut mods = KeyModifiers::NONE;
    let mut key = None;
    for part in spec.split('+') {
        match part.trim().to_lowercase().as_str() {
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "alt" | "opt" | "option" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            "space" => key = Some(KeyCode::Char(' ')),
            k if k.chars().count() == 1 => key = Some(KeyCode::Char(k.chars().next()?)),
            _ => return None,
        }
    }
    Some(KeyEvent::new(key?, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefix_specs() {
        assert_eq!(parse_key("ctrl+a"), Some(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)));
        assert_eq!(parse_key("Ctrl+Space"), Some(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert_eq!(parse_key("f12"), None);
        assert_eq!(State::default().prefix_key(), KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    }
}
