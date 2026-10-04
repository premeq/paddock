use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Mode};
use crate::model::{shorten_path, Link, Status, Target, Tone};
use crate::theme::MOCHA as T;

pub fn status_color(s: Status) -> ratatui::style::Color {
    match s {
        Status::Blocked => T.red,
        Status::Working => T.yellow,
        Status::Done => T.teal,
        Status::Idle => T.green,
        Status::Unknown => T.dim,
    }
}

fn tone(t: Tone) -> ratatui::style::Color {
    match t {
        Tone::Dim => T.dim,
        Tone::Status(s) => status_color(s),
        Tone::Online => T.green,
        Tone::Offline => T.dim,
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.buffer_mut().set_style(area, Style::default().bg(T.bg).fg(T.text));
    if area.height < 8 || area.width < 40 {
        Paragraph::new("terminal too small").render(area, f.buffer_mut());
        return;
    }
    let header = Rect::new(area.x, area.y, area.width, 1);
    let footer = Rect::new(area.x, area.bottom() - 2, area.width, 2);
    let body = Rect::new(area.x, area.y + 2, area.width, area.height - 5);
    draw_header(f.buffer_mut(), header, app);
    hline(f.buffer_mut(), area.y + 1, area);
    hline(f.buffer_mut(), footer.y - 1, area);

    let detail_w = if body.width >= 110 { 52 } else if body.width >= 84 { body.width / 3 } else { 0 };
    let tree = Rect::new(body.x + 1, body.y, body.width.saturating_sub(detail_w + 3), body.height);
    draw_tree(f.buffer_mut(), tree, app);
    if detail_w > 0 {
        let detail = Rect::new(tree.right() + 2, body.y, detail_w, body.height);
        draw_detail(f.buffer_mut(), detail, app);
    }
    draw_footer(f.buffer_mut(), footer, app);
    if let Mode::Prompt(p) = &app.mode {
        draw_prompt(f, area, &p.title, &p.input);
    }
    if let Mode::Confirm(c) = &app.mode {
        draw_prompt(f, area, &c.title, "y to confirm, any other key to cancel");
    }
}

fn hline(buf: &mut Buffer, y: u16, area: Rect) {
    buf.set_string(area.x, y, "─".repeat(area.width as usize), Style::default().fg(T.line));
}

fn put_right(buf: &mut Buffer, area: Rect, y: u16, spans: Vec<Span>) {
    let w: usize = spans.iter().map(|s| s.content.width()).sum();
    let x = area.right().saturating_sub(w as u16);
    buf.set_line(x, y, &Line::from(spans), w as u16);
}

fn draw_header(buf: &mut Buffer, area: Rect, app: &App) {
    let c = app.counts;
    let mut spans = vec![
        Span::styled(" paddock", Style::default().fg(T.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" · home   ", Style::default().fg(T.sub)),
        Span::styled(
            format!(
                "{} {} · {} {} · {} {}",
                c.machines,
                plural(c.machines, "machine"),
                c.workspaces,
                plural(c.workspaces, "workspace"),
                c.agents,
                plural(c.agents, "agent")
            ),
            Style::default().fg(T.dim),
        ),
    ];
    for (n, s) in [
        (c.blocked, Status::Blocked),
        (c.working, Status::Working),
        (c.done, Status::Done),
        (c.idle, Status::Idle),
    ] {
        if n > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(T.dim)));
            let mut st = Style::default().fg(status_color(s));
            if s == Status::Blocked {
                st = st.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(format!("{n} {}", s.text()), st));
        }
    }
    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
    let right = match &app.last_error {
        Some(e) => Span::styled(format!("{e} "), Style::default().fg(T.red)),
        None => Span::styled(
            format!("{} ", app.refreshed_ago()),
            Style::default().fg(T.dim),
        ),
    };
    put_right(buf, area, area.y, vec![right]);
}

fn plural(n: usize, w: &str) -> String {
    if n == 1 {
        w.to_owned()
    } else {
        format!("{w}s")
    }
}

fn draw_tree(buf: &mut Buffer, area: Rect, app: &mut App) {
    let search = match &app.mode {
        Mode::Search => format!("/ {}▏", app.query),
        _ if !app.query.is_empty() => format!("/ {}", app.query),
        _ => "/ search agents and terminals".to_owned(),
    };
    let st = if matches!(app.mode, Mode::Search) { T.text } else { T.dim };
    buf.set_string(area.x, area.y, &search, Style::default().fg(st));
    put_right(
        buf,
        area,
        area.y,
        vec![Span::styled(format!("filter: {}", app.filter.text()), Style::default().fg(T.dim))],
    );
    buf.set_string(area.x, area.y + 1, "─".repeat(area.width as usize), Style::default().fg(T.line));

    let list = Rect::new(area.x, area.y + 2, area.width, area.height.saturating_sub(2));
    let visible = list.height as usize;
    if app.selected >= app.scroll + visible {
        app.scroll = app.selected + 1 - visible;
    }
    if app.selected < app.scroll {
        app.scroll = app.selected;
    }
    if app.rows.is_empty() {
        buf.set_string(list.x, list.y, "No matching agents or terminals", Style::default().fg(T.dim));
        return;
    }
    for (i, row) in app.rows.iter().enumerate().skip(app.scroll).take(visible) {
        let y = list.y + (i - app.scroll) as u16;
        let selected = i == app.selected;
        let rect = Rect::new(list.x, y, list.width, 1);
        let base = if selected {
            Style::default().fg(T.sel_fg).bg(T.accent)
        } else if row.dimmed {
            Style::default().fg(T.dim).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(T.text)
        };
        if selected {
            buf.set_style(rect, base);
        }
        let mut spans = Vec::new();
        let indent = "  ".repeat(row.depth as usize);
        spans.push(Span::raw(indent));
        if row.depth == 2 {
            let branch = if row.last_child { "└─ " } else { "├─ " };
            spans.push(Span::styled(branch, if selected { base } else { base.fg(T.dim) }));
        }
        if row.current {
            spans.push(Span::styled("◆ ", base));
        }
        if let Some(s) = row.status {
            let dot = if selected { base } else { base.fg(status_color(s)) };
            spans.push(Span::styled(format!("{} ", s.dot()), dot));
        }
        let mut label = base;
        if row.bold && !selected {
            label = label.add_modifier(Modifier::BOLD);
        }
        if row.label == "Needs you" {
            label = label.fg(T.red).add_modifier(Modifier::BOLD);
        }
        if row.glyph.is_some() && !selected {
            label = label.fg(T.accent);
        }
        if matches!(row.target, Target::Machine { .. }) && !selected && !row.dimmed {
            label = label.fg(T.sub);
        }
        spans.push(Span::styled(row.label.clone(), label));
        let left_w: usize = spans.iter().map(|s| s.content.width()).sum();
        let right_w: usize = row.right.iter().map(|(t, _)| t.width()).sum();
        let avail = (list.width as usize).saturating_sub(right_w + 1);
        buf.set_line(rect.x, y, &Line::from(spans), avail.min(left_w) as u16);
        let right: Vec<Span> = row
            .right
            .iter()
            .map(|(t, tn)| {
                let st = if selected { base } else { base.fg(tone(*tn)) };
                let st = if matches!(tn, Tone::Status(Status::Blocked)) {
                    st.add_modifier(Modifier::BOLD)
                } else {
                    st
                };
                Span::styled(t.clone(), st)
            })
            .collect();
        put_right(buf, rect, y, right);
    }
}

fn draw_detail(buf: &mut Buffer, area: Rect, app: &App) {
    let Some(row) = app.rows.get(app.selected) else {
        return;
    };
    let title = match &row.target {
        Target::Action(_) => "paddock".to_owned(),
        _ => row.label.clone(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(T.line))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(T.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buf);
    let mut lines: Vec<Line> = Vec::new();
    let key = |k: &str| Span::styled(format!("{k:<9}"), Style::default().fg(T.dim));
    let val = |v: String| Span::styled(v, Style::default().fg(T.text));
    let head = |h: &str| {
        Line::from(Span::styled(
            h.to_owned(),
            Style::default().fg(T.sub).add_modifier(Modifier::BOLD),
        ))
    };
    let home = app.home.as_str();
    match &row.target {
        Target::Machine { machine } => {
            if let Some(m) = app.machine(machine) {
                lines.push(Line::from(vec![key("link"), val(link_text(m.link))]));
                if let Some(t) = &m.ssh_target {
                    lines.push(Line::from(vec![key("ssh"), val(t.clone())]));
                }
                if let Some(s) = &m.session {
                    lines.push(Line::from(vec![key("session"), val(s.clone())]));
                }
                if let Some(ms) = m.latency_ms {
                    lines.push(Line::from(vec![key("latency"), val(format!("{ms} ms"))]));
                }
                if let Some(e) = &m.error {
                    lines.push(Line::from(vec![key("error"), Span::styled(e.clone(), Style::default().fg(T.red))]));
                }
                lines.push(Line::default());
                lines.push(Line::from(vec![
                    key("spaces"),
                    val(m.snapshot.workspaces.len().to_string()),
                ]));
                lines.push(Line::from(vec![key("panes"), val(m.snapshot.panes.len().to_string())]));
            }
        }
        Target::Workspace { machine, workspace_id } => {
            if let Some(m) = app.machine(machine) {
                if let Some(ws) = m.snapshot.workspaces.iter().find(|w| &w.workspace_id == workspace_id) {
                    lines.push(Line::from(vec![key("machine"), val(m.label.clone())]));
                    if let Some(t) = &ws.worktree {
                        lines.push(Line::from(vec![key("path"), val(shorten_path(&t.checkout_path, home, 40))]));
                        if t.is_linked_worktree {
                            lines.push(Line::from(vec![key("repo"), val(t.repo_name.clone())]));
                        }
                    }
                    if let Some(g) = m.git.get(workspace_id) {
                        let mut b = g.branch.clone();
                        if g.ahead > 0 || g.behind > 0 {
                            b.push_str(&format!(" · ↑{} ↓{}", g.ahead, g.behind));
                        }
                        if g.changed > 0 {
                            b.push_str(&format!(" · {} changed", g.changed));
                        }
                        lines.push(Line::from(vec![key("branch"), val(b)]));
                    }
                    lines.push(Line::default());
                    lines.push(head("agents"));
                    let mut any = false;
                    for p in m.snapshot.panes.iter().filter(|p| &p.workspace_id == workspace_id) {
                        if let Some(a) = &p.agent {
                            any = true;
                            lines.push(Line::from(vec![
                                Span::styled(
                                    format!("{} ", p.agent_status.dot()),
                                    Style::default().fg(status_color(p.agent_status)),
                                ),
                                val(format!("{a} · {}", p.agent_status.text())),
                                Span::styled(format!("  {}", p.pane_id), Style::default().fg(T.dim)),
                            ]));
                        }
                    }
                    if !any {
                        lines.push(Line::from(Span::styled("none", Style::default().fg(T.dim))));
                    }
                    lines.push(Line::default());
                    lines.push(head("tabs"));
                    for t in m.snapshot.tabs.iter().filter(|t| &t.workspace_id == workspace_id) {
                        lines.push(Line::from(vec![
                            val(t.label.clone()),
                            Span::styled(
                                format!("  {} {}", t.pane_count, plural(t.pane_count as usize, "pane")),
                                Style::default().fg(T.dim),
                            ),
                        ]));
                    }
                    lines.push(Line::default());
                    lines.push(head("actions"));
                    for (k, v) in [
                        ("enter", "open"),
                        ("c", "new tab here"),
                        ("t", "new worktree from here"),
                        ("r", "rename"),
                        ("x", "close workspace"),
                        ("D", "delete worktree checkout"),
                    ] {
                        lines.push(action_line(k, v));
                    }
                }
            }
        }
        Target::Pane { machine, pane_id } => {
            if let Some(m) = app.machine(machine) {
                if let Some(p) = m.snapshot.panes.iter().find(|p| &p.pane_id == pane_id) {
                    lines.push(Line::from(vec![key("machine"), val(m.label.clone())]));
                    lines.push(Line::from(vec![key("pane"), val(p.pane_id.clone())]));
                    lines.push(Line::from(vec![key("cwd"), val(shorten_path(p.cwd(), home, 40))]));
                    if let Some(a) = &p.agent {
                        lines.push(Line::from(vec![
                            key("agent"),
                            val(a.clone()),
                            Span::raw(" · "),
                            Span::styled(
                                p.agent_status.text(),
                                Style::default().fg(status_color(p.agent_status)),
                            ),
                        ]));
                    }
                    if let Some(t) = &p.terminal_title_stripped {
                        lines.push(Line::from(vec![key("title"), val(t.clone())]));
                    }
                    lines.push(Line::default());
                    lines.push(head("screen"));
                    match app.pane_text.get(&(machine.clone(), pane_id.clone())) {
                        Some(text) => {
                            for l in text.lines().filter(|l| !l.trim().is_empty()).rev().take(6).collect::<Vec<_>>().into_iter().rev() {
                                let l: String = l.chars().take(inner.width as usize).collect();
                                lines.push(Line::from(Span::styled(l, Style::default().fg(T.dim))));
                            }
                        }
                        None => lines.push(Line::from(Span::styled("…", Style::default().fg(T.dim)))),
                    }
                    lines.push(Line::default());
                    lines.push(head("actions"));
                    for (k, v) in [("enter", "attach"), ("r", "rename pane"), ("x", "close pane")] {
                        lines.push(action_line(k, v));
                    }
                }
            }
        }
        Target::Action(_) => {
            lines.push(Line::from(val("Home screen for your herdr fleet.".into())));
            lines.push(Line::default());
            lines.push(head("keys"));
            for (k, v) in [
                ("↑↓ j k", "move"),
                ("← →", "previous / next space"),
                ("enter", "open"),
                ("esc", "back to last pane"),
                ("/", "search"),
                ("a b w i d", "filter: all blocked working idle done"),
                ("n t m", "new space, new worktree, connect machine"),
                ("c r x D", "new tab, rename, close, delete worktree"),
                ("q", "quit"),
            ] {
                lines.push(action_line(k, v));
            }
        }
    }
    Paragraph::new(lines).render(inner, buf);
}

fn action_line<'a>(k: &str, v: &str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{k:<9}"),
            Style::default().fg(T.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(v.to_owned(), Style::default().fg(T.text)),
    ])
}

fn link_text(l: Link) -> String {
    match l {
        Link::Online => "online".into(),
        Link::Offline => "offline".into(),
        Link::Connecting => "connecting…".into(),
    }
}

fn draw_footer(buf: &mut Buffer, area: Rect, app: &App) {
    let crumb = app
        .rows
        .get(app.selected)
        .map(|r| match &r.target {
            Target::Pane { machine, pane_id } => {
                format!(" {} / {} / {pane_id}", app.machine(machine).map(|m| m.label.as_str()).unwrap_or(""), r.label)
            }
            Target::Workspace { machine, workspace_id } => {
                format!(" {} / {} / {workspace_id}", app.machine(machine).map(|m| m.label.as_str()).unwrap_or(""), r.label)
            }
            _ => format!(" {}", r.label),
        })
        .unwrap_or_default();
    buf.set_string(area.x, area.y, crumb, Style::default().fg(T.dim));
    let hint = match app.mode {
        Mode::Search => " type to filter · enter keep · esc clear",
        Mode::Prompt(_) => " enter confirm · esc cancel",
        Mode::Confirm(_) => " y confirm · esc cancel",
        Mode::Normal => " ↑↓ rows · ←→ space · enter open · n/t/m create · c/r/x/D edit · a/b/w/i/d filter · / search · q quit",
    };
    buf.set_string(area.x, area.y + 1, hint, Style::default().fg(T.dim));
    if app.last_pane.is_some() {
        put_right(
            buf,
            area,
            area.y + 1,
            vec![Span::styled("esc → last pane ", Style::default().fg(T.dim))],
        );
    }
}

fn draw_prompt(f: &mut Frame, area: Rect, title: &str, input: &str) {
    let w = (area.width.saturating_sub(4)).min(70);
    let rect = Rect::new(area.x + (area.width - w) / 2, area.y + area.height / 2 - 2, w, 5);
    Clear.render(rect, f.buffer_mut());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(T.accent))
        .style(Style::default().bg(T.bg))
        .title(Span::styled(format!(" {title} "), Style::default().fg(T.accent)));
    let inner = block.inner(rect);
    block.render(rect, f.buffer_mut());
    let line = Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), 1);
    f.buffer_mut().set_style(line, Style::default().bg(T.line).fg(T.text));
    f.buffer_mut().set_string(line.x, line.y, format!(" {input}"), Style::default().fg(T.text).bg(T.line));
    f.set_cursor_position((line.x + 1 + input.width() as u16, line.y));
}
