use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Paddock's own persisted state and settings: ~/.config/paddock/state.json.
#[derive(Debug, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub pinned: Vec<String>,
    /// Key that returns from a pane to the home screen, e.g. "f12" or "ctrl+a".
    #[serde(default = "default_home_key")]
    pub home_key: String,
}

fn default_home_key() -> String {
    "f12".into()
}

impl Default for State {
    fn default() -> Self {
        State {
            pinned: Vec::new(),
            home_key: default_home_key(),
        }
    }
}

impl State {
    /// The home key as a key event, or F12 when the setting cannot be parsed.
    pub fn home_key(&self) -> KeyEvent {
        parse_key(&self.home_key).unwrap_or(KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE))
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
            "esc" | "escape" => key = Some(KeyCode::Esc),
            k if k.chars().count() == 1 => key = Some(KeyCode::Char(k.chars().next()?)),
            k if k.starts_with('f') && k[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) => {
                key = Some(KeyCode::F(k[1..].parse().ok()?))
            }
            _ => return None,
        }
    }
    Some(KeyEvent::new(key?, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_specs() {
        assert_eq!(parse_key("ctrl+a"), Some(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)));
        assert_eq!(parse_key("Ctrl+Space"), Some(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert_eq!(parse_key("F12"), Some(KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE)));
        assert_eq!(parse_key("shift+f5"), Some(KeyEvent::new(KeyCode::F(5), KeyModifiers::SHIFT)));
        assert_eq!(parse_key("f99"), None);
        assert_eq!(State::default().home_key(), KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE));
    }
}
