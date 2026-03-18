# Interactive / agentic SSH with Cursor

**Auto-login (keys) is enough for SSH.** To make the **AI agent** work *on the server* (edit files, run shell there), the workspace must be **opened on the remote machine** via Cursor’s Remote SSH—not just a local terminal running `ssh`.

## 1. Use Cursor’s Remote SSH (not Microsoft’s)

1. Extensions → install **`Anysphere Remote SSH`** (`anysphere.remote-ssh`).
2. If you still have **`Remote - SSH` (Microsoft)** enabled, **disable or uninstall** it to avoid conflicts.
3. `Ctrl+Shift+P` → **Remote-SSH: Connect to Host…** → pick a host from `~/.ssh/config` (add `Host` / `HostName` / `User` / `IdentityFile` entries first).

After the window reloads, the status bar shows **SSH: hostname**. Terminals, file tree, and **Agent** actions target that remote machine.

## 2. SSH config tips

| Goal | Setting |
|------|--------|
| Fewer idle drops | `ServerAliveInterval 60`, `ServerAliveCountMax 3` (added to your `~/.ssh/config` **Host \*** if missing) |
| Faster repeat connects | `ControlMaster` / `ControlPath` / `ControlPersist` — **can conflict** with multiple Cursor windows on the *same* host; remove if unstable |

## 3. If the agent still feels “local only”

- Confirm the window title / status bar shows the **remote** host.
- Open a folder on the server (e.g. `/opt/relay` or your deploy dir), not a local clone, when you want deploy edits there.
- For one-off commands without full remote workspace: use the **integrated terminal** after connecting via Remote-SSH, or paste output back into chat.

## 4. Cursor Server install failures on Linux

If Remote-SSH hangs on “Installing Cursor Server”, check disk space on the server, `~/.cursor-server` permissions, and [Cursor forum / GitHub issues](https://github.com/cursor/cursor/issues) for lock-file / proxy fixes.
