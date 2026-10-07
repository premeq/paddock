use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::herdr::{self, Runner, Update};
use crate::state::State;
use crate::model::{build_rows, Action, Counts, Filter, Link, Machine, Row, Status, Target};
use crate::pane_view::PaneView;

pub struct Prompt {
    pub title: String,
    pub input: String,
    pub kind: PromptKind,
}

pub enum PromptKind {
    NewWorkspace { machine: String },
    ConnectMachine,
    NewWorktree { machine: String, workspace_id: String },
    RenameWorkspace { machine: String, workspace_id: String },
    RenamePane { machine: String, pane_id: String },
}

pub struct Confirm {
    pub title: String,
    pub argv: Vec<String>,
    pub machine: String,
}

pub enum Mode {
    Normal,
    Search,
    Prompt(Prompt),
    Confirm(Confirm),
}

/// A pane streamed into paddock.
pub struct Attached {
    pub view: PaneView,
    pub machine: String,
    pub pane_id: String,
}

/// What the event loop must do outside the TUI after handling a key.
pub enum Effect {
    None,
    Quit,
    /// Suspend the TUI, run this argv on the user's terminal, resume.
    Exec(Vec<String>),
}

pub struct App {
    pub machines: Vec<Machine>,
    pub rows: Vec<Row>,
    pub counts: Counts,
    pub selected: usize,
    pub scroll: usize,
    pub query: String,
    pub filter: Filter,
    pub mode: Mode,
    pub home: String,
    pub last_pane: Option<(String, String)>,
    pub pane_text: HashMap<(String, String), String>,
    pub last_error: Option<String>,
    pub refreshed: Option<Instant>,
    pub state: State,
    pub list_area: Rect,
    pub attached: Option<Attached>,
    pub notice: Option<String>,
    pub size: (u16, u16),
    pub demo: bool,
    poll_remote: Duration,
    wake: HashMap<String, Sender<()>>,
    tx: Sender<Update>,
    rx: Receiver<Update>,
    pane_text_requested: Option<(String, String)>,
    user_moved: bool,
}

impl App {
    pub fn new(poll_local: Duration, poll_remote: Duration) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut machines = vec![Machine::local()];
        machines.extend(herdr::saved_machines().unwrap_or_default());
        let mut wake = HashMap::new();
        for m in &machines {
            let every = if m.local { poll_local } else { poll_remote };
            wake.insert(m.id.clone(), crate::events::spawn_watcher(m, every, tx.clone()));
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let mut app = App {
            machines,
            rows: Vec::new(),
            counts: Counts::default(),
            selected: 0,
            scroll: 0,
            query: String::new(),
            filter: Filter::All,
            mode: Mode::Normal,
            home,
            last_pane: None,
            pane_text: HashMap::new(),
            last_error: None,
            refreshed: None,
            state: State::load(),
            list_area: Rect::default(),
            attached: None,
            notice: None,
            size: (80, 24),
            demo: false,
            poll_remote,
            wake,
            tx,
            rx,
            pane_text_requested: None,
            user_moved: false,
        };
        app.rebuild();
        Ok(app)
    }

    /// Fixture fleet for screenshots and UI work without a herdr server.
    pub fn demo() -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = App {
            machines: crate::demo::fleet(),
            rows: Vec::new(),
            counts: Counts::default(),
            selected: 0,
            scroll: 0,
            query: String::new(),
            filter: Filter::All,
            mode: Mode::Normal,
            home: "/home/alex".into(),
            last_pane: None,
            pane_text: crate::demo::screens(),
            last_error: None,
            refreshed: Some(Instant::now()),
            state: State::default(),
            list_area: Rect::default(),
            attached: None,
            notice: None,
            size: (80, 24),
            demo: true,
            poll_remote: Duration::from_secs(3),
            wake: HashMap::new(),
            tx,
            rx,
            pane_text_requested: None,
            user_moved: false,
        };
        app.rebuild();
        Ok(app)
    }

    pub fn machine(&self, id: &str) -> Option<&Machine> {
        self.machines.iter().find(|m| m.id == id)
    }

    /// Header note: empty while everything is fine.
    pub fn health(&self) -> String {
        let offline = self.machines.iter().filter(|m| m.link == Link::Offline).count();
        let local_stale = match self.refreshed {
            Some(t) if t.elapsed().as_secs() >= 25 => Some(format!("local stale · {}s", t.elapsed().as_secs())),
            None => Some("connecting…".into()),
            _ => None,
        };
        match (local_stale, offline) {
            (Some(s), 0) => s,
            (Some(s), n) => format!("{s} · {n} offline"),
            (None, 0) => String::new(),
            (None, n) => format!("{n} offline"),
        }
    }

    /// Drains poller updates. Returns true when something changed.
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        if let Some(a) = &mut self.attached {
            changed |= a.view.pump();
            if let Some(reason) = a.view.closed() {
                let reason = reason.to_owned();
                self.detach(Some(format!("pane stream closed: {reason}")));
                changed = true;
            }
        }
        while let Ok(u) = self.rx.try_recv() {
            changed = true;
            match u {
                Update::Snapshot {
                    machine,
                    snapshot,
                    latency_ms,
                } => {
                    if let Some(m) = self.machines.iter_mut().find(|m| m.id == machine) {
                        m.snapshot = snapshot;
                        m.link = Link::Online;
                        m.latency_ms = Some(latency_ms);
                        m.error = None;
                        if m.local {
                            self.refreshed = Some(Instant::now());
                            self.last_error = None;
                        }
                    }
                }
                Update::Git { machine, info } => {
                    if let Some(m) = self.machines.iter_mut().find(|m| m.id == machine) {
                        m.git = info;
                    }
                }
                Update::Failed { machine, error } => {
                    if let Some(m) = self.machines.iter_mut().find(|m| m.id == machine) {
                        m.link = Link::Offline;
                        m.error = Some(error.clone());
                        if m.local {
                            self.last_error = Some(error);
                        }
                    }
                }
                Update::PaneText {
                    machine,
                    pane_id,
                    text,
                } => {
                    self.pane_text.insert((machine, pane_id), text);
                }
            }
        }
        if changed {
            self.rebuild();
        }
        changed
    }

    pub fn rebuild(&mut self) {
        let keep = self
            .rows
            .get(self.selected)
            .filter(|_| self.user_moved)
            .map(|r| r.target.clone());
        let built = build_rows(&self.machines, &self.query, self.filter, &self.home, &self.state.pinned);
        self.rows = built.rows;
        self.counts = built.counts;
        self.selected = keep
            .and_then(|t| self.rows.iter().position(|r| r.target == t))
            .unwrap_or_else(|| {
                self.rows
                    .iter()
                    .position(|r| matches!(r.target, Target::Pane { .. }))
                    .unwrap_or(0)
            });
        self.clamp();
        self.request_pane_text();
    }

    fn clamp(&mut self) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = self.selected.min(self.rows.len() - 1);
        if !self.rows[self.selected].selectable() {
            self.step(1);
        }
    }

    fn step(&mut self, dir: i32) {
        if self.rows.is_empty() {
            return;
        }
        let n = self.rows.len() as i32;
        let mut i = self.selected as i32;
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if self.rows[i as usize].selectable() {
                self.selected = i as usize;
                break;
            }
        }
        self.request_pane_text();
    }

    fn jump_workspace(&mut self, dir: i32) {
        let n = self.rows.len() as i32;
        let mut i = self.selected as i32;
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if matches!(self.rows[i as usize].target, Target::Workspace { .. }) {
                self.selected = i as usize;
                break;
            }
        }
        self.request_pane_text();
    }

    fn request_pane_text(&mut self) {
        let Some(Target::Pane { machine, pane_id }) = self.rows.get(self.selected).map(|r| r.target.resolved()) else {
            return;
        };
        let key = (machine.clone(), pane_id.clone());
        if self.pane_text_requested.as_ref() == Some(&key) && self.pane_text.contains_key(&key) {
            return;
        }
        if self.demo {
            return;
        }
        if let Some(m) = self.machine(&machine) {
            herdr::fetch_pane_text(m, pane_id.clone(), self.tx.clone());
            self.pane_text_requested = Some(key);
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Effect {
        self.user_moved = true;
        if let Some(a) = &mut self.attached {
            let home = self.state.home_key();
            if k.code == home.code && k.modifiers == home.modifiers {
                self.detach(None);
            } else {
                a.view.key(k);
            }
            return Effect::None;
        }
        self.notice = None;
        match &mut self.mode {
            Mode::Normal => self.key_normal(k),
            Mode::Search => {
                match k.code {
                    KeyCode::Esc => {
                        self.query.clear();
                        self.mode = Mode::Normal;
                    }
                    KeyCode::Enter => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        self.query.pop();
                    }
                    KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => self.query.push(c),
                    KeyCode::Down => self.step(1),
                    KeyCode::Up => self.step(-1),
                    _ => {}
                }
                self.rebuild();
                Effect::None
            }
            Mode::Prompt(p) => {
                match k.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        p.input.pop();
                    }
                    KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => p.input.push(c),
                    KeyCode::Enter => {
                        let Mode::Prompt(p) = std::mem::replace(&mut self.mode, Mode::Normal) else {
                            unreachable!()
                        };
                        return self.submit(p);
                    }
                    _ => {}
                }
                Effect::None
            }
            Mode::Confirm(c) => {
                if k.code == KeyCode::Char('y') {
                    let argv = c.argv.clone();
                    let machine = c.machine.clone();
                    self.mode = Mode::Normal;
                    self.run(&machine, &argv);
                } else {
                    self.mode = Mode::Normal;
                }
                Effect::None
            }
        }
    }

    fn key_normal(&mut self, k: KeyEvent) -> Effect {
        let target = self.rows.get(self.selected).map(|r| r.target.resolved());
        match k.code {
            KeyCode::Char('q') => return Effect::Quit,
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Effect::Quit,
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Right | KeyCode::Char('l') => self.jump_workspace(1),
            KeyCode::Left | KeyCode::Char('h') => self.jump_workspace(-1),
            KeyCode::Char('g') | KeyCode::Home => {
                self.selected = 0;
                self.clamp();
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.rows.len().saturating_sub(1);
                self.step(-1);
                self.step(1);
            }
            KeyCode::PageDown => {
                for _ in 0..10 {
                    self.step(1);
                }
            }
            KeyCode::PageUp => {
                for _ in 0..10 {
                    self.step(-1);
                }
            }
            KeyCode::Char('/') => self.mode = Mode::Search,
            KeyCode::Char('a') => self.set_filter(Filter::All),
            KeyCode::Char('b') => self.set_filter(Filter::Blocked),
            KeyCode::Char('w') => self.set_filter(Filter::Working),
            KeyCode::Char('i') => self.set_filter(Filter::Idle),
            KeyCode::Char('d') => self.set_filter(Filter::Done),
            KeyCode::Esc => {
                if !self.query.is_empty() || self.filter != Filter::All {
                    self.query.clear();
                    self.filter = Filter::All;
                    self.rebuild();
                } else if let Some((m, p)) = self.last_pane.clone() {
                    return self.attach(&m, &p);
                }
            }
            KeyCode::Enter => return self.open(target),
            KeyCode::Char('n') => self.prompt_new_workspace(),
            KeyCode::Char('t') => {
                let (machine, workspace_id) = match &target {
                    Some(Target::Workspace { machine, workspace_id }) => (machine.clone(), workspace_id.clone()),
                    Some(Target::Pane { machine, pane_id }) if self.workspace_of(machine, pane_id).is_some() => (machine.clone(), self.workspace_of(machine, pane_id).unwrap()),
                    _ => {
                        self.notice = Some("select a space first".into());
                        return Effect::None;
                    }
                };
                self.mode = Mode::Prompt(Prompt {
                    title: "new worktree · branch name".into(),
                    input: String::new(),
                    kind: PromptKind::NewWorktree { machine, workspace_id },
                });
            }
            KeyCode::Char('m') => self.prompt_connect_machine(),
            KeyCode::Char('c') => {
                if let Some((machine, workspace_id)) = self.workspace_target(&target) {
                    self.run(&machine, &["tab", "create", "--workspace", &workspace_id, "--no-focus"].map(String::from));
                }
            }
            KeyCode::Char('r') => match &target {
                Some(Target::Workspace { machine, workspace_id }) => {
                    self.mode = Mode::Prompt(Prompt {
                        title: "rename space".into(),
                        input: String::new(),
                        kind: PromptKind::RenameWorkspace {
                            machine: machine.clone(),
                            workspace_id: workspace_id.clone(),
                        },
                    })
                }
                Some(Target::Pane { machine, pane_id }) => {
                    self.mode = Mode::Prompt(Prompt {
                        title: "rename pane".into(),
                        input: String::new(),
                        kind: PromptKind::RenamePane {
                            machine: machine.clone(),
                            pane_id: pane_id.clone(),
                        },
                    })
                }
                _ => {}
            },
            KeyCode::Char('x') => match &target {
                Some(Target::Workspace { machine, workspace_id }) => {
                    self.mode = Mode::Confirm(Confirm {
                        title: format!("close space {workspace_id}?"),
                        argv: ["workspace", "close", workspace_id].map(String::from).to_vec(),
                        machine: machine.clone(),
                    })
                }
                Some(Target::Pane { machine, pane_id }) => {
                    self.mode = Mode::Confirm(Confirm {
                        title: format!("close pane {pane_id}?"),
                        argv: ["pane", "close", pane_id].map(String::from).to_vec(),
                        machine: machine.clone(),
                    })
                }
                _ => {}
            },
            KeyCode::Char('p') => {
                if let Some((machine, workspace_id)) = self.workspace_target(&target) {
                    self.state.toggle_pin(&format!("{machine}:{workspace_id}"));
                    self.rebuild();
                }
            }
            KeyCode::Char('D') => {
                if let Some(Target::Workspace { machine, workspace_id }) = &target {
                    self.mode = Mode::Confirm(Confirm {
                        title: format!("delete worktree checkout of {workspace_id}? (git worktree remove)"),
                        argv: ["worktree", "remove", "--workspace", workspace_id].map(String::from).to_vec(),
                        machine: machine.clone(),
                    })
                }
            }
            _ => {}
        }
        Effect::None
    }

    pub fn paste(&mut self, text: &str) {
        match &mut self.attached {
            Some(a) => a.view.paste(text),
            None => match &mut self.mode {
                Mode::Search => {
                    self.query.push_str(text);
                    self.rebuild();
                }
                Mode::Prompt(p) => p.input.push_str(text),
                _ => {}
            },
        }
    }

    pub fn detach(&mut self, notice: Option<String>) {
        if let Some(a) = self.attached.take() {
            a.view.release();
        }
        self.notice = notice;
        self.pane_text.clear();
        self.rebuild();
    }

    /// Breadcrumb for the attached pane: machine, space, tab, pane label.
    pub fn attached_crumbs(&self) -> Vec<String> {
        let Some(a) = self.attached.as_ref() else {
            return Vec::new();
        };
        let Some(m) = self.machine(&a.machine) else {
            return Vec::new();
        };
        let mut crumbs = vec![m.label.clone()];
        if let Some(p) = m.snapshot.panes.iter().find(|p| p.pane_id == a.pane_id) {
            if let Some(t) = m.snapshot.tabs.iter().find(|t| t.tab_id == p.tab_id) {
                crumbs.push(t.label.clone());
            }
            if let Some(name) = &p.label {
                crumbs.push(name.clone());
            }
        }
        crumbs
    }

    pub fn attached_agent(&self) -> Option<(String, Status)> {
        let a = self.attached.as_ref()?;
        let p = self.machine(&a.machine)?.snapshot.panes.iter().find(|p| p.pane_id == a.pane_id)?;
        p.agent.as_ref().map(|k| (k.clone(), p.agent_status))
    }

    pub fn mouse(&mut self, m: MouseEvent) -> Effect {
        if let Some(a) = &mut self.attached {
            a.view.mouse(m, (0, 2));
            return Effect::None;
        }
        if !matches!(self.mode, Mode::Normal) {
            return Effect::None;
        }
        match m.kind {
            MouseEventKind::ScrollDown => self.step(1),
            MouseEventKind::ScrollUp => self.step(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                let a = self.list_area;
                if m.column >= a.x && m.column < a.right() && m.row >= a.y && m.row < a.bottom() {
                    let ix = self.scroll + (m.row - a.y) as usize;
                    if self.rows.get(ix).is_some_and(|r| r.selectable()) {
                        if ix == self.selected {
                            let target = self.rows[ix].target.resolved();
                            return self.open(Some(target));
                        }
                        self.user_moved = true;
                        self.selected = ix;
                        self.request_pane_text();
                    }
                }
            }
            _ => {}
        }
        Effect::None
    }

    fn set_filter(&mut self, f: Filter) {
        self.filter = if self.filter == f { Filter::All } else { f };
        self.rebuild();
    }

    fn workspace_of(&self, machine: &str, pane_id: &str) -> Option<String> {
        self.machine(machine)?
            .snapshot
            .panes
            .iter()
            .find(|p| p.pane_id == pane_id)
            .map(|p| p.workspace_id.clone())
    }

    fn workspace_target(&self, target: &Option<Target>) -> Option<(String, String)> {
        match target {
            Some(Target::Workspace { machine, workspace_id }) => Some((machine.clone(), workspace_id.clone())),
            Some(Target::Pane { machine, pane_id }) => {
                self.workspace_of(machine, pane_id).map(|w| (machine.clone(), w))
            }
            _ => None,
        }
    }

    fn prompt_connect_machine(&mut self) {
        self.mode = Mode::Prompt(Prompt {
            title: "connect machine · ssh target (host, user@host, ssh alias)".into(),
            input: String::new(),
            kind: PromptKind::ConnectMachine,
        });
    }

    /// Picks up machines added or removed outside paddock; starts watchers for new ones.
    pub fn reload_machines(&mut self) {
        let saved = herdr::saved_machines().unwrap_or_default();
        self.machines.retain(|m| m.local || saved.iter().any(|s| s.id == m.id));
        for m in saved {
            if self.machines.iter().any(|k| k.id == m.id) {
                continue;
            }
            self.wake.insert(m.id.clone(), crate::events::spawn_watcher(&m, self.poll_remote, self.tx.clone()));
            self.machines.push(m);
        }
        self.rebuild();
    }

    fn prompt_new_workspace(&mut self) {
        let machine = match self.rows.get(self.selected).map(|r| &r.target) {
            Some(Target::Machine { machine }) | Some(Target::Workspace { machine, .. }) | Some(Target::Pane { machine, .. }) => machine.clone(),
            _ => "local".into(),
        };
        let label = self.machine(&machine).map(|m| m.label.clone()).unwrap_or_default();
        self.mode = Mode::Prompt(Prompt {
            title: format!("new space on {label} · directory"),
            input: "~/".into(),
            kind: PromptKind::NewWorkspace { machine },
        });
    }

    fn open(&mut self, target: Option<Target>) -> Effect {
        match target {
            Some(Target::Pane { machine, pane_id }) => self.attach(&machine, &pane_id),
            Some(Target::Workspace { machine, workspace_id }) => {
                let pane = self.machine(&machine).and_then(|m| {
                    let ws = m.snapshot.workspaces.iter().find(|w| w.workspace_id == workspace_id)?;
                    m.snapshot
                        .panes
                        .iter()
                        .find(|p| p.tab_id == ws.active_tab_id && p.focused)
                        .or_else(|| m.snapshot.panes.iter().find(|p| p.tab_id == ws.active_tab_id))
                        .map(|p| p.pane_id.clone())
                });
                match pane {
                    Some(p) => self.attach(&machine, &p),
                    None => Effect::None,
                }
            }
            Some(Target::Action(Action::NewWorkspace)) => {
                self.prompt_new_workspace();
                Effect::None
            }
            Some(Target::Action(Action::ConnectMachine)) => {
                self.prompt_connect_machine();
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn attach(&mut self, machine: &str, pane_id: &str) -> Effect {
        if self.demo {
            self.notice = Some("demo mode: attach is disabled".into());
            return Effect::None;
        }
        let Some(m) = self.machine(machine) else {
            return Effect::None;
        };
        let Some(p) = m.snapshot.panes.iter().find(|p| p.pane_id == pane_id) else {
            return Effect::None;
        };
        let runner = Runner::for_machine(m);
        let (cols, rows) = (self.size.0, self.size.1.saturating_sub(1).max(1));
        match PaneView::open(&runner, &p.pane_id, cols, rows) {
            Ok(view) => {
                if p.agent.is_some() {
                    let _ = runner.json(&["agent", "focus", pane_id]);
                    if let Some(w) = self.wake.get(machine) {
                        let _ = w.send(());
                    }
                }
                self.last_pane = Some((machine.to_owned(), pane_id.to_owned()));
                self.attached = Some(Attached {
                    view,
                    machine: machine.to_owned(),
                    pane_id: pane_id.to_owned(),
                });
            }
            Err(e) => self.notice = Some(format!("attach failed: {e}")),
        }
        Effect::None
    }

    fn submit(&mut self, p: Prompt) -> Effect {
        let input = p.input.trim().to_owned();
        if input.is_empty() {
            return Effect::None;
        }
        match p.kind {
            PromptKind::ConnectMachine => {
                return Effect::Exec(vec!["herdr".into(), "machine".into(), "add".into(), input]);
            }
            PromptKind::NewWorkspace { machine } => {
                self.run(&machine, &["workspace", "create", "--cwd", &input, "--no-focus"].map(String::from));
            }
            PromptKind::NewWorktree { machine, workspace_id } => {
                self.run(&machine, &["worktree", "create", "--workspace", &workspace_id, "--branch", &input, "--no-focus"].map(String::from));
            }
            PromptKind::RenameWorkspace { machine, workspace_id } => {
                self.run(&machine, &["workspace", "rename", &workspace_id, &input].map(String::from));
            }
            PromptKind::RenamePane { machine, pane_id } => {
                self.run(&machine, &["pane", "rename", &pane_id, &input].map(String::from));
            }
        }
        Effect::None
    }

    fn run(&mut self, machine: &str, argv: &[String]) {
        let Some(m) = self.machine(machine) else {
            return;
        };
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        if self.demo {
            self.notice = Some("demo mode: actions are disabled".into());
            return;
        }
        match Runner::for_machine(m).json(&args) {
            Ok(_) => {
                self.last_error = None;
                if let Some(w) = self.wake.get(machine) {
                    let _ = w.send(());
                }
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }
}
