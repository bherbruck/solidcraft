# Agent chat

Agent chat is a side panel where you talk to a coding agent you already use (Claude Code,
Codex, Google Antigravity, OpenCode, Grok Build and others), or to a model behind an OpenAI-compatible endpoint you
choose (Ollama or LM Studio on your computer, OpenAI, any other URL), and have it work on the
design that's open. A coding agent runs on your computer under **your own account** with that
agent: SolidCraft doesn't sell or proxy a model and needs no key for it. An endpoint's key, if
it needs one, goes into your system keychain, never into SolidCraft's settings.

Desktop app only.

## Turning it on and off

Everything about it is on one page: **Preferences → AI**.

- **Show AI features** (on by default) is the master switch. Turned off, SolidCraft shows nothing
  of it: no AI button, no shortcut, no menu entry, no panel, and nothing is looked for or
  started. The page stays, so you can turn it back on.
- **Show Assistant button in application bar** (on by default): the **Assistant** icon in the application bar
  row, which stays there however wide the window is. Turn it off to keep only the shortcut and
  the menu entry.

With AI features on, the panel opens from:

- the **Assistant** icon in the application bar;
- **Ctrl+Shift+A** (Cmd+Shift+A on a Mac), which also works from the chat's own text box;
- **View settings** (the last button of the navigation bar under the view) → **Agent chat**.

Nothing runs until you need it: SolidCraft looks for installed agents (a quick `--version`
each, in the background) when the panel or the AI page first opens, and starts an agent only
when you send a message or press **Connect**.

## The panel

- **Docking:** drag the panel's header (the dotted grip and the title) to the other half of the
  window and let go. The side it would land on lights up while you drag. You can also pick
  **Left** or **Right** under Preferences → AI → Panel. SolidCraft remembers the side.
- **Width:** drag the panel's inner edge. SolidCraft remembers it.
- **Minimise** (the line in the header) folds the panel into a thin strip that keeps the chat
  icon, the agent's state, and a count of anything waiting for your answer. Click the icon to
  unfold it. **Close** (×) hides the panel. Neither stops the agent; **Stop** does.
- **The agent switcher** under the title picks the agent for this conversation; switching stops
  the one running. Next to it, when the agent has modes of its own (Claude Code has *Default*,
  *Accept Edits*, *Plan*…), a second drop-down sets the agent's mode. When the agent reports its models (an ACP agent's
  model list, or an endpoint's `/models`), a model drop-down shows the one in use and changes it;
  SolidCraft remembers the last choice for each agent and puts it back on the next session. An
  endpoint's agent also keeps it as its default model. An agent that reports no models shows no
  drop-down.

## Typing while the agent works

The input stays enabled during a turn. **Enter** queues the message: it shows in the conversation
as a **Queued** bubble that you can **Edit** or remove (×) until it is sent. Several queued
messages go as one prompt, in order, when the turn ends normally.

- **Send now** (on the bubble, or **Ctrl+Enter** / **Cmd+Enter** in the input) stops the current
  step, waits for the agent to say it stopped, then sends the queue as the next prompt, with a
  short note that the agent's previous step was interrupted. What the interrupted message changed
  undoes as one step, like any other message.
- **Stop** (the square next to the input, or **Esc** in the input) stops the agent and keeps
  the queue: the bubbles wait for **Send**.
- **New chat** (the header button, **Ctrl+Shift+L** / **Cmd+Shift+L** with the chat focused)
  stops what runs, clears the conversation and the queue and starts a fresh session on the same
  agent, keeping the agent and its mode. It asks first when a message is running or queued. The
  design's undo history is untouched.

## Agents

Preferences → AI → **Agents** lists every agent the chat can use, each with its kind (a program or
an endpoint), version or model, whether it looks signed in, **Test**, **Default** (the one a new
conversation starts with), its own permission setting, and **Remove** for the ones you added.

### Found by themselves

SolidCraft looks for agents that speak the [Agent Client Protocol](https://agentclientprotocol.com)
on your `PATH` and in the usual install folders a desktop app's `PATH` often misses (npm's
global folder, nvm, Volta, bun, `~/.local/bin`, `~/.grok/bin`, Homebrew, `%APPDATA%\npm`). For each one it
shows its **version** (from `--version`, given three seconds) and whether it looks **signed in**.
Version checks and agent sessions also use those folders on their `PATH`, so adapters can find
runtimes such as Node when SolidCraft is opened from Finder or the Dock. Existing absolute
`PATH` entries keep their order; no shell startup files are run.
SolidCraft only checks whether the agent's own login file (`~/.claude/.credentials.json`,
`~/.codex/auth.json`, `~/.gemini/oauth_creds.json`, …) or one of its key variables exists. It never
reads the file and never starts the agent to find out. On macOS Claude Code keeps its login in
the keychain, so SolidCraft doesn't guess there.

| Agent | What to install |
|---|---|
| Claude Code | `npm install -g @agentclientprotocol/claude-agent-acp` (the ACP adapter for Claude Code) |
| Codex | `npm install -g @agentclientprotocol/codex-acp` |
| Google Antigravity | Google's ACP server, a separate download from Google's `agy` command (which has no ACP mode): take the zip for your system from the [ACP registry](https://agentclientprotocol.com/registry), unzip it and put `agy_acp_server.par` (`agy_acp_server.exe` on Windows) on your `PATH`. The zip also holds `localharness_external`: leave the two together. It signs in itself (browser). |
| OpenCode | `npm install -g opencode-ai` (`opencode acp`) |
| XAI Grok | [Grok Build installer](https://docs.x.ai/build/overview), then `grok login` (`grok agent stdio` speaks ACP) |
| GitHub Copilot CLI, Qwen Code, goose | listed with their install command when missing |

The panel lists agents that aren't installed with their install command and a **Copy** button.
**Refresh** looks again after you install one. **Test** starts an agent briefly, asks it for a
session and ends it again.

### XAI Grok

Install **Grok Build** and run `grok login` in a terminal to sign in with your Grok account.
In SolidCraft, open **AI → Refresh**, select
**XAI Grok**, then **Connect**. You can make it the default under
**Preferences → AI → Agents**.

SolidCraft reuses Grok Build's sign-in in `~/.grok/auth.json`; it checks only whether that file
exists and does not read or copy its tokens. If sign-in is needed, the panel offers Grok's own
sign-in methods. You can also run `grok login` again and reconnect. Grok controls model access
and usage limits for your account. This connection does not require an API key in SolidCraft.
Grok Build also supports `XAI_API_KEY` for users who choose API authentication.

### Add agent…

- **Installed agent:** your own entry for an installed agent, for example with other arguments.
- **Custom command:** any other program that speaks ACP on stdin and stdout, such as
  `my-agent --acp`. SolidCraft starts it directly, not through a shell.
- **OpenAI-compatible API:** a preset (Ollama `http://localhost:11434/v1`, LM Studio
  `http://localhost:1234/v1`, OpenAI) or a custom URL, the model name, and an optional key.
  The key goes into the system keychain (Keychain on macOS, Credential Manager on Windows, the
  Secret Service on Linux) and is only ever sent over HTTPS or to this computer. **Test** asks the
  endpoint for its models. For these agents SolidCraft runs the conversation itself, with the
  same tools, limits and questions as for a coding agent, and at most 24 model requests per
  message.

## Signing in

If a coding agent isn't signed in, it says so and the panel shows **Sign in to the agent** with
the agent's own ways to sign in:

- **A command for a terminal**, e.g. `claude-agent-acp --cli auth login --claudeai`, with a **Copy**
  button. Run it in a terminal, sign in there, then press **I've signed in: connect again**.
- **A button** for agents that sign in by themselves (Codex's ChatGPT login opens your browser).
- **A variable to set** for agents that take a key from the environment. The panel has no field
  for a coding agent's key, on purpose.

Whatever you typed before signing in is sent once the agent is ready.

## What the agent can do

The agent reaches the open design only through SolidCraft's own tools (the
[MCP server](mcp.md)), which SolidCraft hands it when the chat starts:

- **Read the design:** list commands, inspect features, sketches and parameters, measure,
  take pictures of the view.
- **Change the design:** run any modelling command, alone or in a batch. Changes show in the
  window as they happen.
- **One undo per message:** everything the agent changes while answering one message becomes
  a single undo step, named after your message. The conversation shows *N changes, one undo
  step* with **Undo these changes** while nothing else has happened since.
- **Tool cards:** each tool call shows as a card with its state; click it to see what it returned.
- **Formatted text:** the agent's answers show as Markdown: headings, lists, quotes, code blocks
  (with **Copy**), bold, italic, `code` and links (web links only; each link is a button that opens
  it in your browser). What you type, and the agent's thoughts, stay plain text.
- **Attach view:** tick it to send a picture of the current view with your message, for agents
  that take pictures.

## Permissions

Preferences → AI → **Permissions** decides what the agent may do without asking, for every agent;
each agent can have its own setting in its row.

- **Ask every time:** every change to the design and every file action waits for **Allow**,
  **Always allow** or **Deny** in the panel.
- **Auto-allow model edits** (the default): modelling commands run at once; saving, opening, exporting,
  importing, starting a new design, any command given a file name, and undoing past the start of
  your message still ask.
- **Allow everything:** nothing asks. The page warns you: the agent can then save, overwrite and
  replace your design.

**Always allow** remembers that command for that agent. The page lists what you allowed this way,
per agent, with **Reset**.

These settings cover SolidCraft's tools. A coding agent's own tools (its shell, its file edits)
follow the agent's own mode, which you pick in the panel's header. When an agent asks before
using one of SolidCraft's tools, SolidCraft answers for you, since its own gate checks the call,
and notes it in the chat as *Auto-allowed*. With **Allow everything**, or after **Always allow**,
the agent's other questions are answered the same way and noted too.

What counts as "a command given a file name": a parameter that holds a file or folder (`file`,
`dir`, `output`, …), or `path` and `source` when the value reads like a file name (a separator, `~`,
a leading dot or a known extension), or any value of those in a command that exports, imports,
saves, opens or inserts. A sweep's or pattern's `path` (curve ids) and an occurrence's `path`
(ids) are geometry and run without a question.

## What it can't do

- **Files wait for you** unless you chose **Allow everything**. **Cancel** or **Stop** declines
  everything that's waiting, and a question nobody answers is declined after four minutes. The
  [MCP server's folder limits](mcp.md) still apply after you allow: the design's folder, the
  recovery folder and the server's own scratch folder.
- **A limit per message.** The agent may make 24 changes per message by default; you can set
  the number on the AI page. Reading the design doesn't count. When it reaches the limit, it's
  told to stop and report what's left; send another message to let it carry on.
- **No crash reports,** whatever the permissions. Crash reports can hold other designs and file
  paths.
- **Not the rest of the app.** It can't click the interface, change settings, quit, or drive the
  chat itself.
- **Its own tools stay off where we can turn them off.** SolidCraft declares no file system and
  no terminal to the agent, runs it in an empty working folder of its own, and asks the Claude
  Code adapter to turn off its shell, file and web tools for the session.

When the panel's agent is stopped, another agent is picked, or SolidCraft quits, the agent and
every program it started (the CLI it wraps, its MCP server) are stopped.

## Privacy

Nothing goes anywhere except to the agent you run, or the endpoint you added. SolidCraft starts
a coding agent as a program on your computer and talks to it over its standard input and
output. What the agent sends to its own service, under your account and that service's terms,
is between you and that service, exactly as when you use the agent in a terminal. SolidCraft
never sees, stores or passes on a coding agent's password, token or key. An endpoint's key is
kept in the system keychain and sent only to that endpoint. SolidCraft's own connection to the
agent's tools uses a random per-launch token in a file only you can read, which never passes
through the agent.

## Trying it without an account

A scripted test agent is in the source tree. It makes a block, rounds it and measures it, with
no model behind it:

```sh
cargo build --release -p solidcraft-ui-egui --example acp_test_agent
```

Then add it under Preferences → AI → **Add agent… → Custom command** with the path of
`target/release/examples/acp_test_agent`. Put "save" in your message to see an Allow / Deny card,
or "ask" to see the agent ask its own permission.
