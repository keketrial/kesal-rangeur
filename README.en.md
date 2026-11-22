# kesal-rangeur

[🇫🇷 Français](README.md) · 🇬🇧 English · [🇪🇸 Español](README.es.md)

**Automatically files your downloads** (movies, TV series, cartoons, shows, documentaries, stand-up…) into the right place of your library, with **titles in your own language**.

Why? Sonarr and Radarr rely on TheTVDB, which often returns English titles. `kesal-rangeur` uses **TMDB**, which has localized titles (French, Spanish, German…).

## Features

- **Full simulation**: a tab shows everything that would happen (files to transfer, size, new folders, renames, duplicates, to-sort), downloadable report. Only then choose the mode: simulation / manual / automatic.
- Watches one or more **incoming folders** (Resilio Sync, torrent client…).
- **Waits for transfers to finish**: no temporary file (`.!sync`, `.part`…) and nothing modified for 30 min (configurable). Checked again right before copying.
- **Parses release names**: movie or series, season, full pack, quality, language tags.
- **Identifies with TMDB** in your language and picks the category from genres (Animation, Documentary, Reality, Talk…). Custom rules supported.
- **Finds the existing series folder** (even under a different name) and adds the season; otherwise creates "Title (year)". Same-title remakes are told apart by year.
- Handles shows where **TMDB has one entry per season** and renumbers episodes so Plex and Jellyfin match them.
- **Custom naming (optional)**: checkboxes (year, resolution, H.265/AV1 codec, audio, HDR, language…) or variables (`{n} {y} {s00e00} {t}`…), Plex style, TMDB episode titles, before → after preview. Off by default (original names kept).
- **Duplicate detection** (movie: title + year; episode: series + SxxEyy).
- **No extra disk space**: reflink (Btrfs/XFS), else hard link, else plain copy. The original stays in place.
- **When in doubt → "To sort"**: TMDB candidates with posters, you pick, it remembers.
- **History with an Undo button**, automatic retries, never leaves anything half-done.
- Notifies **Plex** and **Jellyfin** (and Home Assistant) after filing.
- Mobile-friendly web UI with **built-in help** (`?` button).

## Safety: dry-run mode

On first start the app is in **dry-run mode**: it analyses and proposes, but **writes nothing**. Review the result, then turn dry-run off in Settings when you are ready. Every real filing can be undone.

## Install

### Docker (recommended, e.g. on a NAS)

```yaml
services:
  rangeur:
    image: ghcr.io/YOUR-ACCOUNT/kesal-rangeur:latest
    ports: ["3050:3050"]
    user: "1026:100"                 # PUID:PGID owning your media
    volumes:
      - ./config:/config
      - /volume1/MEDIA:/media        # ONE mount for incoming AND destination folders
```

> **One mount for everything.** For instant copies, incoming and destination folders must be on the same Btrfs/XFS volume *and* inside one Docker mount. Check with:
> `docker exec kesal-rangeur kesal-rangeur selftest /media/incoming /media/movies`

Then open `http://your-server:3050` and follow the built-in help (`?`).

### Binaries

Windows (`.exe`), Linux and macOS (x86 and ARM): see *Releases*. Run `kesal-rangeur serve`. Reflink copies only work on Linux; elsewhere hard link or plain copy.

### Build

```sh
cargo build --release     # recent Rust; the UI is embedded in the binary
```

## Settings

Everything is configured in the UI. Files live in the config folder: `config.toml`, `secrets.json` (keys, mode 600), `choices.json`, `history.json`, `tmdb-cache.json`.

**TMDB key**: free at [themoviedb.org](https://www.themoviedb.org/settings/api). Each user enters their own; none is bundled. A step-by-step guide is in the app's help.

## Assistant (MCP)

An AI assistant (Claude Code, Claude Desktop…) can help you configure and operate the app: `kesal-rangeur mcp` is an [MCP](https://modelcontextprotocol.io) server (stdio) that drives an **already running** instance through its HTTP API. It keeps dry-run mode on until you have explicitly agreed to real copies.

```sh
claude mcp add kesal-rangeur -- kesal-rangeur mcp
# with Docker:
claude mcp add kesal-rangeur -- docker exec -i kesal-rangeur kesal-rangeur mcp
# app not on http://127.0.0.1:3050:
claude mcp add kesal-rangeur -e KESAL_URL=http://nas:3050 -- kesal-rangeur mcp
```

Then ask the assistant to use the `setup_kesal_rangeur` prompt (guided first-run setup). Details: [docs/mcp.md](docs/mcp.md).

## Network security

- **Password (recommended)**: set the `KESAL_PASSWORD` environment variable; the whole UI then asks for it (any username).
- **Host names**: access by IP address, `localhost`, a short name (Docker service) or `.local` is accepted. For a domain name (tunnel, reverse proxy), add it to `KESAL_ALLOWED_HOSTS` (comma-separated); this protects against DNS rebinding.
- Requests that change anything are refused when they come from another website (`Origin` header check).
- A token (Plex, Jellyfin, Home Assistant) is **bound to the address it was entered with**: if the address changes without a new token, the old one is erased.
- Still keep the UI on your local network or behind a VPN; do not expose it directly to the Internet.

## Roadmap

Tracker search (Prowlarr / Jackett, Torznab), release selection by preference, sending to your torrent client, new-episode watching.

## License

GPL v3 — see [LICENSE](LICENSE). This app must stay **free** (the TMDB API is free for non-commercial use only).

## TMDB

> This product uses the TMDB API but is not endorsed or certified by TMDB.

---

<img src="web/tmdb-logo.svg" alt="TMDB" width="200">

This application uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by TMDB.
