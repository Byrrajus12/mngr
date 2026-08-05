# mngr

Windows desktop overlay for monitoring and controlling AI coding agents. Sits on the edge of your screen as a slim rail, notifies you when your agent needs you, expands into a panel that sees all of your sessions and lets you interact with them, monitor usage, and open the exact session.

<img width="920" height="518" alt="overview-ezgif com-optimize" src="https://github.com/user-attachments/assets/9eed5913-be88-407c-896c-01dd4130dbee" />

<!-- TODO: GIF showing permission approval flow -->

## What it does

You run Claude Code and Codex sessions across different terminals, IDEs, or apps. mngr watches all of them from one place.

- See every active session at a glance: project, provider, terminal, elapsed time, what the agent is doing right now
- Approve or deny permission requests without switching windows. The agent wants to edit a file or run a command, you see the details and say yes or no
- Answer agent questions from the overlay
- Check usage quota for both Claude and OpenAI with live rate limit percentages and reset countdowns
- Jump to any session's terminal tab instantly

## Install

Download the latest release from [Releases](https://github.com/Byrrajus12/mngr/releases).

Run the installer. On first launch, mngr installs hook scripts for Claude Code and Codex automatically. For Codex, you'll need to trust the hooks once via `/hooks` in the Codex TUI.

### Build from source

Requires Node.js, Rust, and the [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/).

```
git clone https://github.com/Byrrajus12/mngr.git
cd mngr
npm install
cargo build --manifest-path src-tauri/Cargo.toml
npm run tauri dev
```

## How it works

mngr uses the hook systems built into Claude Code and Codex. A PowerShell hook script is registered for lifecycle events (session start/end, tool use, permission requests, stop). When an event fires, the hook sends a JSON payload over a named pipe to mngr's Rust backend. For permission requests, the hook waits for mngr to write a response file with the user's decision, then returns the appropriate allow/deny response to the agent.

```
Claude Code / Codex
  -> hook event fires
    -> PowerShell hook script reads stdin JSON
      -> sends enriched payload to \\.\pipe\mngr
        -> Rust backend updates session state
          -> React frontend renders session cards

Permission request flow:
  hook fires -> pipe event to mngr -> card appears in panel
  user clicks allow/deny -> response file written -> hook reads it -> returns decision to agent
```

No API keys needed. No data leaves your machine. Usage tracking for Claude reads your existing OAuth credentials locally to poll your rate limits. Codex usage is read directly from session transcript files on disk.

## Supported agents

| Agent | Monitoring | Approve/deny | Questions | Usage tracking | Jump to session |
|-------|-----------|-------------|-----------|---------------|-----------------|
| Claude Code | yes | yes | yes | yes (OAuth + hook cache) | yes -- tab-level in Windows Terminal, window-level everywhere else, Claude Desktop deep link for app sessions |
| Codex | yes | yes | not yet | yes (JSONL transcripts) | yes -- tab-level in Windows Terminal, window-level everywhere else |

Codex hooks are synchronous, which means mngr takes over the approval UI entirely when it's running. When mngr is closed, hooks exit cleanly and Codex shows its own prompts as usual.

## Architecture

Tauri v2 app. Rust backend handles the named pipe listener, session state machine, hook installation, response file IPC, usage data, and window management. React + TypeScript frontend renders the overlay. Custom CSS, no component libraries.

```
src-tauri/src/lib.rs          Rust backend (sessions, pipe, IPC, usage, hooks)
scripts/claude-hook.ps1       Claude Code hook script
scripts/codex-hook.ps1        Codex hook script
src/components/SessionCard.tsx Session card with permission/question UI
src/components/Panel.tsx       Panel shell, usage display
src/components/Filament.tsx    Edge rail with status segments
```

Session state machine: Working -> WaitingForApproval -> Working -> Idle -> Done. SessionEnd events from either agent set Done immediately instead of waiting for the 5-minute idle timeout.

## Known limitations

Some of these are upstream issues in the agent CLIs:

- Codex App with "approve for me" enabled still gets intercepted by mngr's hooks. Codex sends `permission_mode=default` regardless of approval settings, so there's no way to detect auto-approve from the hook payload.
- Codex `SessionStart` fires on first prompt submission, not when the session actually opens ([openai/codex#15266](https://github.com/openai/codex/issues/15266)).
- Codex permission requests don't include file diffs, only the command string.
- Codex question answering isn't supported yet (no hook event for `request_user_input`).
- Dual display (showing permissions in both mngr and the terminal simultaneously) isn't possible with Codex's sync hooks.
