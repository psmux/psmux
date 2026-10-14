# Claude Code Agent Teams

psmux has first-class support for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) agent teams. When Claude Code runs inside a psmux session, it automatically spawns teammate agents in separate tmux panes instead of running them in-process, giving you full visibility into what each agent is doing.

## Prerequisites

### PowerShell 7+

[Install PowerShell 7 on Windows](https://learn.microsoft.com/en-us/powershell/scripting/install/install-powershell-on-windows?view=powershell-7.6)

To work with Claude Code, psmux **requires PowerShell 7 or later**. The env shim and teammate mode injection rely on PowerShell 7+ features that are not available in the legacy Windows PowerShell 5.1.

Check your current version:

```powershell
$PSVersionTable.PSVersion
```

If you are on an older version, install PowerShell 7+ via winget:

```powershell
winget install --id Microsoft.PowerShell --source winget
```

After installation, restart your terminal and verify the version again.

- `pwsh` will run the new version
- `powershell` will still run the older legacy version as a fallback

You may need to restart VS Code for changes to the default terminal to take effect.

> **Credit:** This prerequisite documentation was contributed by [@LiamKarlMitchell](https://github.com/LiamKarlMitchell) in [#184](https://github.com/psmux/psmux/pull/184) after discovering the PowerShell version requirement while troubleshooting [#173](https://github.com/psmux/psmux/issues/173).

## Quick Start

1. **Install psmux** (see [README](../README.md#installation))

2. **Start a psmux session:**

   ```powershell
   psmux new-session -s work
   ```

3. **Run Claude Code inside the psmux pane:**

   ```powershell
   claude
   ```

4. **Ask Claude to create a team.** Claude Code will automatically split panes for each teammate agent.

That's it. No extra configuration needed. psmux handles everything automatically.

## How It Works

When a pane spawns inside psmux, several environment variables are set automatically:

| Variable | Value | Purpose |
|----------|-------|---------|
| `TMUX` | `/tmp/psmux-{pid}/...` | Tells Claude Code it's inside tmux |
| `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` | `1` | Enables the agent teams feature gate |
| `PSMUX_CLAUDE_TEAMMATE_MODE` | `tmux` | Triggers the `--teammate-mode tmux` CLI injection |

Claude Code detects the `TMUX` environment variable, recognizes it's inside a tmux-compatible multiplexer, and uses the **TmuxBackend** to spawn teammate agents via `split-window` and `send-keys`: the same mechanism it uses on Linux/macOS tmux.

### The Two Things psmux Fixes

Claude Code's standalone binary (the Bun SFE `claude.exe`) has two issues on Windows that psmux works around:

1. **Agent teams feature gate**: The entire teammate tool-set (spawnTeam, spawnTeammate) is gated behind `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`. Without this env var, Claude only has the in-process "Agent" tool and never creates separate panes. psmux sets this automatically.

2. **`teammateMode` default**: Early standalone binaries ignored `teammateMode: "tmux"` from `~/.claude/settings.json`, so psmux injects `--teammate-mode tmux` via a PowerShell wrapper function that's loaded in every pane. The injection only happens when you have NOT configured `teammateMode` yourself: if the key is present in your user settings (`~/.claude/settings.json` or `$env:CLAUDE_CONFIG_DIR\settings.json`), your project's `.claude/settings.json` / `.claude/settings.local.json` (searched upward from the current directory), managed settings, or an explicit `--teammate-mode` argument, psmux leaves the invocation untouched and your configuration wins.

## Configuration Options

These options can be set in `~/.psmux.conf` or at runtime:

```tmux
# Auto-inject --teammate-mode tmux for Claude Code (default: on)
set -g claude-code-fix-tty on

# Disable the Claude Code teammate-mode workaround
set -g claude-code-fix-tty off
```

### What each option controls

| Option | Default | Description |
|--------|---------|-------------|
| `claude-code-fix-tty` | `on` | Sets `PSMUX_CLAUDE_TEAMMATE_MODE=tmux` and `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`, and defines a `claude` wrapper function that injects `--teammate-mode tmux` when `teammateMode` is not configured in any of your Claude Code settings files (your settings always take priority) |

`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` is only filled in when the pane would not already have it: a value inherited from the environment that started the server, or one you set with `set-environment -g CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS 0`, is kept as is. With `claude-code-fix-tty off` psmux does not set it at all.

### Your own `claude` or `env` always wins

The `claude` wrapper and the `env` shim are installed after your PowerShell profile has run, and only when the name is still free. If your profile defines `claude` or `env` as a function, filter or alias, psmux leaves it alone, so the pane runs exactly what runs outside psmux. In that case the `--teammate-mode tmux` injection does not happen; add `"teammateMode": "tmux"` to `~/.claude/settings.json` or pass the flag from your own wrapper.

## Two Agent Systems in Claude Code

Claude Code has **two completely separate agent systems**. Understanding both is critical because psmux can only control one of them.

### 1. Teammate Agents (tmux panes) ✅

The **teammate system** spawns agents in visible tmux panes. This is the system psmux fully supports.

- Triggered when the model passes `team_name` + `name` to the subagent tool
- Gated by `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` (psmux sets this)
- Controlled by `--teammate-mode tmux` (psmux injects this)
- Each agent gets its own pane with full terminal visibility
- Lower-tier models (Haiku, Sonnet) tend to prefer this path

### 2. Worktree Agents (in-process, invisible) ⚠️

The **worktree system** creates isolated git worktrees and runs agents in-process, **invisible to the user**.

- Triggered when the model passes `isolation: "worktree"` to the subagent tool
- Creates git worktrees at `.claude/worktrees/agent-<id>/` via `git worktree add`
- Each agent works on a separate branch in an isolated repo copy
- Runs entirely in-process (no pane, no terminal output visible)
- Higher-tier models (Opus) tend to prefer this path for git-level isolation
- **On Windows, worktree tmux integration is hardcoded disabled** (`"--tmux may not have effect on Windows when model chooses worktrees. Opus tends to always choose that."`)
- There is **no env var or setting** to force worktree agents into tmux panes

### Why Opus says "Let me launch agents in worktrees"

Both systems are exposed through the **same subagent tool**. The model chooses which to use:

| Parameter | System | Visibility | Model preference |
|-----------|--------|------------|-----------------|
| `team_name` + `name` | Teammate | Visible tmux pane | Haiku, Sonnet |
| `isolation: "worktree"` | Worktree | Invisible in-process | Opus |

Opus prefers worktree agents because they provide **git-level isolation**: each agent works on its own branch and can't cause merge conflicts with other agents. The tradeoff is zero visibility.

### Workaround: Project Instructions

Since the model decides which system to use, you can influence its choice via `CLAUDE.md` project instructions:

```markdown
# Agent Configuration
When spawning subagents, always use the teammate system (team_name + name parameters)
instead of worktree isolation. This ensures agents are visible in tmux panes.
Do NOT use isolation: "worktree". Use teammates instead.
```

Place this in your project's `CLAUDE.md` or `~/.claude/CLAUDE.md` for global effect. This is a **best-effort** approach, the model may still choose worktree isolation for complex parallel tasks.

## Important: Interactive Mode Required

Agent teams spawn in separate tmux panes only when Claude Code is running **interactively** (the default when you type `claude` in a pane). When using `-p` (pipe/print mode), Claude intentionally runs agents in-process since there's no interactive terminal to split.

```powershell
# ✅ Interactive: agents spawn in tmux panes
claude

# ❌ Pipe mode: agents run in-process (by design)
claude -p "do something"
```

## Verifying the Setup

To confirm everything is configured correctly inside a psmux pane:

```powershell
# Check environment variables
Write-Host "TMUX: $env:TMUX"
Write-Host "AGENT_TEAMS: $env:CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"
Write-Host "TEAMMATE_MODE: $env:PSMUX_CLAUDE_TEAMMATE_MODE"
```

Expected output:
```
TMUX: /tmp/psmux-{pid}/default,{port},0
AGENT_TEAMS: 1
TEAMMATE_MODE: tmux
```

You can also verify the `claude` wrapper is active:

```powershell
Get-Command claude | Format-List
```

If the wrapper is active, this shows a `Function` (not an `Application`). The wrapper auto-injects `--teammate-mode tmux` when calling `claude.exe`, unless `teammateMode` is already configured in your settings.json (user, project, or managed scope) or passed explicitly on the command line. Your own configuration always outranks the psmux default.

## What the Teammate Backend Asks of psmux

Claude Code's `TmuxBackend` drives the multiplexer with a small, fixed set of tmux commands. All
of them work against psmux, and the whole sequence is pinned by an end to end test
(`tests/test_issue580_teammate_backend.ps1`, [#580](https://github.com/psmux/psmux/issues/580)):

| What Claude Code runs | What psmux does |
|---|---|
| `new-session -d -s <team> -- cat` | tmux's blocker idiom: a pane that sits and reads stdin. On Windows a bare `cat` would hit PowerShell's `Get-Content` alias and wedge at a parameter prompt, so psmux substitutes a real stdin draining blocker, on both the `new-session` and `new-window` shapes and on the direct exec (`--`) path |
| `split-window -t %N ...`, `send-keys -t %N ...`, `respawn-pane -t %N` | Bare `%id` pane targets resolve across every window of the session, not just the active one, and a target that does not exist is refused with an error at exit 1 instead of silently acting on the active pane |
| `set-option -p -t %N remain-on-exit failed` | Pane scoped options. `-p` is the tmux pane scope flag, not a target flag. `remain-on-exit` follows tmux semantics per pane (`on`, `off`, `failed`): a teammate that crashes stays visible with its error, a teammate that exits cleanly closes its pane |
| `show-options -p -t %N` | Lists the pane's options as `name value` lines. The `-v` flag and an option name filter are not yet applied at pane scope (still open under #580), so parse the `name value` shape |

Exactly two pane scoped options exist, `remain-on-exit` and psmux's own `@mouse-force` (see
below). Any other name is refused with
`pane-scoped option 'x' is not supported (supported: remain-on-exit, @mouse-force)` at exit 1
rather than stored as a silent no-op, and a `%id` that does not exist answers
`can't find pane: %N`.

## Mouse and the Wheel in a Claude Code Pane

Claude Code reads the mouse as VT bytes on stdin and enables mouse tracking itself (it sends
DECSET 1000, 1002, 1003 and 1006 at startup). Two Windows specifics affect it:

- **Windows builds below 22523** (Windows 10, Windows Server 2019 and 2022): conhost does not hand
  an SGR mouse report on the ConPTY input pipe to the child, so Claude Code cannot receive the
  wheel there at all. On those builds a dropped console registration used to turn each wheel
  notch into Up and Down arrow keys, which is the "Scroll wheel is sending arrow keys" message
  ([#597](https://github.com/psmux/psmux/issues/597)). psmux now keeps the registration alive on
  every build, so the wheel is a no-op over Claude Code on those builds rather than a stream of
  arrow keys. On 22523 and above the wheel works.
- **A `node` child entering raw mode** overwrites the pane console's mode word and drops
  `ENABLE_MOUSE_INPUT` permanently, which used to silence the wheel for the rest of the pane's
  life. psmux now latches the wheel authorization on the pane for as long as the process that
  earned it is alive ([#613](https://github.com/psmux/psmux/issues/613)). If a pane still never
  earns it, `set-option -p -t %N @mouse-force on` exempts that pane from the gate.

See [mouse-ssh.md](mouse-ssh.md) and [faq.md](faq.md) for the full picture.

## Troubleshooting

### Teammates stop landing in panes after you open Agent View

Pressing Left in Claude Code to open Agent View makes Claude Code re-execute itself with a fresh
argv, spawned directly from the binary rather than through your shell. The `--teammate-mode tmux`
flag that psmux's `claude` wrapper function injected is not carried across, so the relaunched
process falls back to Claude Code's built in default, which is `in-process`. Every environment
variable survives the round trip; it is only the flag that is lost
([#578](https://github.com/psmux/psmux/issues/578), upstream behaviour).

The fix that survives Agent View is to set the mode in your Claude Code settings so it does not
depend on argv at all. In `~/.claude/settings.json`:

```json
{ "teammateMode": "tmux" }
```

The `/config` screen inside Claude Code sets the same key. Once `teammateMode` is configured there,
psmux deliberately stops injecting the flag so the two never fight. Typing `claude` again at the
pane prompt also restores it for that launch, because that goes back through the wrapper.

### Agents still running in-process

1. **Check you're in interactive mode**: not using `-p` or `--print`
2. **Verify env vars**: run the verification commands above
3. **Check debug log**: start Claude with `--debug-file $env:TEMP\claude_debug.log` and look for:
   - `[TeammateModeSnapshot] Captured from CLI override: tmux`: teammate mode is set
   - `[BackendRegistry] isInProcessEnabled: false`: tmux panes will be used
   - `[BackendRegistry] isInProcessEnabled: true (non-interactive session)`: you're in pipe mode

### Opus using "worktree agents" instead of tmux panes

This is expected behavior. Opus prefers `isolation: "worktree"` over the teammate system. These are two completely different agent systems, see [Two Agent Systems](#two-agent-systems-in-claude-code) above.

**What you'll see:** Claude says "Let me launch 3 implementation agents in worktrees": agents run invisibly, no panes appear.

**Workaround:** Add a `CLAUDE.md` instruction telling the model to prefer teammates over worktree isolation. This is best-effort, the model ultimately decides.

### Teammate panes show "The module 'X' could not be loaded"

A teammate pane that opens with

```
&: The module 'Code' could not be loaded. For more information, run 'Import-Module Code'.
```

is the signature of a launch line that lost its quoting on the way into the
pane ([#634](https://github.com/psmux/psmux/issues/634)). Claude Code sends the
teammate command as one operand,
`cd '<cwd>' && env <VAR=val ...> '<claude>' <flags>`, and it POSIX quotes every
value that contains a space. psmux used to scan that assignment run on plain
whitespace, so a value such as `XDIR='D:\POC Code\todosample'` was read as the
assignment `XDIR='D:\POC` plus a leftover `Code\todosample'`, and the leftover
became the program psmux invoked. PowerShell reads `Name\Command` as a module
qualified call, which is where the module name in the message comes from: it is
whatever word followed the first space in the value.

The scan is quote aware from psmux 3.3.9, so projects and Claude Code installs
under paths with spaces (`OneDrive - Contoso`, `POC Code`, `Program Files`) work
without any workaround. On 3.3.8 and earlier the workaround is to launch the
team from a path without spaces, or to clear any environment variable whose
value contains a space from the list Claude Code forwards to teammates.

### Claude command not found

Make sure `claude.exe` is on your PATH. Install via:
```powershell
npm install -g @anthropic-ai/claude-code
```

### Wrapper not injecting `--teammate-mode`

If `teammateMode` is set anywhere in your Claude Code settings (user, project, or managed settings.json), the wrapper intentionally does NOT inject the flag; your configuration is respected as-is. Remove the key from your settings if you want the psmux default back.

The wrapper is also only defined when `claude-code-fix-tty` is `on` (default). Check:
```powershell
tmux show-options -g claude-code-fix-tty
```

## Technical Details

For the curious, here's what happens under the hood when Claude Code spawns a teammate:

1. Claude calls `spawnTeammate` tool (available because `T8()` gate passes due to `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`)
2. `BackendRegistry.detectAndGetBackend()` checks `isInProcessEnabled`:
   - If non-interactive → true → in-process (by design)
   - If interactive → checks `teammateMode` → `"tmux"` → false → uses TmuxBackend
3. `TmuxBackend` runs `tmux split-window` via psmux's tmux compatibility
4. Sends `cd <workdir> && claude.exe --agent-id <id> --agent-name <name> ...` via `tmux send-keys`
5. The teammate agent starts in its own pane with full terminal access
