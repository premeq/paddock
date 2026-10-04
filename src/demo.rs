use std::collections::HashMap;

use crate::model::{GitInfo, Link, Machine, Pane, Snapshot, Status, Tab, Workspace, Worktree};

fn pane(ws: &str, tab: &str, id: &str, cwd: &str, agent: Option<&str>, status: Status, title: Option<&str>, focused: bool) -> Pane {
    Pane {
        pane_id: format!("{ws}:{id}"),
        tab_id: format!("{ws}:{tab}"),
        workspace_id: ws.into(),
        agent_status: status,
        agent: agent.map(str::to_owned),
        label: None,
        cwd: Some(cwd.into()),
        foreground_cwd: Some(cwd.into()),
        terminal_title_stripped: title.map(str::to_owned),
        focused,
    }
}

fn ws(id: &str, label: &str, tab: &str, path: &str, linked: bool) -> Workspace {
    Workspace {
        workspace_id: id.into(),
        label: label.into(),
        active_tab_id: format!("{id}:{tab}"),
        worktree: Some(Worktree {
            checkout_path: path.into(),
            is_linked_worktree: linked,
            repo_name: path.rsplit('/').next().unwrap_or("").into(),
        }),
    }
}

fn tab(ws: &str, id: &str, label: &str, panes: u32) -> Tab {
    Tab {
        tab_id: format!("{ws}:{id}"),
        workspace_id: ws.into(),
        label: label.into(),
        pane_count: panes,
    }
}

fn git(branch: &str, ahead: u32, changed: u32) -> GitInfo {
    GitInfo { branch: branch.into(), ahead, behind: 0, changed }
}

pub fn fleet() -> Vec<Machine> {
    let mut local = Machine::local();
    local.link = Link::Online;
    local.snapshot = Snapshot {
        workspaces: vec![
            ws("w1", "[1] shopfront", "t1", "/home/alex/code/shopfront", false),
            ws("w2", "[2] checkout-retry", "t1", "/home/alex/.herdr/worktrees/shopfront/checkout-retry", true),
        ],
        tabs: vec![tab("w1", "t1", "[1] claude", 1), tab("w1", "t2", "[2] zsh", 1), tab("w2", "t1", "[1] codex", 1)],
        panes: vec![
            pane("w1", "t1", "p1", "/home/alex/code/shopfront", Some("claude"), Status::Blocked, Some("Add retry to payment webhook"), true),
            pane("w1", "t2", "p2", "/home/alex/code/shopfront", None, Status::Unknown, Some("zsh"), false),
            pane("w2", "t1", "p3", "/home/alex/.herdr/worktrees/shopfront/checkout-retry", Some("codex"), Status::Working, Some("Flaky checkout test"), false),
        ],
        focused_pane_id: Some("w1:p1".into()),
    };
    local.git.insert("w1".into(), git("main", 0, 2));
    local.git.insert("w2".into(), git("checkout-retry", 3, 7));

    let mut build = Machine::remote("m1".into(), "build-box".into(), "build-box".into(), "agents".into());
    build.link = Link::Online;
    build.latency_ms = Some(18);
    build.snapshot = Snapshot {
        workspaces: vec![ws("w1", "[1] infra", "t1", "/home/alex/infra", false), ws("w2", "[2] docs", "t1", "/home/alex/docs", false)],
        tabs: vec![tab("w1", "t1", "[1] claude", 1), tab("w2", "t1", "[1] zsh", 1)],
        panes: vec![
            pane("w1", "t1", "p1", "/home/alex/infra", Some("claude"), Status::Done, Some("Rotate TLS certificates"), true),
            pane("w2", "t1", "p2", "/home/alex/docs", None, Status::Unknown, Some("zsh"), false),
        ],
        focused_pane_id: Some("w1:p1".into()),
    };
    build.git.insert("w1".into(), git("main", 0, 0));
    build.git.insert("w2".into(), git("release-notes", 1, 1));

    let mut lab = Machine::remote("m2".into(), "gpu-lab".into(), "gpu-lab".into(), "default".into());
    lab.link = Link::Online;
    lab.latency_ms = Some(41);
    lab.snapshot = Snapshot {
        workspaces: vec![ws("w1", "[1] ranker", "t1", "/home/alex/ranker", false)],
        tabs: vec![tab("w1", "t1", "[1] claude", 2)],
        panes: vec![
            pane("w1", "t1", "p1", "/home/alex/ranker", Some("claude"), Status::Idle, Some("Evaluate ranker v3"), true),
            pane("w1", "t1", "p2", "/home/alex/ranker", None, Status::Unknown, Some("zsh"), false),
        ],
        focused_pane_id: Some("w1:p1".into()),
    };
    lab.git.insert("w1".into(), git("exp/ranker-v3", 12, 0));

    let mut laptop = Machine::remote("m3".into(), "old-laptop".into(), "old-laptop".into(), "default".into());
    laptop.link = Link::Offline;
    laptop.error = Some("ssh timeout".into());

    vec![local, build, lab, laptop]
}

pub fn screens() -> HashMap<(String, String), String> {
    let lines = [
        "● I added exponential backoff to the webhook handler and a test for the",
        "  three-retry case. The payment provider mock needs a new fixture.",
        "",
        "  Edit src/payments/webhook.rs",
        "  Edit tests/webhook_retry.rs",
        "",
        "  Do you want to proceed?",
        "  ❯ 1. Yes",
        "    2. Yes, and don't ask again this session",
        "    3. No",
    ];
    HashMap::from([(("local".to_owned(), "w1:p1".to_owned()), lines.join("\n"))])
}
