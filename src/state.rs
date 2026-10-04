use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Paddock's own persisted state: ~/.config/paddock/state.json.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub pinned: Vec<String>,
}

impl State {
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

    fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}
