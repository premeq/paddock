use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{GitInfo, Machine, Snapshot};

/// Runs `herdr` either locally or routed to a saved SSH machine.
#[derive(Debug, Clone)]
pub struct Runner {
    pub machine: Option<String>,
    pub ssh_target: Option<String>,
    pub session: Option<String>,
}

impl Runner {
    pub fn local() -> Self {
        Runner {
            machine: None,
            ssh_target: None,
            session: None,
        }
    }

    pub fn for_machine(m: &Machine) -> Self {
        if m.local {
            Runner::local()
        } else {
            Runner {
                machine: Some(m.id.clone()),
                ssh_target: m.ssh_target.clone(),
                session: m.session.clone(),
            }
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new("herdr");
        if let Some(m) = &self.machine {
            c.arg("--machine").arg(m);
        }
        c.args(args);
        c.stdin(Stdio::null());
        c
    }

    pub fn json(&self, args: &[&str]) -> Result<Value> {
        let out = self
            .command(args)
            .output()
            .with_context(|| format!("spawn herdr {}", args.join(" ")))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let v: Value = serde_json::from_str(text.trim()).map_err(|_| {
            let err = String::from_utf8_lossy(&out.stderr);
            anyhow!("{}", first_line(if err.trim().is_empty() { &text } else { &err }))
        })?;
        if let Some(e) = v.get("error") {
            bail!(
                "{}",
                e.get("message").and_then(Value::as_str).unwrap_or("herdr error")
            );
        }
        Ok(v.get("result").cloned().unwrap_or(v))
    }

    pub fn text(&self, args: &[&str]) -> Result<String> {
        let out = self.command(args).output()?;
        if !out.status.success() {
            bail!("{}", first_line(&String::from_utf8_lossy(&out.stderr)));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let lister = self.clone();
        let list_thread = std::thread::spawn(move || lister.json(&["workspace", "list"]));
        let v = self.json(&["api", "snapshot"])?;
        let snap = v.get("snapshot").cloned().unwrap_or(v);
        let mut snapshot: Snapshot = serde_json::from_value(snap)?;
        let list = list_thread.join().map_err(|_| anyhow!("workspace list thread panicked"))??;
        if let Some(ws) = list.get("workspaces") {
            let detailed: Vec<crate::model::Workspace> = serde_json::from_value(ws.clone())?;
            let by_id: HashMap<String, crate::model::Workspace> =
                detailed.into_iter().map(|w| (w.workspace_id.clone(), w)).collect();
            for w in &mut snapshot.workspaces {
                if let Some(d) = by_id.get(&w.workspace_id) {
                    w.worktree = d.worktree.clone();
                }
            }
        }
        Ok(snapshot)
    }

    /// Runs a shell script on the machine that owns the panes (local sh or ssh).
    pub fn shell(&self, script: &str) -> Result<String> {
        let mut c = match &self.ssh_target {
            Some(target) => {
                let mut c = Command::new("ssh");
                c.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", target, "sh", "-c"]);
                c.arg(shell_quote(script));
                c
            }
            None => {
                let mut c = Command::new("sh");
                c.arg("-c").arg(script);
                c
            }
        };
        let out = c.stdin(Stdio::null()).output()?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn git_info(&self, paths: &[(String, String)]) -> HashMap<String, GitInfo> {
        if paths.is_empty() {
            return HashMap::new();
        }
        let mut script = String::new();
        for (id, path) in paths {
            script.push_str(&format!(
                "p={p}; b=$(git -C \"$p\" rev-parse --abbrev-ref HEAD 2>/dev/null); \
                 ab=$(git -C \"$p\" rev-list --left-right --count HEAD...@{{u}} 2>/dev/null | tr '\\t' ' '); \
                 n=$(git -C \"$p\" status --porcelain 2>/dev/null | wc -l | tr -d ' '); \
                 printf '%s\\t%s\\t%s\\t%s\\n' {id} \"$b\" \"$ab\" \"$n\";\n",
                p = shell_quote(path),
                id = shell_quote(id)
            ));
        }
        let mut out = HashMap::new();
        let Ok(text) = self.shell(&script) else {
            return out;
        };
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 4 || f[1].is_empty() {
                continue;
            }
            let mut ab = f[2].split_whitespace().map(|n| n.parse().unwrap_or(0));
            out.insert(
                f[0].to_owned(),
                GitInfo {
                    branch: f[1].to_owned(),
                    ahead: ab.next().unwrap_or(0),
                    behind: ab.next().unwrap_or(0),
                    changed: f[3].parse().unwrap_or(0),
                },
            );
        }
        out
    }

    pub fn session_control_argv(&self, pane_id: &str, cols: u16, rows: u16) -> Vec<String> {
        let (cols, rows) = (cols.to_string(), rows.to_string());
        match &self.ssh_target {
            Some(target) => {
                let mut cmd = String::from("exec herdr");
                if let Some(s) = &self.session {
                    cmd.push_str(&format!(" --session {}", shell_quote(s)));
                }
                cmd.push_str(&format!(
                    " terminal session control {} --cols {cols} --rows {rows}",
                    shell_quote(pane_id)
                ));
                ["ssh", "-o", "BatchMode=yes", "-S", "none", target, &cmd].map(String::from).to_vec()
            }
            None => ["herdr", "terminal", "session", "control", pane_id, "--cols", &cols, "--rows", &rows]
                .map(String::from)
                .to_vec(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SavedMachine {
    id: String,
    label: String,
    target: String,
    #[serde(default)]
    session: Option<String>,
    #[serde(default = "yes")]
    enabled: bool,
}

fn yes() -> bool {
    true
}

pub fn saved_machines() -> Result<Vec<Machine>> {
    let out = Command::new("herdr")
        .args(["machine", "list", "--json"])
        .stdin(Stdio::null())
        .output()?;
    let list: Vec<SavedMachine> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    Ok(list
        .into_iter()
        .filter(|m| m.enabled)
        .map(|m| {
            Machine::remote(
                m.id,
                m.label,
                m.target,
                m.session.unwrap_or_else(|| "default".into()),
            )
        })
        .collect())
}

pub enum Update {
    Snapshot {
        machine: String,
        snapshot: Snapshot,
        latency_ms: u64,
    },
    Git {
        machine: String,
        info: HashMap<String, GitInfo>,
    },
    Failed {
        machine: String,
        error: String,
    },
    PaneText {
        machine: String,
        pane_id: String,
        text: String,
    },
}

pub fn fetch_pane_text(machine: &Machine, pane_id: String, tx: Sender<Update>) {
    let runner = Runner::for_machine(machine);
    let id = machine.id.clone();
    std::thread::spawn(move || {
        let text = runner
            .text(&["pane", "read", &pane_id, "--source", "visible", "--lines", "60"])
            .unwrap_or_default();
        let _ = tx.send(Update::PaneText {
            machine: id,
            pane_id,
            text,
        });
    });
}

pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./:@~".contains(&b)) {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("unknown error").trim().to_owned()
}
