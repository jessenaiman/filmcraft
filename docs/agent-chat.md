# Native Agent Chat — opt-in local prototype

Window ▸ Agent Chat opens the dockable native panel. It is disabled unless configured at launch;
it never starts an agent, selects a model or changes an existing agent's assignment. Configure an
**existing** Herdr target and the live FilmCraft loopback control address:

```sh
FILMCRAFT_AGENT_TARGET=video-dev \
FILMCRAFT_AGENT_CHAT_DIR=/absolute/path/to/private-session/agent-chat \
FILMCRAFT_AGENT_CONTROL_ADDR=127.0.0.1:19876 \
./target/release/filmcraft \
  --control 19876 --no-recover \
  --data-dir /absolute/path/to/private-session \
  /absolute/path/to/project.fcproj
```

`herdr` must be on the desktop process's `PATH`. Substitute the target, absolute chat directory,
control address, project and data directory for another session. Do not launch a second app at an
occupied port. Enabling this bridge grants the assigned agent access to **all existing** engine commands
and live UI controls, not just conversational advice. There is no new engine or ML capability: the
prompt points at this checkout's absolute `docs/control-protocol.md` and `docs/agents.md`, requests
dynamic discovery (`engine.commands`, `ui.menu.list`), inspection, same-project reversible edits,
rendered screenshots, undo and reporting actual observed results. Audio, captions, effects and export
are reached through the existing registry; unsupported features must be reported honestly. This is
local trusted-agent tooling, **not a permissions sandbox**. The prompt asks before destructive or
external operations/overwrites; it does not enforce an engine-command allowlist.

## Driving the panel

Control requests (JSON lines over TCP, not HTTP):

```json
{"id":1,"method":"ui.panel.show","params":{"panel":"AgentChat"}}
{"id":2,"method":"ui.agent.inspect"}
{"id":3,"method":"ui.agent.send","params":{"text":"Inspect this live project and report what you actually observe."}}
{"id":4,"method":"ui.agent.inspect"}
```

The CLI can send the same raw control method and JSON params object, so use identical method/params
with the CLI, MCP, or an in-app control request:

```sh
filmcraft-cli --bridge 127.0.0.1:19876 control ui.agent.inspect '{}'
filmcraft-cli --bridge 127.0.0.1:19876 control ui.agent.send '{"text":"Inspect this live project and report what you actually observe."}'
```

`control` requires a running app and loopback `--bridge`; params must be one JSON object (or omitted
for `{}`). It rejects headless project/demo, save, and data-directory options. Use `exec` for the
engine-command shortcut (`engine.execute`) rather than a raw control method.

Build the desktop with the existing transcription feature enabled:
`cargo build --release -p filmcraft --features whisper`.

Wait for transcript loading before sending. `ui.agent.send` and the Send button use the same dispatch
function. Automation ids: `agent.input`, `agent.send`, `agent.transcript`, `panel.tab.AgentChat`.
There is at most one request in flight, including while switching projects. Each request has unique
request/project/thread ids, bounded user text, project/sequence/selection summary, the control
address and the exact reply path. The native background worker invokes
`herdr agent prompt <target> <prompt>` with **separate argv arguments**, no shell. Its prompt instructs
the assigned agent to write a genuine UTF-8 JSON response atomically to `<requestId>.reply.json`:

```json
{"reply":"The actual agent response, with observed actions/results or a truthful error."}
```

## Bounds and limitations

The request is also saved as `<requestId>.request.json`; a per-project transcript file retains the
latest 64 entries (trimmed further to fit 3 MiB). Project paths identify saved-project conversations;
an unsaved project's conversation lasts for that app instance. Renaming/Save As starts a different
conversation. Input is limited to 8 KiB UTF-8, reply text to 32 KiB, request JSON to 24 KiB, selection
summaries to 64 ids and prompt history to eight short recent entries. Files are bounded on read;
transcript/reply symlinks and pipes are rejected. Dispatch has a 15-second subprocess deadline and
reply polling has a 300-second deadline; neither blocks the UI. Transcript/request/reply files are
local plaintext and retained: choose a private directory and remove old request/reply files yourself.
No credentials are added to requests. Errors appear in the panel and `ui.agent.inspect`. Reopening
an interrupted request never silently resends it. Timeout/interruption is **not cancellation** of the
remote agent: its actions may have run or may still run; inspect the live project and late reply
before retrying. Replies arriving after timeout are left on disk, not automatically reattached.

Implementation status: desktop release compilation verified. Live UI/screenshot and genuine
agent-reply verification belong to the session owner after relaunch. No simulated reply validates
this bridge, and no test suite was run for this bounded prototype. Process/filesystem glue is
native-only; the portable panel remains disabled on web.
