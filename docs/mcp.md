# MCP server (AI assistant)

`kesal-rangeur mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server that lets an AI assistant
(Claude Code, Claude Desktop, any MCP client) configure and operate kesal-rangeur for you.

It speaks MCP over **stdio** (newline-delimited JSON-RPC 2.0) and relays to the HTTP API of a kesal-rangeur instance
that is **already running**. It has no extra dependencies and nothing but JSON-RPC is ever written to stdout (logs go to
stderr).

## Register it

```sh
# kesal-rangeur installed on the same machine, app on http://127.0.0.1:3050
claude mcp add kesal-rangeur -- kesal-rangeur mcp

# Docker (the MCP process runs inside the container, where the app listens on 127.0.0.1:3050)
claude mcp add kesal-rangeur -- docker exec -i kesal-rangeur kesal-rangeur mcp

# app running elsewhere
claude mcp add kesal-rangeur -e KESAL_URL=http://nas.local:3050 -- kesal-rangeur mcp
```

For Claude Desktop, add the same command to `claude_desktop_config.json`:

```json
{ "mcpServers": { "kesal-rangeur": { "command": "kesal-rangeur", "args": ["mcp"], "env": { "KESAL_URL": "http://127.0.0.1:3050" } } } }
```

`KESAL_URL` defaults to `http://127.0.0.1:3050`. If the app is not reachable, every tool answers with an error that
explains how to start it.

## First run

Ask the assistant to use the **`setup_kesal_rangeur`** prompt. It walks through: TMDB key, titles language, arrival
folder, destination folders, naming preset, a dry-run scan review, and only then (with your explicit agreement) real
copies. Folder paths are those seen by the app (inside the container with Docker).

## Safety model

* **Dry-run stays on** until you say otherwise. `configure` can never change it.
* `set_dry_run(enabled=false)` is refused unless `confirm` is exactly `I understand real copies will start`; the tool
  description tells the assistant to get your explicit agreement first. Enabling dry-run needs no confirmation.
* Even with dry-run off, `run` only files items planned as `ranger`, one copy at a time, and every filing can be undone
  (`undo`).
* Secrets (TMDB key, Plex token, Jellyfin key, Home Assistant token) are write-only: the app only reports whether they
  are set. Prefer typing them in the web UI; the assistant is told not to ask you to paste them in the chat.
* The web UI and API have no password: keep them on your local network.

## Tools

| Tool | What it does |
|---|---|
| `status` | Version, dry-run state, TMDB/Plex/Jellyfin configured, counters, missing folders, next-step hints. |
| `get_settings` | Current settings (secrets masked). |
| `configure` | Partial update (language, inputs, destinations, film_libraries, rules, naming, plex_url, jellyfin_url, ha_url, ha_service, copy_method, scan_interval_min, auto_range, optional secrets). Never touches dry-run. |
| `set_dry_run` | Enable/disable dry-run (disabling needs the confirmation phrase). |
| `test_tmdb`, `test_services` | Read-only connectivity tests. |
| `browse_folders` | List sub-folders to pick arrival/destination folders. |
| `scan` | Analyse the arrival folders, wait up to ~60 s, return counters. |
| `plan` | Summarize the analysis (`filter`: ranger, doublon, a_trier, attendre, ignorer, fait; `limit`). |
| `search_tmdb`, `choose` | Look up a title and tell the app which TMDB entry an item really is. |
| `preview_naming` | Example movie/episode paths for a naming preset or custom formats. |
| `history`, `undo` | Recent real filings; revert one. |
| `run` | File the planned items (refused with HTTP 403 while dry-run is on). |

`configure` semantics: `inputs`, `film_libraries` and `rules` replace the whole list; `destinations` is merged per
category (an empty string removes one); `naming` is merged field by field (a preset name is accepted as a shortcut).
Unknown fields are rejected so typos do not pass silently.

## Resources and prompts

* `kesal://help`: the in-app user guide (French) as markdown.
* `kesal://settings`: current settings as JSON.
* Prompt `setup_kesal_rangeur` (optional `language` argument): the guided first-run checklist.

## Tests

```sh
cargo test mcp                    # JSON-RPC layer and configure merge logic (no network)

# integration test against a SANDBOX instance (dry-run on, throw-away folders; never your real library)
KESAL_BIND=127.0.0.1:3061 target/debug/kesal-rangeur serve /path/to/sandbox/config.toml &
python3 -I tests/mcp_integration.py --bin target/debug/kesal-rangeur --url http://127.0.0.1:3061
```
