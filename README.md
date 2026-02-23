# task-warrior-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) server that wraps the [Taskwarrior](https://taskwarrior.org) CLI. Gives Claude (or any MCP client) structured, project-scoped access to your tasks.

## Why project-scoped?

Every read and write operation requires a `project` field. The server automatically prepends `project:<name>` to all Taskwarrior queries. Without this, a single `task next` call can dump thousands of unrelated tasks into your LLM's context — slow, expensive, and useless. An explicit `all_projects` boolean opt-out exists for the rare cases that genuinely need a global view.

## Requirements

- [Taskwarrior](https://taskwarrior.org/download/) (`task` on `$PATH`)

## Installation

### From MCP Registry (recommended)

Find `task-warrior-mcp` on the [MCP Registry](https://registry.modelcontextprotocol.io) and follow the installation prompt in your MCP client. No build step required.

### From source

Requires [Rust](https://rustup.rs) (stable).

```sh
git clone https://github.com/<you>/task-warrior-mcp
cd task-warrior-mcp
cargo build --release
```

The binary lands at `target/release/task-warrior-mcp`.

## Configuration

### Claude Code (global, all sessions)

If installed via the MCP registry, your client handles configuration automatically. For a local build:

```sh
claude mcp add --scope user taskwarrior /path/to/task-warrior-mcp/target/release/task-warrior-mcp
```

### Claude Desktop

Merge the snippet below into your `claude_desktop_config.json` (replace `<INSTALL_DIR>` with the absolute path to this repo):

```json
{
  "mcpServers": {
    "taskwarrior": {
      "command": "<INSTALL_DIR>/target/release/task-warrior-mcp",
      "args": []
    }
  }
}
```

**macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
**Linux:** `~/.config/Claude/claude_desktop_config.json`

## Tools

| Tool | Required | Optional |
|---|---|---|
| `add_task` | `description`, `project` | `due`, `tags`, `priority`, `wait`, `scheduled` |
| `list_tasks` | `project` | `filter`, `report`, `all_projects` |
| `search_tasks` | `pattern`, `project` | `filter`, `all_projects` |
| `get_task` | `id` | — |
| `modify_task` | `id`, `modifications` | — |
| `complete_task` | `id` | — |
| `delete_task` | `id` | — |
| `annotate_task` | `id`, `note` | — |

### Date syntax

`today` · `tomorrow` · `eow` · `eom` · `friday` · `2025-06-15` · `2025-06-15T14:30` · `today+3d` · `later`

### Filter virtual tags

`+OVERDUE` · `+DUE` · `+TODAY` · `+READY` · `+ACTIVE` · `+BLOCKED` · `+BLOCKING` · `+WAITING`

### Reports

`next` (default, urgency-sorted) · `list` · `all` · `completed` · `waiting` · `blocked`

### Priorities

`H` (high) · `M` (medium) · `L` (low)

## Development

```sh
just build          # cargo build --release
just test           # run all tests (each isolated in a temp taskwarrior DB)
just lint           # clippy + fmt check
```

Or use cargo directly:

```sh
cargo test
cargo clippy
cargo fmt
```

A pre-push hook runs fmt + clippy + tests automatically — no setup needed after cloning.

CI runs the same checks on every push and PR via GitHub Actions.

### Releasing

Requires: [`just`](https://github.com/casey/just), [`gh`](https://cli.github.com), `jq`, `openssl`, and [`cargo-edit`](https://github.com/killercup/cargo-edit) (`just setup`).

```sh
just setup                  # install cargo-edit (one-time)
just release                # bump patch, build, push tag, create GitHub release, update server.json
just release minor          # bump minor version
just release-version 1.2.3  # explicit version
just publish                # push server.json to the MCP registry (after mcp-publisher login)
```
