# Terminal contributions

A package can show things in the terminal UI (`praxis tui`):

* a **status line** in the header, refreshed on an interval, and
* **`/` commands** the operator types like any built-in command.

Both are backed by one of the package's own tools. The kernel runs that tool
exactly like a model-requested call: same catalog owner, enable switches,
declared secrets and sandbox. Nothing runs inside the TUI process, and a remote
TUI works the same way because everything goes through the gateway.

```json
{
  "name": "bili",
  "tools": [{"name": "bili_status", "description": "Cache usage", "parameters": {"type": "object"},
             "handler": {"type": "script", "path": "status.py", "interpreter": "python3"}}],
  "provides": {
    "terminal": {
      "status":   {"tool": "bili_status", "every_seconds": 30},
      "commands": [{"name": "bili", "tool": "bili_status", "description": "Show bili cache usage"}]
    }
  }
}
```

| Field | Rule |
|---|---|
| `status.tool`, `commands[].tool` | must be a tool of the same package |
| `status.every_seconds` | 10–3600 (default 60) |
| `commands[].name` | `[a-z][a-z0-9_-]{0,31}`, at most 8 per package, unique |
| `commands[].description` | at most 120 characters |

The status tool is called with `{}`; a command tool with
`{"text": "<rest of the line>"}` (`/bili details` → `{"text": "details"}`).
A script may print plain text or JSON with a `text`/`result` string.

What reaches the terminal is plain text: escape sequences and control
characters are removed (a package cannot clear the screen or set the window
title), the status line is the first non-empty line, at most 80 characters, and
command output is limited to 16 KiB. Built-in commands win over package
commands with the same name; between packages, the first by name wins.

Disabling the package removes its status line and commands within five
minutes (the TUI re-reads the list), or immediately after a restart.

Gateway endpoints (authenticated like the rest of `/v1`):

* `GET /v1/terminal` — `{"status": [{"plugin", "every_seconds"}], "commands": [{"name", "plugin", "description"}]}`
* `POST /v1/terminal/<session>/status/<plugin>` — `{"plugin", "text"}`
* `POST /v1/terminal/<session>/command/<name>` with `{"text": "…"}` — `{"plugin", "text"}`

Implementation: `src/plugins/terminal.rs`, `src/gateway/client_api.rs`,
`crates/praxis-tui/src/plugin_terminal.rs`.
