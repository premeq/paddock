use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Blocked,
    Working,
    Done,
    Idle,
    Unknown,
}

impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Status::Blocked => "blocked",
            Status::Working => "working",
            Status::Done => "done",
            Status::Idle => "idle",
            Status::Unknown => "unknown",
        }
    }
    pub fn dot(self) -> &'static str {
        match self {
            Status::Blocked | Status::Working | Status::Done => "●",
            Status::Idle => "○",
            Status::Unknown => "·",
        }
    }
    pub fn needs_you(self) -> bool {
        matches!(self, Status::Blocked | Status::Done)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    pub label: String,
    pub active_tab_id: String,
    #[serde(default)]
    pub worktree: Option<Worktree>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Worktree {
    pub checkout_path: String,
    pub is_linked_worktree: bool,
    pub repo_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tab {
    pub tab_id: String,
    pub workspace_id: String,
    pub label: String,
    pub pane_count: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pane {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub agent_status: Status,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub focused: bool,
}

impl Pane {
    pub fn cwd(&self) -> &str {
        self.foreground_cwd
            .as_deref()
            .or(self.cwd.as_deref())
            .unwrap_or("")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Snapshot {
    pub workspaces: Vec<Workspace>,
    pub tabs: Vec<Tab>,
    pub panes: Vec<Pane>,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GitInfo {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub changed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Online,
    Offline,
    Connecting,
}

#[derive(Debug, Clone)]
pub struct Machine {
    pub id: String,
    pub label: String,
    pub local: bool,
    pub ssh_target: Option<String>,
    pub session: Option<String>,
    pub link: Link,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
    pub snapshot: Snapshot,
    pub git: HashMap<String, GitInfo>,
}

impl Machine {
    pub fn local() -> Self {
        Machine {
            id: "local".into(),
            label: "Local".into(),
            local: true,
            ssh_target: None,
            session: None,
            link: Link::Connecting,
            latency_ms: None,
            error: None,
            snapshot: Snapshot::default(),
            git: HashMap::new(),
        }
    }
    pub fn remote(id: String, label: String, target: String, session: String) -> Self {
        Machine {
            id,
            label,
            local: false,
            ssh_target: Some(target),
            session: Some(session),
            link: Link::Connecting,
            latency_ms: None,
            error: None,
            snapshot: Snapshot::default(),
            git: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Machine { machine: String },
    Workspace { machine: String, workspace_id: String },
    Pane { machine: String, pane_id: String },
    /// A pane listed again under "Needs you"; same pane, distinct row identity.
    Attention { machine: String, pane_id: String },
    Action(Action),
    Separator(u8),
}

impl Target {
    /// Folds the Needs-you alias onto the pane it points at.
    pub fn resolved(&self) -> Target {
        match self {
            Target::Attention { machine, pane_id } => Target::Pane {
                machine: machine.clone(),
                pane_id: pane_id.clone(),
            },
            t => t.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NewWorkspace,
    ConnectMachine,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub depth: u8,
    pub label: String,
    pub right: Vec<(String, Tone)>,
    pub status: Option<Status>,
    pub glyph: Option<&'static str>,
    pub last_child: bool,
    pub current: bool,
    pub bold: bool,
    pub dimmed: bool,
    pub target: Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Dim,
    Status(Status),
    Online,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Blocked,
    Working,
    Idle,
    Done,
}

impl Filter {
    pub fn text(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Blocked => "blocked",
            Filter::Working => "working",
            Filter::Idle => "idle",
            Filter::Done => "done",
        }
    }
    fn keep(self, status: Status) -> bool {
        match self {
            Filter::All => true,
            Filter::Blocked => status == Status::Blocked,
            Filter::Working => status == Status::Working,
            Filter::Idle => status == Status::Idle,
            Filter::Done => status == Status::Done,
        }
    }
}

pub fn shorten_path(path: &str, home: &str, max: usize) -> String {
    let p = match home_prefix(path, home) {
        Some(n) => format!("~{}", &path[n..]),
        None => path.to_owned(),
    };
    if p.chars().count() <= max {
        return p;
    }
    let leaf = p.rsplit('/').next().unwrap_or(&p);
    let root = if p.starts_with('~') { "~" } else { "" };
    format!("{root}/…/{leaf}")
}

fn home_prefix(path: &str, home: &str) -> Option<usize> {
    if !home.is_empty() && path.starts_with(home) {
        return Some(home.len());
    }
    for root in ["/home/", "/Users/"] {
        if let Some(rest) = path.strip_prefix(root) {
            let user = rest.split('/').next()?;
            if !user.is_empty() {
                return Some(root.len() + user.len());
            }
        }
    }
    None
}

pub struct Rows {
    pub rows: Vec<Row>,
    pub counts: Counts,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Counts {
    pub machines: usize,
    pub workspaces: usize,
    pub agents: usize,
    pub blocked: usize,
    pub working: usize,
    pub idle: usize,
    pub done: usize,
}

fn matches(words: &[String], text: &str) -> bool {
    let lower = text.to_lowercase();
    words.iter().all(|w| lower.contains(w.as_str()))
}

pub fn build_rows(machines: &[Machine], query: &str, filter: Filter, home: &str, pinned: &[String]) -> Rows {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let filtering = !words.is_empty() || filter != Filter::All;
    let mut counts = Counts {
        machines: machines.len(),
        ..Counts::default()
    };
    let mut rows = Vec::new();
    let mut needs = Vec::new();

    for m in machines {
        let dimmed = m.link != Link::Online;
        counts.workspaces += m.snapshot.workspaces.len();
        let mut groups: Vec<(bool, Vec<Row>)> = Vec::new();
        let mut agent_names: HashMap<&str, usize> = HashMap::new();
        for p in &m.snapshot.panes {
            if p.agent.is_some() {
                counts.agents += 1;
                match p.agent_status {
                    Status::Blocked => counts.blocked += 1,
                    Status::Working => counts.working += 1,
                    Status::Idle => counts.idle += 1,
                    Status::Done => counts.done += 1,
                    Status::Unknown => {}
                }
            }
            *agent_names.entry(p.tab_id.as_str()).or_default() += 1;
        }
        for ws in &m.snapshot.workspaces {
            let ws_match = matches(&words, &ws.label) || matches(&words, &m.label);
            let tabs: Vec<&Tab> = m
                .snapshot
                .tabs
                .iter()
                .filter(|t| t.workspace_id == ws.workspace_id)
                .collect();
            let mut children = Vec::new();
            for tab in &tabs {
                let panes: Vec<&Pane> = m
                    .snapshot
                    .panes
                    .iter()
                    .filter(|p| p.tab_id == tab.tab_id)
                    .collect();
                for (ix, p) in panes.iter().enumerate() {
                    let kind = p.agent.as_deref();
                    let name = p
                        .label
                        .as_deref()
                        .or(if tab.label.parse::<u32>().is_err() {
                            Some(tab.label.as_str())
                        } else {
                            None
                        })
                        .or(p.terminal_title_stripped.as_deref());
                    let label = if panes.len() == 1 {
                        name.map(str::to_owned).unwrap_or_else(|| {
                            if tabs.len() > 1 {
                                format!("{} · {}", kind.unwrap_or("terminal"), tab.label)
                            } else {
                                ws.label.clone()
                            }
                        })
                    } else {
                        let own = p.label.as_deref().or(kind).or(p.terminal_title_stripped.as_deref());
                        format!("{} · {}", own.unwrap_or("terminal"), ix + 1)
                    };
                    let hay = format!("{label} {} {} {}", p.cwd(), kind.unwrap_or(""), p.pane_id);
                    if !filter.keep(p.agent_status) && kind.is_some() {
                        continue;
                    }
                    if filter != Filter::All && kind.is_none() {
                        continue;
                    }
                    if !(words.is_empty() || ws_match || matches(&words, &hay)) {
                        continue;
                    }
                    let right = match kind {
                        Some(k) => vec![
                            (format!("{k} "), Tone::Dim),
                            (p.agent_status.text().to_owned(), Tone::Status(p.agent_status)),
                        ],
                        None => vec![(shorten_path(p.cwd(), home, 28), Tone::Dim)],
                    };
                    let row = Row {
                        depth: 2,
                        label: label.clone(),
                        right,
                        status: kind.map(|_| p.agent_status),
                        glyph: None,
                        last_child: false,
                        current: m.snapshot.focused_pane_id.as_deref() == Some(&p.pane_id),
                        bold: false,
                        dimmed,
                        target: Target::Pane {
                            machine: m.id.clone(),
                            pane_id: p.pane_id.clone(),
                        },
                    };
                    if kind.is_some() && p.agent_status.needs_you() && !dimmed {
                        needs.push(Row {
                            depth: 1,
                            label: format!("{} / {} / {label}", m.label, ws.label),
                            last_child: false,
                            current: false,
                            target: Target::Attention {
                                machine: m.id.clone(),
                                pane_id: p.pane_id.clone(),
                            },
                            ..row.clone()
                        });
                    }
                    children.push(row);
                }
            }
            if filtering && children.is_empty() && !(words.is_empty() || ws_match) {
                continue;
            }
            if filter != Filter::All && children.is_empty() {
                continue;
            }
            if let Some(last) = children.last_mut() {
                last.last_child = true;
            }
            let pin_key = format!("{}:{}", m.id, ws.workspace_id);
            let is_pinned = pinned.contains(&pin_key);
            let mut right = Vec::new();
            if let Some(g) = m.git.get(&ws.workspace_id) {
                if !g.branch.is_empty() {
                    right.push((g.branch.clone(), Tone::Dim));
                }
            }
            if ws.worktree.as_ref().is_some_and(|w| w.is_linked_worktree) {
                right.push(((if right.is_empty() { "worktree" } else { " · worktree" }).into(), Tone::Dim));
            }
            let mut group = Vec::new();
            group.push(Row {
                depth: 1,
                label: if is_pinned { format!("★ {}", ws.label) } else { ws.label.clone() },
                right,
                status: None,
                glyph: None,
                last_child: false,
                current: false,
                bold: true,
                dimmed,
                target: Target::Workspace {
                    machine: m.id.clone(),
                    workspace_id: ws.workspace_id.clone(),
                },
            });
            group.extend(children);
            groups.push((is_pinned, group));
        }
        groups.sort_by_key(|(pinned, _)| !pinned);
        let machine_rows: Vec<Row> = groups.into_iter().flat_map(|(_, g)| g).collect();
        if filtering && machine_rows.is_empty() && !matches(&words, &m.label) {
            continue;
        }
        let right = if m.local {
            Vec::new()
        } else {
            match m.link {
                Link::Online => vec![(
                    match m.latency_ms {
                        Some(ms) => format!("● online · {ms} ms"),
                        None => "● online".into(),
                    },
                    Tone::Online,
                )],
                Link::Offline => vec![(
                    format!("○ offline{}", m.error.as_deref().map(|e| format!(" · {e}")).unwrap_or_default()),
                    Tone::Offline,
                )],
                Link::Connecting => vec![("○ connecting…".into(), Tone::Dim)],
            }
        };
        rows.push(Row {
            depth: 0,
            label: m.label.clone(),
            right,
            status: None,
            glyph: None,
            last_child: false,
            current: false,
            bold: true,
            dimmed,
            target: Target::Machine {
                machine: m.id.clone(),
            },
        });
        rows.extend(machine_rows);
    }

    let mut out = Vec::new();
    if !needs.is_empty() && !filtering {
        out.push(Row {
            depth: 0,
            label: "Needs you".into(),
            right: vec![(needs.len().to_string(), Tone::Status(Status::Blocked))],
            status: None,
            glyph: None,
            last_child: false,
            current: false,
            bold: true,
            dimmed: false,
            target: Target::Separator(0),
        });
        out.extend(needs);
        out.push(spacer(1));
    }
    out.extend(rows);
    if !filtering {
        out.push(spacer(2));
        for (label, key, action) in [
            ("+ new space", "n", Action::NewWorkspace),
            ("+ connect machine…", "m", Action::ConnectMachine),
        ] {
            out.push(Row {
                depth: 0,
                label: label.into(),
                right: vec![(key.into(), Tone::Dim)],
                status: None,
                glyph: Some("+"),
                last_child: false,
                current: false,
                bold: false,
                dimmed: false,
                target: Target::Action(action),
            });
        }
    }
    Rows { rows: out, counts }
}

fn spacer(n: u8) -> Row {
    Row {
        depth: 0,
        label: String::new(),
        right: Vec::new(),
        status: None,
        glyph: None,
        last_child: false,
        current: false,
        bold: false,
        dimmed: false,
        target: Target::Separator(n),
    }
}

impl Row {
    pub fn selectable(&self) -> bool {
        !matches!(self.target, Target::Separator(_))
    }
}
