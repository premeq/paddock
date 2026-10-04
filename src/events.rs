use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::herdr::{shell_quote, Runner, Update};
use crate::model::Machine;

const LIFECYCLE: &[&str] = &[
    "workspace.created",
    "workspace.closed",
    "workspace.renamed",
    "workspace.focused",
    "workspace.updated",
    "tab.created",
    "tab.closed",
    "tab.renamed",
    "tab.focused",
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.focused",
    "pane.exited",
    "pane.agent_detected",
    "worktree.created",
    "worktree.removed",
];

const SLOW_POLL: Duration = Duration::from_secs(10);
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Keeps one machine fresh: a poller that fetches snapshots, and an event
/// subscription that wakes the poller the moment herdr reports a change.
/// Without events the poller runs at `every`; with them it slows to a safety net.
/// Returns a sender that forces an immediate refetch, used after paddock's own actions.
pub fn spawn_watcher(machine: &Machine, every: Duration, tx: Sender<Update>) -> Sender<()> {
    let runner = Runner::for_machine(machine);
    let id = machine.id.clone();
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    let (panes_tx, panes_rx) = mpsc::channel::<Vec<String>>();
    let live = Arc::new(AtomicBool::new(false));
    spawn_events(runner.clone(), wake_tx.clone(), panes_rx, live.clone());
    std::thread::spawn(move || poll_loop(runner, id, every, tx, wake_rx, panes_tx, live));
    wake_tx
}

fn poll_loop(
    runner: Runner,
    id: String,
    every: Duration,
    tx: Sender<Update>,
    wake: Receiver<()>,
    panes_tx: Sender<Vec<String>>,
    live: Arc<AtomicBool>,
) {
    let mut last_git = Instant::now() - Duration::from_secs(60);
    let mut paths = Vec::new();
    let mut known_panes: Vec<String> = Vec::new();
    loop {
        let started = Instant::now();
        match runner.snapshot() {
            Ok(snapshot) => {
                paths = snapshot
                    .workspaces
                    .iter()
                    .filter_map(|w| w.worktree.as_ref().map(|t| (w.workspace_id.clone(), t.checkout_path.clone())))
                    .collect();
                let mut panes: Vec<String> = snapshot.panes.iter().map(|p| p.pane_id.clone()).collect();
                panes.sort();
                if panes != known_panes {
                    known_panes = panes.clone();
                    let _ = panes_tx.send(panes);
                }
                let latency_ms = started.elapsed().as_millis() as u64;
                if tx.send(Update::Snapshot { machine: id.clone(), snapshot, latency_ms }).is_err() {
                    return;
                }
            }
            Err(e) => {
                if tx.send(Update::Failed { machine: id.clone(), error: e.to_string() }).is_err() {
                    return;
                }
            }
        }
        if last_git.elapsed() >= Duration::from_secs(30) {
            last_git = Instant::now();
            let info = runner.git_info(&paths);
            if tx.send(Update::Git { machine: id.clone(), info }).is_err() {
                return;
            }
        }
        let interval = if live.load(Ordering::Relaxed) { SLOW_POLL } else { every };
        match wake.recv_timeout(interval.saturating_sub(started.elapsed())) {
            Ok(()) => {
                std::thread::sleep(DEBOUNCE);
                while wake.try_recv().is_ok() {}
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn spawn_events(runner: Runner, wake: Sender<()>, panes_rx: Receiver<Vec<String>>, live: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut panes: Vec<String> = Vec::new();
        loop {
            while let Ok(p) = panes_rx.try_recv() {
                panes = p;
            }
            let (restart_tx, restart_rx) = mpsc::channel::<()>();
            let outcome = match Stream::connect(&runner) {
                Ok(mut stream) => {
                    if stream.subscribe(&panes).is_ok() {
                        live.store(true, Ordering::Relaxed);
                        let _ = wake.send(());
                        let reader_wake = wake.clone();
                        let lines = stream.lines();
                        let reader = std::thread::spawn(move || {
                            for line in lines {
                                let Ok(line) = line else { break };
                                if line.contains("subscription_started") {
                                    continue;
                                }
                                if reader_wake.send(()).is_err() {
                                    break;
                                }
                            }
                            let _ = restart_tx.send(());
                        });
                        // Re-subscribe when the pane set changes so agent status
                        // events, which are per pane, cover every pane.
                        loop {
                            match panes_rx.recv_timeout(Duration::from_millis(500)) {
                                Ok(p) => {
                                    let mut p = p;
                                    while let Ok(more) = panes_rx.try_recv() {
                                        p = more;
                                    }
                                    if p != panes {
                                        panes = p;
                                        break;
                                    }
                                }
                                Err(RecvTimeoutError::Timeout) => {
                                    if restart_rx.try_recv().is_ok() {
                                        break;
                                    }
                                }
                                Err(RecvTimeoutError::Disconnected) => return,
                            }
                        }
                        stream.close();
                        let _ = reader.join();
                        Ok(())
                    } else {
                        Err(())
                    }
                }
                Err(_) => Err(()),
            };
            live.store(false, Ordering::Relaxed);
            if outcome.is_err() {
                match panes_rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(p) => panes = p,
                    Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
        }
    });
}

enum Stream {
    Local(std::os::unix::net::UnixStream),
    Ssh(Child),
}

impl Stream {
    fn connect(runner: &Runner) -> std::io::Result<Stream> {
        match &runner.ssh_target {
            None => {
                let path = std::env::var("HERDR_SOCKET_PATH").unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_default();
                    match &runner.session {
                        Some(s) if s != "default" => format!("{home}/.config/herdr/sessions/{s}/herdr.sock"),
                        _ => format!("{home}/.config/herdr/herdr.sock"),
                    }
                });
                std::os::unix::net::UnixStream::connect(path).map(Stream::Local)
            }
            Some(target) => {
                let sock = match &runner.session {
                    Some(s) if s != "default" => format!("~/.config/herdr/sessions/{s}/herdr.sock"),
                    _ => "~/.config/herdr/herdr.sock".to_owned(),
                };
                let remote = format!("exec python3 -c {} {}", shell_quote(BRIDGE), shell_quote(&sock));
                Command::new("ssh")
                    .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=2", "-S", "none", target, &remote])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .map(Stream::Ssh)
            }
        }
    }

    fn subscribe(&mut self, panes: &[String]) -> std::io::Result<()> {
        let mut subs: Vec<serde_json::Value> = LIFECYCLE.iter().map(|t| json!({ "type": t })).collect();
        for p in panes {
            subs.push(json!({ "type": "pane.agent_status_changed", "pane_id": p }));
        }
        let line = json!({ "id": "paddock", "method": "events.subscribe", "params": { "subscriptions": subs } }).to_string() + "\n";
        match self {
            Stream::Local(s) => s.write_all(line.as_bytes()),
            Stream::Ssh(c) => c.stdin.as_mut().expect("piped stdin").write_all(line.as_bytes()),
        }
    }

    fn lines(&mut self) -> Box<dyn Iterator<Item = std::io::Result<String>> + Send> {
        match self {
            Stream::Local(s) => Box::new(BufReader::new(s.try_clone().expect("clone unix stream")).lines()),
            Stream::Ssh(c) => Box::new(BufReader::new(c.stdout.take().expect("piped stdout")).lines()),
        }
    }

    fn close(self) {
        match self {
            Stream::Local(s) => {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            Stream::Ssh(mut c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

/// Bridges stdin/stdout to the herdr unix socket on a remote machine, so one
/// SSH connection carries the event stream without needing socat or nc there.
const BRIDGE: &str = r#"import os,socket,sys,threading
s=socket.socket(socket.AF_UNIX);s.connect(os.path.expanduser(sys.argv[1]))
def up():
    for l in sys.stdin.buffer:s.sendall(l)
threading.Thread(target=up,daemon=True).start()
while True:
    d=s.recv(65536)
    if not d:break
    sys.stdout.buffer.write(d);sys.stdout.buffer.flush()"#;
