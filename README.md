# paddock

Home screen for your [herdr](https://herdr.dev) fleet. A standalone terminal
client that shows every machine, space, terminal and agent on one screen and
drops you into any pane with Enter.

![home screen concept](docs/home-concept.png)

## What it does

- One tree: machines → spaces → panes, with agent kind and status on the right.
- **Needs you** on top: blocked and done agents from every machine.
- Detail panel for the selected row: path, branch and dirty state, agents, tabs,
  the last lines of the pane's screen.
- Create spaces, worktrees and tabs. Rename, close, delete worktree checkouts.
- Connect a new machine (`herdr machine add`).
- Enter attaches your terminal to the pane. `ctrl+b q` detaches back to home.
  Esc reopens the last pane.

Data comes from the `herdr` CLI: `api snapshot` and `workspace list` locally
and through `herdr --machine <id>` for every enabled saved SSH machine. Panes
are attached with `herdr terminal attach`, over `ssh -t` for remote machines,
so rendering fidelity is herdr's own.

## Install

Requires herdr 0.9 or newer on this machine and on every remote, with saved
machines configured through `herdr machine add`.

```bash
cargo install --path .
paddock
```

Options: `--local-poll SECS` (default 1), `--remote-poll SECS` (default 3).

### Zed

Run it in a center pane, next to file buffers: open the command palette and
run **workspace: new center terminal**, then type `paddock`. Bind it once in
`keymap.json` if you want a key:

```json
{ "context": "Workspace", "bindings": { "cmd-shift-h": "workspace::NewCenterTerminal" } }
```

## Keys

| Key | Action |
|---|---|
| `↑↓` `j k` | move |
| `←→` `h l` | previous / next space |
| `enter` | open: attach to pane, or to the space's active pane |
| `esc` | clear search and filter, else reopen last pane |
| `/` | search; words match independently |
| `a b w i d` | filter: all, blocked, working, idle, done |
| `n` `t` `m` | new space, new worktree from branch, connect machine |
| `c` `r` `x` `D` | new tab, rename, close, delete worktree checkout |
| `q` | quit |

## Limits

- Remote machines are polled, not streamed. Local state is polled every second.
- Attaching to a remote pane needs non-interactive SSH to the saved target.
- herdr's own view does not follow paddock. Attaching focuses the pane on its
  server, which marks done agents as seen.

## Layout

```
src/main.rs   terminal lifecycle, event loop, suspend for attach
src/app.rs    state, keys, prompts, actions
src/herdr.rs  herdr CLI runner (local or --machine), pollers, git state
src/model.rs  snapshot types, row builder, filters
src/ui.rs     rendering
src/theme.rs  palette
```

Apache-2.0. Not affiliated with herdr.
