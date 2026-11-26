# kesal-rangeur

[🇫🇷 Français](README.md) · 🇬🇧 English · [🇪🇸 Español](README.es.md)

**Automatically files your downloads** (movies, TV series, cartoons, shows, documentaries, stand-up…) into the right place of your library, with **titles in your own language**.

Why? Sonarr and Radarr rely on TheTVDB, which often returns English titles. `kesal-rangeur` uses **TMDB**, which has localized titles (French, Spanish, German…).

## What it does

- Watches one or more **incoming folders** (Resilio Sync, torrent client…) and waits for transfers to finish.
- Recognizes each movie or series with **TMDB**, picks the right folder (existing series, new season, category) and detects **duplicates**.
- **Three modes**, switchable at the top of the UI: Simulation (nothing is written), Manual (you validate each item) or Auto (files what is safe on its own).
- **Copy** or **Move** per incoming folder. Instant copies on Btrfs/XFS (reflink).
- When in doubt, the item goes to **To sort**: you choose, the app remembers.
- Original names kept by default, or names of your choice (year, resolution, H.265/AV1 codec, audio…).
- **History with an Undo button** for every filing.
- Notifies **Plex**, **Jellyfin** and Home Assistant after filing.

On first launch the app is in **Simulation**: it shows everything it would do (files, size, renames, duplicates) without writing anything.

## Installation

### Docker (recommended, e.g. on a NAS)

```yaml
services:
  rangeur:
    image: ghcr.io/keketrial/kesal-rangeur:latest
    ports: ["3050:3050"]
    user: "1026:100"                 # PUID:PGID of your media owner
    volumes:
      - ./config:/config
      - /volume1/MEDIA:/media        # a single mount for the incoming folder and the destinations
```

Keep a **single mount** for the incoming folder and the destinations: that is what makes copies and moves instant. Then open `http://server-address:3050`.

### Windows, Linux, macOS

Download the file for your system from the [**Releases**](https://github.com/keketrial/kesal-rangeur/releases/latest) page.

- **Windows**: unzip, double-click `kesal-rangeur.exe`, then open <http://localhost:3050> in your browser. If Windows shows "Windows protected your PC" (the program is not signed), click *More info*, then *Run anyway*. Keep the window open: closing it stops the app.
- **Linux / macOS**: unpack, then run `./kesal-rangeur serve`.

### Build

```sh
cargo build --release
```

## Settings

Everything is set in the UI (`?` button for help). You need a free **TMDB key**, from [themoviedb.org](https://www.themoviedb.org/settings/api): everyone uses their own.

To protect the UI, set `KESAL_PASSWORD`. To reach it through a domain name, add it to `KESAL_ALLOWED_HOSTS`. Keep it on your local network or behind a VPN.

## AI assistant (optional)

`kesal-rangeur mcp` lets an assistant (Claude Code, Claude Desktop…) drive a running instance. It keeps Simulation on until you accept real copies. Details: [docs/mcp.md](docs/mcp.md).

## License

GPL v3, see [LICENSE](LICENSE). Free app (the TMDB API is free for non-commercial use).

## TMDB

> This product uses the TMDB API but is not endorsed or certified by TMDB.

---

<img src="web/tmdb-logo.svg" alt="TMDB" width="200">

This application uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by TMDB.
