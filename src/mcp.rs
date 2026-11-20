//! Serveur MCP (Model Context Protocol) : `kesal-rangeur mcp`.
//!
//! Permet à un assistant IA (Claude Code, Claude Desktop…) de configurer et d'utiliser kesal-rangeur.
//! Protocole : JSON-RPC 2.0, un message par ligne, sur stdin/stdout. RIEN d'autre que du JSON-RPC ne doit
//! être écrit sur stdout (les journaux vont sur stderr). Le serveur MCP ne fait que relayer vers l'API HTTP
//! d'une instance de kesal-rangeur DÉJÀ lancée (KESAL_URL, par défaut http://127.0.0.1:3050).
//!
//! Les descriptions des outils sont en anglais (le projet est publié en trois langues).

use regex::Regex;
use serde_json::{json, Map, Value};
use std::time::{Duration, Instant};

const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// Phrase exacte exigée pour désactiver l'essai à blanc.
pub const CONFIRM_PHRASE: &str = "I understand real copies will start";
const CATEGORIES: [&str; 6] = ["film", "serie", "dessin_anime", "telerealite", "documentaire", "spectacle"];
const HELP_HTML: &str = include_str!("../web/help.fr.html");

// ---------------------------------------------------------------------------------------------
// Client HTTP vers l'application
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub enum ApiError {
    Unreachable(String, String),
    Http(u16, String),
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unreachable(base, why) => write!(
                f,
                "Cannot reach kesal-rangeur at {base} ({why}). The app must be running first: start it with `kesal-rangeur serve` (or `docker compose up -d`), then retry. If it listens elsewhere, set the KESAL_URL environment variable of this MCP server (default http://127.0.0.1:3050)."
            ),
            ApiError::Http(code, body) => write!(f, "kesal-rangeur answered HTTP {code}: {}", body.trim()),
            ApiError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl From<ApiError> for String {
    fn from(e: ApiError) -> String {
        e.to_string()
    }
}

pub struct Api {
    pub base: String,
    http: reqwest::Client,
}

impl Api {
    pub fn new(base: &str) -> Api {
        let mut base = base.trim().trim_end_matches('/').to_string();
        if !base.starts_with("http://") && !base.starts_with("https://") {
            base = format!("http://{base}");
        }
        let mut b = reqwest::Client::builder().timeout(Duration::from_secs(30));
        if base.contains("127.0.0.1") || base.contains("localhost") || base.contains("[::1]") {
            b = b.no_proxy();
        }
        Api { base, http: b.build().unwrap_or_else(|_| reqwest::Client::new()) }
    }

    pub fn from_env() -> Api {
        Api::new(&std::env::var("KESAL_URL").unwrap_or_else(|_| "http://127.0.0.1:3050".into()))
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Value, ApiError> {
        let resp = req.send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() || e.is_request() {
                ApiError::Unreachable(self.base.clone(), e.without_url().to_string())
            } else {
                ApiError::Other(e.without_url().to_string())
            }
        })?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| ApiError::Other(e.without_url().to_string()))?;
        if !status.is_success() {
            return Err(ApiError::Http(status.as_u16(), text));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    pub async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, ApiError> {
        self.send(self.http.get(format!("{}{path}", self.base)).query(query)).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        self.send(self.http.post(format!("{}{path}", self.base)).json(body)).await
    }
}

// ---------------------------------------------------------------------------------------------
// Couche JSON-RPC
// ---------------------------------------------------------------------------------------------

fn rpc_error(id: Value, code: i64, msg: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg.into()}})
}

/// Traite une ligne reçue ; renvoie la ligne de réponse (None = rien à répondre).
pub async fn handle_line(api: &Api, line: &str) -> Option<String> {
    let parsed: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(rpc_error(Value::Null, -32700, format!("Parse error: {e}")).to_string()),
    };
    if let Some(batch) = parsed.as_array() {
        if batch.is_empty() {
            return Some(rpc_error(Value::Null, -32600, "Invalid Request: empty batch").to_string());
        }
        let mut out = Vec::new();
        for m in batch {
            if let Some(r) = handle_message(api, m).await {
                out.push(r);
            }
        }
        return if out.is_empty() { None } else { Some(Value::Array(out).to_string()) };
    }
    handle_message(api, &parsed).await.map(|v| v.to_string())
}

pub async fn handle_message(api: &Api, msg: &Value) -> Option<Value> {
    let Some(obj) = msg.as_object() else {
        return Some(rpc_error(Value::Null, -32600, "Invalid Request: expected a JSON object"));
    };
    let id = obj.get("id").cloned();
    let Some(method) = obj.get("method").and_then(|m| m.as_str()) else {
        // Réponse d'un client à une requête du serveur : on n'en envoie pas, donc on ignore.
        if obj.contains_key("result") || obj.contains_key("error") {
            return None;
        }
        return Some(rpc_error(id.unwrap_or(Value::Null), -32600, "Invalid Request: missing method"));
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        // Notification (notifications/initialized, notifications/cancelled…) : jamais de réponse.
        if method != "notifications/initialized" {
            eprintln!("[mcp] notification ignorée : {method}");
        }
        return None;
    };
    Some(match dispatch(api, method, &params).await {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, m)) => rpc_error(id, code, m),
    })
}

async fn dispatch(api: &Api, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
            let version = if PROTOCOLS.contains(&asked) { asked } else { PROTOCOLS[0] };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": false, "subscribe": false}, "prompts": {"listChanged": false}},
                "serverInfo": {"name": "kesal-rangeur", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Operate a running kesal-rangeur (automatic media organizer: arrival folder -> TMDB identification -> library). Always keep dry-run mode ON until the user explicitly agrees to real copies. Start with the `status` tool; for a first setup use the `setup_kesal_rangeur` prompt."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools_def()})),
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).ok_or((-32602, "Invalid params: missing tool name".to_string()))?;
            if !tools_def().iter().any(|t| t["name"] == name) {
                return Err((-32602, format!("Unknown tool: {name}")));
            }
            let empty = Map::new();
            let args = match params.get("arguments") {
                None | Some(Value::Null) => &empty,
                Some(Value::Object(m)) => m,
                Some(_) => return Err((-32602, "Invalid params: arguments must be an object".into())),
            };
            let (text, is_error) = match call_tool(api, name, args).await {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        "resources/list" => Ok(json!({"resources": [
            {"uri": "kesal://help", "name": "help", "title": "kesal-rangeur user guide", "description": "The in-app user guide (French): TMDB key, folders, naming variables, duplicates, Docker, security, FAQ.", "mimeType": "text/markdown"},
            {"uri": "kesal://settings", "name": "settings", "title": "Current settings", "description": "Current kesal-rangeur settings as JSON (secrets are masked by the app).", "mimeType": "application/json"}
        ]})),
        "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
        "resources/read" => {
            let uri = params.get("uri").and_then(|v| v.as_str()).ok_or((-32602, "Invalid params: missing uri".to_string()))?;
            match uri {
                "kesal://help" => Ok(json!({"contents": [{"uri": uri, "mimeType": "text/markdown", "text": help_markdown()}]})),
                "kesal://settings" => {
                    let s = api.get("/api/settings", &[]).await.map_err(|e| (-32603, e.to_string()))?;
                    Ok(json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": serde_json::to_string_pretty(&s).unwrap_or_default()}]}))
                }
                _ => Err((-32002, format!("Resource not found: {uri}"))),
            }
        }
        "prompts/list" => Ok(json!({"prompts": [{
            "name": "setup_kesal_rangeur",
            "title": "Set up kesal-rangeur",
            "description": "Guided first-run setup: TMDB key, arrival folder, destinations, naming, dry-run review, then (only with the user's agreement) real copies.",
            "arguments": [{"name": "language", "description": "Language to talk to the user in (e.g. fr, en, es). Optional: defaults to the language the user writes in.", "required": false}]
        }]})),
        "prompts/get" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name != "setup_kesal_rangeur" {
                return Err((-32602, format!("Unknown prompt: {name}")));
            }
            let lang = params.get("arguments").and_then(|a| a.get("language")).and_then(|v| v.as_str());
            Ok(json!({
                "description": "Guided first-run setup of kesal-rangeur",
                "messages": [{"role": "user", "content": {"type": "text", "text": setup_prompt(lang)}}]
            }))
        }
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}

// ---------------------------------------------------------------------------------------------
// Prompt et aide
// ---------------------------------------------------------------------------------------------

pub fn setup_prompt(lang: Option<&str>) -> String {
    let language = match lang.map(|l| l.trim()).filter(|l| !l.is_empty()) {
        Some(l) => format!("Talk to the user in this language: {l}."),
        None => "Talk to the user in the language they write in.".to_string(),
    };
    format!(
        "You are helping the user set up kesal-rangeur, an automatic media organizer: it watches an arrival folder, identifies releases (movies, series, cartoons, documentaries, shows) with TMDB and files them into the library with titles in the user's language. {language} Explain results in plain words, not raw JSON.\n\
\n\
Ground rules\n\
- Keep DRY-RUN mode ON (the default) the whole time. In dry-run nothing is copied, moved, renamed or deleted. Only disable it at the very end, after the user has reviewed the dry-run results and clearly said they want real copies. Never disable it on your own initiative, and never enable `auto_range` before that.\n\
- Secrets (TMDB key, Plex token, Jellyfin key, Home Assistant token): prefer asking the user to type them in the web UI (Settings) at the app's address, so they never appear in this chat. If the user insists on pasting one here, accept it and pass it to the `configure` tool (tmdb_key, plex_token, jellyfin_key, ha_token). Never repeat a secret back.\n\
- The web UI has no password: remind the user once to keep it on their local network.\n\
- Change only what the user asked for; `configure` is a partial update.\n\
\n\
Checklist (do it step by step, confirming each step with the user before moving on)\n\
1. Call `status`. If the app is unreachable, explain how to start it (`kesal-rangeur serve`, or `docker compose up -d`; default address http://127.0.0.1:3050, KESAL_URL if different) and stop until it runs. Read the hints it returns.\n\
2. TMDB key. It is free: create an account on themoviedb.org, then Settings > API (https://www.themoviedb.org/settings/api) > Create > Developer; either the 32-character API key or the long read-access token (starts with eyJ) works. Once saved, call `test_tmdb`.\n\
3. Titles language: ask which language the titles should be in (fr-FR, en-US, es-ES…) and set `language`.\n\
4. Arrival folder: ask where downloads arrive (Resilio Sync folder, torrent client folder…). Use `browse_folders` to find it. With Docker the path is the one seen from inside the container, not on the host. Recommend action `copy` (the original stays, instant reflink/hard-link copies on the same Btrfs/XFS volume) and keep the default settle time (30 min) unless asked otherwise. Set it with `configure` (inputs). Mode `auto` lets TMDB decide the category; a fixed category sends everything of that folder to one destination.\n\
5. Destination folders: one per category the user really uses (film, serie, dessin_anime, telerealite, documentaire, spectacle); an empty category sends such items to the manual `a_trier` queue. Use `browse_folders`. Arrival and destinations should live in ONE mount/volume for instant copies. Optionally add `film_libraries` (older movie folders, used only to detect duplicates) and `rules` (force a category for a title).\n\
6. Naming: explain the presets: keep (original names, default), filebot, plex (one folder per movie, Season 01), custom (variables {{n}} {{y}} {{ny}} {{s00e00}} {{t}}…). If the user wants renaming, show `preview_naming` results for the candidate preset and apply it with `configure` (naming).\n\
7. Optional: Plex / Jellyfin / Home Assistant notifications (URLs via `configure`, tokens typed in the web UI), then `test_services`.\n\
8. Dry-run review: call `scan`, then `plan` (filter by ranger, doublon, a_trier, attendre, ignorer). Walk the user through what the app WOULD do: destinations, renamings, duplicates. For `a_trier` items use `search_tmdb` and `choose` after the user picks the right title. Items in `attendre` are still being transferred. Fix the settings and rescan until the user is satisfied.\n\
9. Only when the user explicitly validates the results: tell them real copies will start, get a clear yes, then call `set_dry_run` with enabled=false and confirm=\"{CONFIRM_PHRASE}\". Start prudently with a single item (`run` with `nom`), check it with `history`, mention `undo`, then run the rest. Offer `scan_interval_min` / `auto_range` for hands-off operation only if they ask.\n\
\n\
Resources: kesal://help (full user guide) and kesal://settings (current settings)."
    )
}

/// Aide intégrée (HTML) convertie en markdown léger.
pub fn help_markdown() -> String {
    format!("# kesal-rangeur user guide (in French)\n\n{}", html_to_markdown(HELP_HTML))
}

pub fn html_to_markdown(html: &str) -> String {
    let mut pres: Vec<String> = Vec::new();
    let pre_re = Regex::new(r"(?s)<pre[^>]*>(.*?)</pre>").unwrap();
    let tag_re = Regex::new(r"<[^>]+>").unwrap();
    let mut s = pre_re
        .replace_all(html, |c: &regex::Captures| {
            let inner = decode_entities(&tag_re.replace_all(&c[1], ""));
            pres.push(format!("\n\n```\n{}\n```\n\n", inner.trim_matches('\n')));
            format!("\u{1}PRE{}\u{1}", pres.len() - 1)
        })
        .into_owned();
    let rules: &[(&str, &str)] = &[
        (r"(?s)<h3[^>]*>(.*?)</h3>", "\n\n## $1\n\n"),
        (r"(?s)<h4[^>]*>(.*?)</h4>", "\n\n### $1\n\n"),
        (r#"(?s)<a[^>]*href="(https?://[^"]+)"[^>]*>(.*?)</a>"#, "[$2]($1)"),
        (r"(?s)<li[^>]*>", "\n- "),
        (r"</li>", ""),
        (r"</?[uo]l[^>]*>", "\n"),
        (r"</p>", "\n\n"),
        (r"<br\s*/?>", "\n"),
        (r#"<div class="warn">"#, "\n\n> **Warning:** "),
        (r#"<div class="note">"#, "\n\n> **Note:** "),
        (r"</div>", "\n\n"),
        (r"</?(b|strong)>", "**"),
        (r"</?(i|em)>", "*"),
        (r"</?code>", "`"),
    ];
    for (re, rep) in rules {
        s = Regex::new(re).unwrap().replace_all(&s, *rep).into_owned();
    }
    s = tag_re.replace_all(&s, "").into_owned();
    s = decode_entities(&s);
    for (i, p) in pres.iter().enumerate() {
        s = s.replace(&format!("\u{1}PRE{i}\u{1}"), p);
    }
    let lines: Vec<&str> = s.lines().map(|l| l.trim_end()).collect();
    let joined = lines.join("\n");
    // Éléments de liste serrés (pas de ligne vide entre deux « - »).
    let tight = Regex::new(r"(?m)^(- .*)\n\n(- )").unwrap();
    let mut out = joined;
    for _ in 0..2 {
        out = tight.replace_all(&out, "$1\n$2").into_owned();
    }
    let multi = Regex::new(r"\n{3,}").unwrap();
    multi.replace_all(&out, "\n\n").trim().to_string() + "\n"
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ").replace("&amp;", "&")
}

// ---------------------------------------------------------------------------------------------
// Définition des outils
// ---------------------------------------------------------------------------------------------

fn tool(name: &str, description: &str, properties: Value, required: &[&str], read_only: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required, "additionalProperties": false},
        "annotations": {"readOnlyHint": read_only}
    })
}

pub fn tools_def() -> Vec<Value> {
    let cat_enum = json!(CATEGORIES);
    vec![
        tool("status", "Overview of the running kesal-rangeur: version, dry-run state, whether TMDB/Plex/Jellyfin are configured, number of arrival/destination folders, counters of the last analysis (ranger/doublon/a_trier/attendre/ignorer/fait), folders that do not exist, and hints about what is still missing. Call this first.", json!({}), &[], true),
        tool("get_settings", "Return the current settings (language, arrival folders, destination folders per category, extra film libraries, rules, naming, integrations, copy method, scan interval, dry-run flag). Secrets are masked by the app (only a 'configured' flag and the last 4 characters); they cannot be revealed.", json!({}), &[], true),
        tool(
            "configure",
            "PARTIAL settings update: only the fields you pass are changed, everything else is kept. Never changes dry-run (use set_dry_run). Semantics: `inputs` and `film_libraries` and `rules` REPLACE the whole list; `destinations` is merged per category (use an empty string to remove a category); `naming` is merged field by field (a preset name string is accepted as a shortcut). Secrets (tmdb_key, plex_token, jellyfin_key, ha_token) are optional and write-only: prefer asking the user to type them in the web UI. Do NOT set auto_range=true without the user's explicit agreement: once dry-run is off, it makes the app copy files by itself after every scan.",
            json!({
                "language": {"type": "string", "description": "Language of TMDB titles, e.g. fr-FR, en-US, es-ES."},
                "inputs": {"type": "array", "description": "Arrival folders (replaces the whole list).", "items": {"type": "object", "properties": {
                    "path": {"type": "string", "description": "Absolute folder path as seen by the app (inside the container with Docker)."},
                    "mode": {"type": "string", "description": "'auto' (TMDB decides the category) or a fixed category: film, serie, dessin_anime, telerealite, documentaire, spectacle. Default auto."},
                    "action": {"type": "string", "enum": ["copy", "move"], "description": "copy keeps the original (recommended). Default copy."},
                    "settle_minutes": {"type": "integer", "minimum": 0, "description": "Minutes without change before an item is considered fully transferred. Default 30."}
                }, "required": ["path"]}},
                "destinations": {"type": "object", "description": "Destination folder per category, merged with the existing ones. Keys: film, serie, dessin_anime, telerealite, documentaire, spectacle. Empty string removes a category.", "additionalProperties": {"type": "string"}},
                "film_libraries": {"type": "array", "items": {"type": "string"}, "description": "Extra movie folders read only to detect duplicates (replaces the list)."},
                "rules": {"type": "array", "description": "Personal rules forcing the category of a title (replaces the list).", "items": {"type": "object", "properties": {"title": {"type": "string"}, "category": {"type": "string", "enum": cat_enum}}, "required": ["title", "category"]}},
                "naming": {"description": "Naming of filed items: a preset name (keep, filebot, plex, custom) or an object {preset, movie, episode, season_folder, series_folder} merged into the current naming. Variables: {n} {y} {ny} {s} {s00} {e} {e00} {s00e00} {sxe} {t} {vf} {source} {vc} {group}; '/' creates a folder.", "oneOf": [
                    {"type": "string", "enum": ["keep", "filebot", "plex", "custom"]},
                    {"type": "object", "properties": {"preset": {"type": "string", "enum": ["keep", "filebot", "plex", "custom"]}, "movie": {"type": "string"}, "episode": {"type": "string"}, "season_folder": {"type": "string"}, "series_folder": {"type": "string"}}}
                ]},
                "plex_url": {"type": "string", "description": "e.g. http://host:32400 (empty to disable)."},
                "jellyfin_url": {"type": "string", "description": "e.g. http://host:8096 (empty to disable)."},
                "ha_url": {"type": "string", "description": "Home Assistant URL, e.g. http://host:8123 (empty to disable)."},
                "ha_service": {"type": "string", "description": "Home Assistant notify service, e.g. notify.mobile_app_phone."},
                "copy_method": {"type": "string", "enum": ["auto", "reflink", "hardlink", "copy"], "description": "auto = reflink, then hard link, then plain copy."},
                "scan_interval_min": {"type": "integer", "minimum": 0, "description": "Automatic analysis every N minutes (0 = only on demand)."},
                "auto_range": {"type": "boolean", "description": "After each scan, file the safe items automatically (ignored while dry-run is on). Ask the user first."},
                "tmdb_key": {"type": "string", "description": "TMDB API key or v4 read token (write-only secret)."},
                "plex_token": {"type": "string", "description": "Plex token (write-only secret)."},
                "jellyfin_key": {"type": "string", "description": "Jellyfin API key (write-only secret)."},
                "ha_token": {"type": "string", "description": "Home Assistant long-lived token (write-only secret)."}
            }),
            &[],
            false,
        ),
        tool(
            "set_dry_run",
            "Enable or disable dry-run mode. In dry-run nothing is copied, moved, renamed or deleted. DISABLING it makes real file operations possible (copies/moves into the user's library): you MUST first explain this to the user and obtain their explicit agreement in the conversation, and only then pass confirm exactly equal to \"I understand real copies will start\". Never disable it on your own initiative. Enabling needs no confirmation.",
            json!({
                "enabled": {"type": "boolean", "description": "true = dry-run ON (safe). false = dry-run OFF (real copies possible)."},
                "confirm": {"type": "string", "description": "Required only when enabled=false: the exact phrase \"I understand real copies will start\"."}
            }),
            &["enabled"],
            false,
        ),
        tool("test_tmdb", "Check that the saved TMDB key works (read-only network test).", json!({}), &[], true),
        tool("test_services", "Test connectivity to Plex, Jellyfin and Home Assistant using the saved URLs/tokens (read-only). Unconfigured services are reported as such.", json!({}), &[], true),
        tool("browse_folders", "List sub-folders of a directory on the machine/container where kesal-rangeur runs (to help choose arrival and destination folders), with the number of video files directly inside. Without a path, lists the root '/'.", json!({"path": {"type": "string", "description": "Absolute directory path. Default '/'."}}), &[], true),
        tool("scan", "Start an analysis of the arrival folders and wait (up to ~60 s) until it finishes, then return the counters. Read-only in dry-run (it only queries TMDB and reads folders). Follow with `plan` to see the details.", json!({}), &[], false),
        tool("plan", "Summarize the result of the last analysis: counters per decision plus a list of items (name, recognized title/year/category, decision, destination, reason). For 'a_trier' items, the top TMDB candidates are included so you can call `choose`.", json!({
            "filter": {"type": "string", "enum": ["ranger", "doublon", "a_trier", "attendre", "ignorer", "fait"], "description": "Only items with this decision: ranger = will be filed, doublon = already in library, a_trier = needs a manual choice, attendre = still being transferred, ignorer = junk, fait = already filed this session."},
            "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Max items returned (default 25)."}
        }), &[], true),
        tool("search_tmdb", "Search TMDB (movies and/or TV) in the configured language. Returns id, media, title, year and a short overview; use the id with `choose`.", json!({
            "q": {"type": "string", "description": "Title to search."},
            "kind": {"type": "string", "enum": ["all", "movie", "tv"], "description": "Default all."}
        }), &["q"], true),
        tool("choose", "Tell the app which TMDB entry an item really is (fixes a wrong or doubtful identification); the choice is remembered and the item is re-planned. Requires a previous `scan`. Does not copy anything.", json!({
            "nom": {"type": "string", "description": "The item's name exactly as shown by `plan` (the arrival folder/file name)."},
            "tmdb_id": {"type": "integer", "description": "TMDB id from `search_tmdb` or from the candidates in `plan`."},
            "media": {"type": "string", "enum": ["movie", "tv"]},
            "category": {"type": "string", "enum": cat_enum, "description": "Optional: force the category."}
        }), &["nom", "tmdb_id", "media"], false),
        tool("preview_naming", "Preview a naming configuration without saving it: shows an example movie path and an example episode path. Accepts a preset name or an object {preset, movie, episode, season_folder, series_folder}. Reports an error for unknown variables.", json!({
            "naming": {"description": "Preset name (keep, filebot, plex, custom) or an object.", "oneOf": [
                {"type": "string", "enum": ["keep", "filebot", "plex", "custom"]},
                {"type": "object", "properties": {"preset": {"type": "string", "enum": ["keep", "filebot", "plex", "custom"]}, "movie": {"type": "string"}, "episode": {"type": "string"}, "season_folder": {"type": "string"}, "series_folder": {"type": "string"}}}
            ]}
        }), &["naming"], true),
        tool("history", "Recent real filings (most recent first): id, time, name, title, category, status (fait/annule/erreur), destination, file count, size, reason on errors. Empty in dry-run.", json!({"limit": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Default 10."}}), &[], true),
        tool("undo", "Undo one filing from the history: removes what the app created in the library (and folders it created, if empty). The original in the arrival folder is untouched. Refused if a filed file was modified since.", json!({"id": {"type": "string", "description": "History entry id from `history`."}}), &["id"], false),
        tool(
            "run",
            "Actually file the items planned as 'ranger' (all of them, or only the one named). This performs REAL copies/moves into the library; the app refuses (HTTP 403) while dry-run is on. Only use after the user has reviewed the plan and agreed. Waits up to ~45 s then reports recent history.",
            json!({"nom": {"type": "string", "description": "Optional: only this item (name as shown by `plan`)."}}),
            &[],
            false,
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// Fonctions pures : fusion des réglages, résumés
// ---------------------------------------------------------------------------------------------

const BASE_KEYS: [&str; 14] = ["language", "inputs", "destinations", "film_libraries", "rules", "plex_url", "jellyfin_url", "ha_url", "ha_service", "essai", "copy_method", "scan_interval_min", "auto_range", "naming"];
const SECRET_KEYS: [&str; 4] = ["tmdb_key", "plex_token", "jellyfin_key", "ha_token"];
const PRESETS: [&str; 4] = ["keep", "filebot", "plex", "custom"];
const NAMING_KEYS: [&str; 5] = ["preset", "movie", "episode", "season_folder", "series_folder"];

fn want_str<'a>(k: &str, v: &'a Value) -> Result<&'a str, String> {
    v.as_str().ok_or_else(|| format!("'{k}' must be a string"))
}

fn want_category(k: &str, v: &Value) -> Result<String, String> {
    let s = want_str(k, v)?;
    if CATEGORIES.contains(&s) {
        Ok(s.to_string())
    } else {
        Err(format!("'{k}': unknown category '{s}' (expected one of {})", CATEGORIES.join(", ")))
    }
}

fn valid_language(s: &str) -> bool {
    let (a, b) = match s.split_once('-') {
        Some(p) => p,
        None => return false,
    };
    (2..=3).contains(&a.len()) && a.chars().all(|c| c.is_ascii_lowercase()) && b.len() == 2 && b.chars().all(|c| c.is_ascii_uppercase())
}

fn merge_naming(current: &Value, patch: &Value) -> Result<Value, String> {
    let mut obj = current.as_object().cloned().unwrap_or_default();
    let patch_obj: Map<String, Value> = match patch {
        Value::String(p) => {
            let mut m = Map::new();
            m.insert("preset".into(), Value::String(p.clone()));
            m
        }
        Value::Object(m) => m.clone(),
        _ => return Err("'naming' must be a preset name or an object".into()),
    };
    for (k, v) in patch_obj {
        if !NAMING_KEYS.contains(&k.as_str()) {
            return Err(format!("naming: unknown field '{k}' (expected {})", NAMING_KEYS.join(", ")));
        }
        let s = want_str(&format!("naming.{k}"), &v)?;
        if k == "preset" && !PRESETS.contains(&s) {
            return Err(format!("naming.preset: unknown preset '{s}' (expected {})", PRESETS.join(", ")));
        }
        obj.insert(k, Value::String(s.to_string()));
    }
    Ok(Value::Object(obj))
}

/// Fusionne un changement partiel dans les réglages actuels (réponse de GET /api/settings) et renvoie
/// le corps complet à envoyer en POST + la liste des champs modifiés. `essai` est TOUJOURS repris tel quel.
pub fn merge_settings(current: &Value, patch: &Map<String, Value>) -> Result<(Value, Vec<String>), String> {
    let cur = current.as_object().ok_or("unexpected settings answer from the app")?;
    let mut body = Map::new();
    for k in BASE_KEYS {
        let v = cur.get(k).cloned().ok_or_else(|| format!("unexpected settings answer from the app: missing '{k}'"))?;
        body.insert(k.to_string(), v);
    }
    let mut changed: Vec<String> = Vec::new();
    for (k, v) in patch {
        if v.is_null() {
            continue;
        }
        match k.as_str() {
            "essai" | "dry_run" => return Err("configure never changes dry-run mode: use the set_dry_run tool".into()),
            "language" => {
                let s = want_str(k, v)?.trim();
                if !valid_language(s) {
                    return Err(format!("'language' must look like fr-FR, en-US, es-ES (got '{s}')"));
                }
                body.insert(k.clone(), json!(s));
            }
            "inputs" => {
                let arr = v.as_array().ok_or("'inputs' must be an array of objects")?;
                let mut out = Vec::new();
                for (i, it) in arr.iter().enumerate() {
                    let o = it.as_object().ok_or_else(|| format!("inputs[{i}] must be an object"))?;
                    for key in o.keys() {
                        if !["path", "mode", "action", "settle_minutes"].contains(&key.as_str()) {
                            return Err(format!("inputs[{i}]: unknown field '{key}'"));
                        }
                    }
                    let path = want_str(&format!("inputs[{i}].path"), o.get("path").ok_or_else(|| format!("inputs[{i}].path is required"))?)?.trim();
                    if path.is_empty() {
                        return Err(format!("inputs[{i}].path is empty"));
                    }
                    let mode = match o.get("mode") {
                        None | Some(Value::Null) => "auto".to_string(),
                        Some(m) => {
                            let m = want_str(&format!("inputs[{i}].mode"), m)?;
                            if m == "auto" { m.to_string() } else { want_category(&format!("inputs[{i}].mode"), &json!(m))? }
                        }
                    };
                    let action = match o.get("action") {
                        None | Some(Value::Null) => "copy".to_string(),
                        Some(a) => match want_str(&format!("inputs[{i}].action"), a)? {
                            x @ ("copy" | "move") => x.to_string(),
                            x => return Err(format!("inputs[{i}].action must be 'copy' or 'move' (got '{x}')")),
                        },
                    };
                    let settle = match o.get("settle_minutes") {
                        None | Some(Value::Null) => 30,
                        Some(s) => s.as_u64().ok_or_else(|| format!("inputs[{i}].settle_minutes must be a non-negative integer"))?,
                    };
                    out.push(json!({"path": path, "mode": mode, "action": action, "settle_minutes": settle}));
                }
                body.insert(k.clone(), Value::Array(out));
            }
            "destinations" => {
                let o = v.as_object().ok_or("'destinations' must be an object {category: path}")?;
                let mut dest = body.get("destinations").and_then(|d| d.as_object()).cloned().unwrap_or_default();
                for (cat, p) in o {
                    want_category("destinations key", &json!(cat))?;
                    let p = want_str(&format!("destinations.{cat}"), p)?.trim();
                    if p.is_empty() {
                        dest.remove(cat);
                    } else {
                        dest.insert(cat.clone(), json!(p));
                    }
                }
                body.insert(k.clone(), Value::Object(dest));
            }
            "film_libraries" => {
                let arr = v.as_array().ok_or("'film_libraries' must be an array of paths")?;
                let mut out = Vec::new();
                for (i, p) in arr.iter().enumerate() {
                    let p = want_str(&format!("film_libraries[{i}]"), p)?.trim();
                    if !p.is_empty() {
                        out.push(json!(p));
                    }
                }
                body.insert(k.clone(), Value::Array(out));
            }
            "rules" => {
                let arr = v.as_array().ok_or("'rules' must be an array of {title, category}")?;
                let mut out = Vec::new();
                for (i, r) in arr.iter().enumerate() {
                    let o = r.as_object().ok_or_else(|| format!("rules[{i}] must be an object"))?;
                    let title = want_str(&format!("rules[{i}].title"), o.get("title").ok_or_else(|| format!("rules[{i}].title is required"))?)?.trim();
                    let cat = want_category(&format!("rules[{i}].category"), o.get("category").ok_or_else(|| format!("rules[{i}].category is required"))?)?;
                    if title.is_empty() {
                        return Err(format!("rules[{i}].title is empty"));
                    }
                    out.push(json!({"title": title, "category": cat}));
                }
                body.insert(k.clone(), Value::Array(out));
            }
            "plex_url" | "jellyfin_url" | "ha_url" | "ha_service" => {
                body.insert(k.clone(), json!(want_str(k, v)?.trim()));
            }
            "copy_method" => {
                let s = want_str(k, v)?;
                if !["auto", "reflink", "hardlink", "copy"].contains(&s) {
                    return Err(format!("'copy_method' must be auto, reflink, hardlink or copy (got '{s}')"));
                }
                body.insert(k.clone(), json!(s));
            }
            "scan_interval_min" => {
                let n = v.as_u64().ok_or("'scan_interval_min' must be a non-negative integer")?;
                body.insert(k.clone(), json!(n));
            }
            "auto_range" => {
                let b = v.as_bool().ok_or("'auto_range' must be a boolean")?;
                body.insert(k.clone(), json!(b));
            }
            "naming" => {
                let merged = merge_naming(body.get("naming").unwrap_or(&Value::Null), v)?;
                body.insert(k.clone(), merged);
            }
            s if SECRET_KEYS.contains(&s) => {
                let secret = want_str(k, v)?.trim();
                if secret.is_empty() {
                    continue; // vide = inchangé côté serveur
                }
                body.insert(k.clone(), json!(secret));
            }
            other => return Err(format!("unknown field '{other}'. Allowed: language, inputs, destinations, film_libraries, rules, naming, plex_url, jellyfin_url, ha_url, ha_service, copy_method, scan_interval_min, auto_range, tmdb_key, plex_token, jellyfin_key, ha_token")),
        }
        changed.push(k.clone());
    }
    // L'essai à blanc n'est JAMAIS modifié ici.
    body.insert("essai".into(), cur["essai"].clone());
    Ok((Value::Object(body), changed))
}

/// Corps complet des réglages actuels avec le seul drapeau `essai` modifié.
pub fn with_dry_run(current: &Value, enabled: bool) -> Result<Value, String> {
    let (mut body, _) = merge_settings(current, &Map::new())?;
    body["essai"] = json!(enabled);
    Ok(body)
}

/// Vérifie la phrase de confirmation pour désactiver l'essai à blanc.
pub fn check_confirmation(enabled: bool, confirm: Option<&str>) -> Result<(), String> {
    if enabled || confirm == Some(CONFIRM_PHRASE) {
        return Ok(());
    }
    Err(format!(
        "Refused: disabling dry-run allows REAL copies/moves into the user's library. Explain this to the user, obtain their explicit agreement in the conversation, then call set_dry_run again with enabled=false and confirm=\"{CONFIRM_PHRASE}\" (exact text). Dry-run stays ON."
    ))
}

pub fn iso8601(t: u64) -> String {
    let days = (t / 86400) as i64;
    let secs = t % 86400;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

fn short(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n { format!("{t}…") } else { t }
}

/// Résumé concis d'un élément du plan (réponse de /api/plan ou /api/choice).
pub fn summarize_item(it: &Value) -> Value {
    let mut o = Map::new();
    o.insert("name".into(), it["nom"].clone());
    if let Some(r) = it.get("reconnu").filter(|r| r.is_object()) {
        o.insert("recognized".into(), json!({"title": r["titre"], "year": r["annee"], "category": r["categorie"], "media": r["media"], "tmdb_id": r["tmdb_id"]}));
    }
    let d = &it["decision"];
    let kind = d["decision"].as_str().unwrap_or("?");
    o.insert("decision".into(), json!(kind));
    match kind {
        "ranger" => {
            o.insert("destination".into(), d["vers"].clone());
            if d["nouveau_dossier"].as_bool() == Some(true) {
                o.insert("new_folder".into(), json!(true));
            }
            if let Some(r) = d["renumerotation"].as_array().filter(|a| a.len() == 2) {
                o.insert("renumbering".into(), json!(format!("S{:02} -> S{:02}", r[0].as_u64().unwrap_or(0), r[1].as_u64().unwrap_or(0))));
            }
            if let Some(files) = d["fichiers"].as_array().filter(|f| !f.is_empty()) {
                o.insert("files".into(), json!(files.len()));
                let f = &files[0];
                let from = f["src"].as_str().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
                let dst = f["dst"].as_str().unwrap_or("");
                let rel = d["vers"].as_str().and_then(|v| dst.strip_prefix(v)).map(|s| s.trim_start_matches('/')).filter(|s| !s.is_empty()).unwrap_or(dst);
                o.insert("example".into(), json!(format!("{} -> {}", short(&from, 70), short(rel, 90))));
            }
            if let Some(n) = d["renommages"].as_array().filter(|r| !r.is_empty()) {
                o.insert("renamed_files".into(), json!(n.len()));
            }
        }
        "doublon" => {
            o.insert("existing".into(), d["existant"].clone());
            o.insert("reason".into(), d["raison"].clone());
        }
        "a_trier" => {
            o.insert("reason".into(), d["raison"].clone());
            let cands: Vec<Value> = it["candidats"]
                .as_array()
                .map(|c| c.iter().take(3).map(|c| json!({"tmdb_id": c["id"], "media": c["media"], "title": c["title"], "year": c["year"]})).collect())
                .unwrap_or_default();
            if !cands.is_empty() {
                o.insert("candidates".into(), Value::Array(cands));
            }
        }
        "attendre" | "ignorer" => {
            o.insert("reason".into(), d["raison"].clone());
        }
        "fait" => {
            o.insert("destination".into(), d["vers"].clone());
        }
        _ => {}
    }
    Value::Object(o)
}

/// Suggestions « prochaine étape » selon la réponse de /api/status.
pub fn status_hints(s: &Value, missing: &Value) -> Vec<String> {
    let mut h = Vec::new();
    if s["tmdb_ok"].as_bool() != Some(true) {
        h.push("TMDB key missing or invalid: get a free key at https://www.themoviedb.org/settings/api and enter it in the web UI (Settings), then call test_tmdb.".to_string());
    }
    if s["entrees"].as_u64() == Some(0) {
        h.push("No arrival folder configured: use browse_folders then configure(inputs).".to_string());
    }
    if s["destinations"].as_u64() == Some(0) {
        h.push("No destination folder configured: use configure(destinations).".to_string());
    }
    if missing.as_array().map_or(false, |m| !m.is_empty()) {
        h.push("Some configured folders do not exist (see missing_folders): check the paths (with Docker, use paths as seen inside the container).".to_string());
    }
    if s["essai"].as_bool() == Some(true) {
        h.push("Dry-run is ON: nothing will be written. Keep it that way until the user has reviewed the plan.".to_string());
    } else {
        h.push("Dry-run is OFF: real copies are possible.".to_string());
    }
    h
}

// ---------------------------------------------------------------------------------------------
// Exécution des outils
// ---------------------------------------------------------------------------------------------

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn arg_str<'a>(args: &'a Map<String, Value>, k: &str) -> Result<Option<&'a str>, String> {
    match args.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("'{k}' must be a string")),
    }
}

fn need_str<'a>(args: &'a Map<String, Value>, k: &str) -> Result<&'a str, String> {
    match arg_str(args, k)? {
        Some(s) if !s.trim().is_empty() => Ok(s),
        _ => Err(format!("'{k}' is required")),
    }
}

fn arg_limit(args: &Map<String, Value>, default: u64, max: u64) -> Result<usize, String> {
    match args.get("limit") {
        None | Some(Value::Null) => Ok(default as usize),
        Some(v) => v.as_u64().map(|n| n.clamp(1, max) as usize).ok_or_else(|| "'limit' must be a positive integer".to_string()),
    }
}

pub async fn call_tool(api: &Api, name: &str, args: &Map<String, Value>) -> Result<String, String> {
    eprintln!("[mcp] outil : {name}"); // jamais les arguments (ils peuvent contenir des secrets)
    match name {
        "status" => t_status(api).await,
        "get_settings" => {
            let mut s = api.get("/api/settings", &[]).await?;
            if let Some(o) = s.as_object_mut() {
                o.remove("categories");
                if let Some(r) = o.get("rules").and_then(|r| r.as_array()).filter(|r| r.len() > 40).cloned() {
                    o.insert("rules".into(), json!(r[..40]));
                    o.insert("rules_note".into(), json!(format!("{} rules in total, first 40 shown", r.len())));
                }
            }
            Ok(pretty(&s))
        }
        "configure" => t_configure(api, args).await,
        "set_dry_run" => {
            let enabled = args.get("enabled").and_then(|v| v.as_bool()).ok_or("'enabled' (boolean) is required")?;
            check_confirmation(enabled, arg_str(args, "confirm")?)?;
            let cur = api.get("/api/settings", &[]).await?;
            let body = with_dry_run(&cur, enabled)?;
            api.post("/api/settings", &body).await?;
            Ok(if enabled {
                "Dry-run is now ON: nothing will be copied, moved, renamed or deleted.".into()
            } else {
                "Dry-run is now OFF: real copies/moves are possible (`run`, or automatically if auto_range is on). Every filing can be undone with `undo`.".into()
            })
        }
        "test_tmdb" => Ok(pretty(&api.post("/api/tmdb/test", &json!({})).await?)),
        "test_services" => Ok(pretty(&api.post("/api/services/test", &json!({})).await?)),
        "browse_folders" => t_browse(api, arg_str(args, "path")?).await,
        "scan" => t_scan(api).await,
        "plan" => t_plan(api, args).await,
        "search_tmdb" => {
            let q = need_str(args, "q")?;
            let kind = arg_str(args, "kind")?.unwrap_or("all");
            if !["all", "movie", "tv"].contains(&kind) {
                return Err("'kind' must be all, movie or tv".into());
            }
            let r = api.get("/api/tmdb/search", &[("q", q), ("kind", kind)]).await?;
            let list: Vec<Value> = r
                .as_array()
                .map(|a| a.iter().take(8).map(|c| json!({"tmdb_id": c["id"], "media": c["media"], "title": c["title"], "original_title": c["original_title"], "year": c["year"], "overview": short(c["overview"].as_str().unwrap_or(""), 140)})).collect())
                .unwrap_or_default();
            Ok(pretty(&json!({"results": list})))
        }
        "choose" => {
            let nom = need_str(args, "nom")?;
            let id = args.get("tmdb_id").and_then(|v| v.as_u64()).ok_or("'tmdb_id' (integer) is required")?;
            let media = need_str(args, "media")?;
            if !["movie", "tv"].contains(&media) {
                return Err("'media' must be movie or tv".into());
            }
            let mut body = json!({"nom": nom, "tmdb_id": id, "media": media});
            if let Some(c) = arg_str(args, "category")? {
                body["category"] = json!(want_category("category", &json!(c))?);
            }
            let r = api.post("/api/choice", &body).await?;
            Ok(pretty(&json!({"choice_saved": true, "item": summarize_item(&r["item"])})))
        }
        "preview_naming" => {
            let n = args.get("naming").ok_or("'naming' is required")?;
            let naming = match n {
                Value::String(_) | Value::Object(_) => merge_naming(&json!({}), n)?,
                _ => return Err("'naming' must be a preset name or an object".into()),
            };
            let r = api.post("/api/naming/preview", &json!({"naming": naming})).await?;
            if r["ok"].as_bool() == Some(true) {
                if r["garde"].as_bool() == Some(true) {
                    Ok("Preset 'keep': original names are kept, nothing is renamed.".into())
                } else {
                    Ok(pretty(&json!({"movie_example": r["film"], "series_example": r["serie"]})))
                }
            } else {
                Err(format!("Invalid naming: {}", r["erreur"].as_str().unwrap_or("unknown error")))
            }
        }
        "history" => {
            let limit = arg_limit(args, 10, 50)?;
            let r = api.get("/api/history", &[]).await?;
            Ok(pretty(&summarize_history(&r, limit)))
        }
        "undo" => {
            let id = need_str(args, "id")?;
            api.post("/api/undo", &json!({"id": id})).await?;
            Ok(format!("Undone: history entry {id}. The original in the arrival folder was not touched."))
        }
        "run" => t_run(api, arg_str(args, "nom")?).await,
        _ => Err(format!("Unknown tool: {name}")),
    }
}

pub fn summarize_history(r: &Value, limit: usize) -> Value {
    let all = r["entrees"].as_array().cloned().unwrap_or_default();
    let list: Vec<Value> = all
        .iter()
        .take(limit)
        .map(|e| {
            let mut o = Map::new();
            o.insert("id".into(), e["id"].clone());
            o.insert("time".into(), json!(e["t"].as_u64().map(iso8601)));
            o.insert("name".into(), e["nom"].clone());
            o.insert("title".into(), e["titre"].clone());
            o.insert("category".into(), e["categorie"].clone());
            o.insert("status".into(), e["statut"].clone());
            o.insert("destination".into(), e["vers"].clone());
            o.insert("files".into(), json!(e["fichiers"].as_array().map_or(0, |f| f.len())));
            o.insert("size_mb".into(), json!(e["taille_totale"].as_u64().unwrap_or(0) / 1_000_000));
            if e["statut"] != "fait" && !e["motif"].as_str().unwrap_or("").is_empty() {
                o.insert("reason".into(), e["motif"].clone());
            }
            Value::Object(o)
        })
        .collect();
    json!({"total": all.len(), "shown": list.len(), "entries": list})
}

async fn t_status(api: &Api) -> Result<String, String> {
    let s = api.get("/api/status", &[]).await?;
    let missing = api.get("/api/settings", &[]).await.ok().and_then(|v| v.get("dossiers_introuvables").cloned()).unwrap_or(json!([]));
    let hints = status_hints(&s, &missing);
    Ok(pretty(&json!({
        "version": s["version"],
        "dry_run": s["essai"],
        "scanning": s["scanning"],
        "last_scan": s["dernier_scan"].as_u64().map(iso8601),
        "last_scan_error": s["erreur"],
        "tmdb_configured": s["tmdb_ok"],
        "plex_configured": s["plex"],
        "jellyfin_configured": s["jellyfin"],
        "language": s["langue"],
        "arrival_folders": s["entrees"],
        "destination_folders": s["destinations"],
        "counters": s["compteurs"],
        "total_items": s["total"],
        "copies_in_progress": s["en_cours"],
        "known_movies": s["films_connus"],
        "remembered_choices": s["choix_retenus"],
        "missing_folders": missing,
        "hints": hints,
    })))
}

async fn t_configure(api: &Api, args: &Map<String, Value>) -> Result<String, String> {
    if args.values().all(|v| v.is_null()) {
        return Err("No field provided: pass at least one setting to change (see the tool schema).".into());
    }
    let cur = api.get("/api/settings", &[]).await?;
    let (body, changed) = merge_settings(&cur, args)?;
    let r = api.post("/api/settings", &body).await?;
    let after = api.get("/api/settings", &[]).await.ok();
    let mut out = Map::new();
    out.insert("saved".into(), json!(true));
    out.insert("changed".into(), json!(changed));
    out.insert("dry_run".into(), body["essai"].clone());
    out.insert("tmdb_configured".into(), r["tmdb"].clone());
    if let Some(a) = &after {
        out.insert("missing_folders".into(), a["dossiers_introuvables"].clone());
    }
    let mut warnings = Vec::new();
    if args.get("auto_range").and_then(|v| v.as_bool()) == Some(true) {
        warnings.push(if body["essai"] == json!(true) { "auto_range is on but dry-run is ON: nothing will be filed automatically until dry-run is disabled." } else { "auto_range is on and dry-run is OFF: the app will now file safe items by itself after each scan." });
    }
    if args.get("inputs").is_some() && args.get("destinations").is_none() && cur["destinations"].as_object().map_or(true, |d| d.is_empty()) {
        warnings.push("No destination folder is configured yet.");
    }
    if !warnings.is_empty() {
        out.insert("warnings".into(), json!(warnings));
    }
    Ok(pretty(&Value::Object(out)))
}

async fn t_browse(api: &Api, path: Option<&str>) -> Result<String, String> {
    let wanted = path.filter(|p| !p.trim().is_empty()).unwrap_or("/");
    let r = api.get("/api/fs", &[("path", wanted)]).await?;
    let dirs: Vec<Value> = r["dirs"].as_array().cloned().unwrap_or_default();
    let shown: Vec<Value> = dirs.iter().take(100).cloned().collect();
    let mut o = json!({"path": r["path"], "parent": r["parent"], "video_files_here": r["videos"], "sub_folders": shown});
    if dirs.len() > 100 {
        o["note"] = json!(format!("{} sub-folders, first 100 shown", dirs.len()));
    }
    if r["path"].as_str().map(|p| p.trim_end_matches('/')) != Some(wanted.trim_end_matches('/')) && wanted != "/" {
        o["warning"] = json!(format!("'{wanted}' is not a directory visible to the app, showing '/' instead. With Docker only paths mounted in the container exist."));
    }
    Ok(pretty(&o))
}

async fn t_scan(api: &Api) -> Result<String, String> {
    let started = Instant::now();
    api.post("/api/scan", &json!({})).await?;
    loop {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let s = api.get("/api/status", &[]).await?;
        if s["scanning"].as_bool() != Some(true) {
            if let Some(e) = s["erreur"].as_str() {
                return Err(format!("Scan failed: {e}"));
            }
            return Ok(pretty(&json!({
                "scan": "finished",
                "seconds": started.elapsed().as_secs(),
                "dry_run": s["essai"],
                "counters": s["compteurs"],
                "total_items": s["total"],
                "next": "Call `plan` (optionally with a filter) to see the details."
            })));
        }
        if started.elapsed() > Duration::from_secs(60) {
            return Ok("The scan is still running after 60 s (large folders or slow TMDB). Call `status` in a moment: when scanning=false, use `plan`.".into());
        }
    }
}

async fn t_plan(api: &Api, args: &Map<String, Value>) -> Result<String, String> {
    let filter = arg_str(args, "filter")?;
    if let Some(f) = filter {
        if !["ranger", "doublon", "a_trier", "attendre", "ignorer", "fait"].contains(&f) {
            return Err("'filter' must be one of ranger, doublon, a_trier, attendre, ignorer, fait".into());
        }
    }
    let limit = arg_limit(args, 25, 100)?;
    let r = api.get("/api/plan", &[]).await?;
    let items = r["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return Ok("The plan is empty: no analysis has run yet (call `scan`) or the arrival folders contain nothing.".into());
    }
    let mut counts: std::collections::BTreeMap<String, u64> = Default::default();
    for it in &items {
        *counts.entry(it["decision"]["decision"].as_str().unwrap_or("?").to_string()).or_default() += 1;
    }
    let selected: Vec<&Value> = items.iter().filter(|it| filter.map_or(true, |f| it["decision"]["decision"] == f)).collect();
    let list: Vec<Value> = selected.iter().take(limit).map(|it| summarize_item(it)).collect();
    let mut o = json!({"total": items.len(), "counts": counts, "filter": filter, "matching": selected.len(), "shown": list.len(), "items": list});
    if selected.len() > list.len() {
        o["note"] = json!("List truncated: use `filter` and/or a higher `limit` to see more.");
    }
    Ok(pretty(&o))
}

async fn t_run(api: &Api, nom: Option<&str>) -> Result<String, String> {
    let body = match nom {
        Some(n) => json!({"nom": n}),
        None => json!({"nom": null}),
    };
    match api.post("/api/run", &body).await {
        Ok(_) => {}
        Err(ApiError::Http(403, m)) => {
            return Err(format!("Refused by the app (HTTP 403): {}\nDry-run mode is ON, so nothing was filed. If the user really wants real copies, get their explicit agreement and use set_dry_run(enabled=false, confirm=...).", m.trim()));
        }
        Err(e) => return Err(e.to_string()),
    }
    let started = Instant::now();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let mut still = false;
    loop {
        let s = api.get("/api/status", &[]).await?;
        if s["en_cours"].as_u64().unwrap_or(0) == 0 {
            break;
        }
        if started.elapsed() > Duration::from_secs(45) {
            still = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let h = api.get("/api/history", &[]).await?;
    Ok(pretty(&json!({
        "started": true,
        "still_running": still,
        "note": if still { "Copies are still in progress; check `history` and `status` later." } else { "Finished (or nothing to do). Recent history below; use `undo` with an id to revert." },
        "recent_history": summarize_history(&h, 8)
    })))
}

// ---------------------------------------------------------------------------------------------
// Boucle stdio
// ---------------------------------------------------------------------------------------------

pub async fn run() -> anyhow::Result<()> {
    let api = Api::from_env();
    eprintln!("[mcp] kesal-rangeur {} : serveur MCP sur stdio, application visée : {}", env!("CARGO_PKG_VERSION"), api.base);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(l) => {
                    if tx.blocking_send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    while let Some(line) = rx.recv().await {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(out) = handle_line(&api, &line).await {
            use std::io::Write;
            let mut so = std::io::stdout().lock();
            let _ = writeln!(so, "{out}");
            let _ = so.flush();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Tests (sans réseau)
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn offline() -> Api {
        Api::new("http://127.0.0.1:1")
    }

    async fn rpc(line: &str) -> Value {
        serde_json::from_str(&handle_line(&offline(), line).await.expect("une réponse était attendue")).unwrap()
    }

    fn settings() -> Value {
        json!({
            "language": "fr-FR",
            "inputs": [{"path": "/in", "mode": "auto", "action": "copy", "settle_minutes": 30}],
            "destinations": {"film": "/films", "serie": "/series"},
            "film_libraries": ["/old"],
            "rules": [{"title": "Pekin Express", "category": "telerealite"}],
            "plex_url": "", "jellyfin_url": "", "ha_url": "", "ha_service": "",
            "naming": {"preset": "keep", "movie": "{n} ({y})/{n} ({y})", "episode": "{n} - {s00e00} - {t}", "season_folder": "auto", "series_folder": "{n} ({y})"},
            "essai": true, "copy_method": "auto", "scan_interval_min": 0, "auto_range": false,
            "secrets": {"tmdb": {"ok": true, "fin": "abcd"}}, "dossiers_introuvables": [], "categories": []
        })
    }

    fn patch(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[tokio::test]
    async fn parse_error() {
        let r = rpc("{pas du json").await;
        assert_eq!(r["error"]["code"], -32700);
        assert_eq!(r["id"], Value::Null);
    }

    #[tokio::test]
    async fn invalid_request() {
        assert_eq!(rpc("42").await["error"]["code"], -32600);
        assert_eq!(rpc(r#"{"jsonrpc":"2.0","id":1}"#).await["error"]["code"], -32600);
        assert_eq!(rpc("[]").await["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn initialize_negotiates_version() {
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#).await;
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(r["result"]["serverInfo"]["name"], "kesal-rangeur");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert!(r["result"]["capabilities"]["resources"].is_object());
        assert!(r["result"]["capabilities"]["prompts"].is_object());
        let r = rpc(r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#).await;
        assert_eq!(r["id"], "a");
        assert_eq!(r["result"]["protocolVersion"], PROTOCOLS[0]);
    }

    #[tokio::test]
    async fn notifications_get_no_answer() {
        assert!(handle_line(&offline(), r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).await.is_none());
        assert!(handle_line(&offline(), r#"{"jsonrpc":"2.0","method":"truc/inconnu"}"#).await.is_none());
        assert!(handle_line(&offline(), r#"{"jsonrpc":"2.0","id":3,"result":{}}"#).await.is_none());
    }

    #[tokio::test]
    async fn ping_and_unknown_method() {
        assert_eq!(rpc(r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#).await["result"], json!({}));
        let r = rpc(r#"{"jsonrpc":"2.0","id":8,"method":"sampling/createMessage"}"#).await;
        assert_eq!(r["error"]["code"], -32601);
        assert_eq!(r["id"], 8);
    }

    #[tokio::test]
    async fn batch_skips_notifications() {
        let out = handle_line(&offline(), r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"}]"#).await.unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn tools_list_is_complete_and_valid() {
        let r = rpc(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).await;
        let tools = r["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for n in ["status", "get_settings", "configure", "set_dry_run", "test_tmdb", "test_services", "browse_folders", "scan", "plan", "search_tmdb", "choose", "preview_naming", "history", "undo", "run"] {
            assert!(names.contains(&n), "outil manquant : {n}");
        }
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(t["description"].as_str().unwrap().len() > 20);
            for req in t["inputSchema"]["required"].as_array().unwrap() {
                assert!(t["inputSchema"]["properties"].get(req.as_str().unwrap()).is_some());
            }
        }
        let sd = tools.iter().find(|t| t["name"] == "set_dry_run").unwrap();
        assert!(sd["description"].as_str().unwrap().contains(CONFIRM_PHRASE));
        // aucun outil ne permet de passer `essai` via configure
        let cfg = tools.iter().find(|t| t["name"] == "configure").unwrap();
        assert!(cfg["inputSchema"]["properties"].get("essai").is_none());
    }

    #[tokio::test]
    async fn tools_call_errors() {
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope"}}"#).await;
        assert_eq!(r["error"]["code"], -32602);
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#).await;
        assert_eq!(r["error"]["code"], -32602);
        // application injoignable : résultat isError avec explication, pas une erreur de protocole
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"status","arguments":{}}}"#).await;
        assert_eq!(r["result"]["isError"], true);
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("kesal-rangeur serve") && text.contains("KESAL_URL"), "{text}");
    }

    #[tokio::test]
    async fn set_dry_run_refuses_without_phrase_before_any_network() {
        // Le refus doit venir de la vérification locale (et non d'une erreur réseau).
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"set_dry_run","arguments":{"enabled":false}}}"#).await;
        assert_eq!(r["result"]["isError"], true);
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Refused") && text.contains(CONFIRM_PHRASE), "{text}");
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"set_dry_run","arguments":{"enabled":false,"confirm":"yes"}}}"#).await;
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("Refused"));
    }

    #[tokio::test]
    async fn resources_and_prompts() {
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#).await;
        let uris: Vec<&str> = r["result"]["resources"].as_array().unwrap().iter().map(|x| x["uri"].as_str().unwrap()).collect();
        assert_eq!(uris, vec!["kesal://help", "kesal://settings"]);
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"kesal://help"}}"#).await;
        let text = r["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(text.contains("## ") && text.contains("TMDB") && !text.contains("<h3"));
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"kesal://nope"}}"#).await;
        assert_eq!(r["error"]["code"], -32002);
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"prompts/list"}"#).await;
        assert_eq!(r["result"]["prompts"][0]["name"], "setup_kesal_rangeur");
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"prompts/get","params":{"name":"setup_kesal_rangeur","arguments":{"language":"fr"}}}"#).await;
        let t = r["result"]["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(t.contains("DRY-RUN") && t.contains(CONFIRM_PHRASE) && t.contains("themoviedb.org") && t.contains("language: fr"));
        let r = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"prompts/get","params":{"name":"x"}}"#).await;
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn merge_changes_only_given_fields() {
        let (body, changed) = merge_settings(&settings(), &patch(json!({"language": "en-US", "scan_interval_min": 60}))).unwrap();
        assert_eq!(changed, vec!["language", "scan_interval_min"]);
        assert_eq!(body["language"], "en-US");
        assert_eq!(body["scan_interval_min"], 60);
        assert_eq!(body["inputs"], settings()["inputs"]);
        assert_eq!(body["destinations"], settings()["destinations"]);
        assert_eq!(body["naming"], settings()["naming"]);
        // le corps ne contient que les champs attendus par POST /api/settings
        assert!(body.get("secrets").is_none() && body.get("categories").is_none() && body.get("dossiers_introuvables").is_none());
    }

    #[test]
    fn merge_never_changes_dry_run() {
        for cur_essai in [true, false] {
            let mut s = settings();
            s["essai"] = json!(cur_essai);
            let (body, _) = merge_settings(&s, &patch(json!({"auto_range": true}))).unwrap();
            assert_eq!(body["essai"], json!(cur_essai));
            assert!(merge_settings(&s, &patch(json!({"essai": false}))).is_err());
            assert!(merge_settings(&s, &patch(json!({"dry_run": false}))).is_err());
        }
    }

    #[test]
    fn dry_run_body_flips_only_the_flag() {
        let b = with_dry_run(&settings(), false).unwrap();
        assert_eq!(b["essai"], false);
        let (same, _) = merge_settings(&settings(), &Map::new()).unwrap();
        let mut b2 = b.clone();
        b2["essai"] = json!(true);
        assert_eq!(b2, same);
    }

    #[test]
    fn merge_destinations_per_category() {
        let (body, _) = merge_settings(&settings(), &patch(json!({"destinations": {"documentaire": "/docs", "serie": ""}}))).unwrap();
        assert_eq!(body["destinations"], json!({"film": "/films", "documentaire": "/docs"}));
        assert!(merge_settings(&settings(), &patch(json!({"destinations": {"musique": "/m"}}))).is_err());
    }

    #[test]
    fn merge_naming_shortcut_and_fields() {
        let (body, _) = merge_settings(&settings(), &patch(json!({"naming": "plex"}))).unwrap();
        assert_eq!(body["naming"]["preset"], "plex");
        assert_eq!(body["naming"]["movie"], "{n} ({y})/{n} ({y})");
        let (body, _) = merge_settings(&settings(), &patch(json!({"naming": {"preset": "custom", "movie": "{ny}"}}))).unwrap();
        assert_eq!(body["naming"]["preset"], "custom");
        assert_eq!(body["naming"]["movie"], "{ny}");
        assert_eq!(body["naming"]["episode"], "{n} - {s00e00} - {t}");
        assert!(merge_settings(&settings(), &patch(json!({"naming": "turbo"}))).is_err());
        assert!(merge_settings(&settings(), &patch(json!({"naming": {"film": "x"}}))).is_err());
    }

    #[test]
    fn merge_inputs_validated_and_defaulted() {
        let (body, _) = merge_settings(&settings(), &patch(json!({"inputs": [{"path": " /a "}, {"path": "/b", "mode": "serie", "action": "move", "settle_minutes": 5}]}))).unwrap();
        assert_eq!(body["inputs"][0], json!({"path": "/a", "mode": "auto", "action": "copy", "settle_minutes": 30}));
        assert_eq!(body["inputs"][1], json!({"path": "/b", "mode": "serie", "action": "move", "settle_minutes": 5}));
        for bad in [json!([{"mode": "film"}]), json!([{"path": "/a", "mode": "musique"}]), json!([{"path": "/a", "action": "delete"}]), json!([{"path": "/a", "settle_minutes": -1}]), json!([{"path": "/a", "x": 1}]), json!("/a")] {
            assert!(merge_settings(&settings(), &patch(json!({"inputs": bad}))).is_err());
        }
    }

    #[test]
    fn merge_rules_libraries_and_scalars() {
        let (body, _) = merge_settings(&settings(), &patch(json!({"rules": [{"title": "Allen", "category": "spectacle"}], "film_libraries": ["/x", ""], "copy_method": "hardlink", "plex_url": " http://p:32400 "}))).unwrap();
        assert_eq!(body["rules"], json!([{"title": "Allen", "category": "spectacle"}]));
        assert_eq!(body["film_libraries"], json!(["/x"]));
        assert_eq!(body["copy_method"], "hardlink");
        assert_eq!(body["plex_url"], "http://p:32400");
        for bad in [json!({"language": "français"}), json!({"copy_method": "zip"}), json!({"scan_interval_min": "5"}), json!({"auto_range": "yes"}), json!({"rules": [{"title": "A", "category": "x"}]}), json!({"inconnu": 1})] {
            assert!(merge_settings(&settings(), &patch(bad)).is_err());
        }
    }

    #[test]
    fn merge_secrets_only_when_given() {
        let (body, changed) = merge_settings(&settings(), &patch(json!({"tmdb_key": "  k123  ", "plex_token": ""}))).unwrap();
        assert_eq!(body["tmdb_key"], "k123");
        assert!(body.get("plex_token").is_none());
        assert_eq!(changed, vec!["tmdb_key"]);
        let (body, _) = merge_settings(&settings(), &patch(json!({"language": "fr-FR"}))).unwrap();
        assert!(body.get("tmdb_key").is_none());
    }

    #[test]
    fn null_values_are_ignored() {
        let (body, changed) = merge_settings(&settings(), &patch(json!({"language": null, "auto_range": true}))).unwrap();
        assert_eq!(changed, vec!["auto_range"]);
        assert_eq!(body["language"], "fr-FR");
    }

    #[test]
    fn confirmation_phrase() {
        assert!(check_confirmation(true, None).is_ok());
        assert!(check_confirmation(false, None).is_err());
        assert!(check_confirmation(false, Some("i understand real copies will start")).is_err());
        assert!(check_confirmation(false, Some(CONFIRM_PHRASE)).is_ok());
    }

    #[test]
    fn dates() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso8601(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn html_conversion() {
        let md = html_to_markdown(r#"<h3 id="a">Titre</h3><p>Un <b>gras</b> et <code>{n}</code> &lt;ok&gt; <a href="https://x.org/y" target="_blank">lien</a>.</p><ul><li>un</li><li>deux</li></ul><pre><code>a: <b>b</b>
  c</code></pre><div class="warn">Attention</div>"#);
        assert!(md.contains("## Titre"), "{md}");
        assert!(md.contains("Un **gras** et `{n}` <ok> [lien](https://x.org/y)."), "{md}");
        assert!(md.contains("\n- un\n- deux"), "{md}");
        assert!(md.contains("```\na: b\n  c\n```"), "{md}");
        assert!(md.contains("> **Warning:** Attention"), "{md}");
        assert!(!md.contains('<') || md.contains("<ok>"));
    }

    #[test]
    fn plan_item_summary() {
        let it = json!({
            "nom": "Show.S02.1080p", "reconnu": {"titre": "Show", "annee": 2020, "categorie": "serie", "media": "tv", "tmdb_id": 5},
            "candidats": [],
            "decision": {"decision": "ranger", "vers": "/series/Show (2020)", "nouveau_dossier": true, "renumerotation": [2, 1], "renommages": [{"ancien": "a", "nouveau": "b"}],
                "fichiers": [{"src": "/in/Show.S02.1080p/e1.mkv", "dst": "/series/Show (2020)/Season 01/Show - S01E01.mkv"}]}
        });
        let s = summarize_item(&it);
        assert_eq!(s["decision"], "ranger");
        assert_eq!(s["recognized"]["title"], "Show");
        assert_eq!(s["renumbering"], "S02 -> S01");
        assert_eq!(s["files"], 1);
        assert_eq!(s["example"], "e1.mkv -> Season 01/Show - S01E01.mkv");
        let t = summarize_item(&json!({"nom": "X", "reconnu": null, "decision": {"decision": "a_trier", "raison": "doute", "propositions": []},
            "candidats": [{"id": 1, "media": "movie", "title": "A", "year": 2000}, {"id": 2, "media": "tv", "title": "B", "year": null}, {"id": 3, "media": "tv", "title": "C"}, {"id": 4, "media": "tv", "title": "D"}]}));
        assert_eq!(t["reason"], "doute");
        assert_eq!(t["candidates"].as_array().unwrap().len(), 3);
        assert!(t.get("recognized").is_none());
    }

    #[test]
    fn hints_and_history() {
        let h = status_hints(&json!({"tmdb_ok": false, "entrees": 0, "destinations": 0, "essai": true}), &json!(["/x"]));
        assert!(h.len() >= 5 && h[0].contains("TMDB"));
        let hist = summarize_history(&json!({"entrees": [{"id": "e1", "t": 0, "nom": "n", "titre": "t", "categorie": "film", "statut": "erreur", "motif": "disque plein", "vers": "/v", "fichiers": [], "taille_totale": 5_000_000}, {"id": "e2"}]}), 1);
        assert_eq!(hist["total"], 2);
        assert_eq!(hist["shown"], 1);
        assert_eq!(hist["entries"][0]["reason"], "disque plein");
        assert_eq!(hist["entries"][0]["size_mb"], 5);
    }

    #[test]
    fn api_base_normalisation() {
        assert_eq!(Api::new("127.0.0.1:3050/").base, "http://127.0.0.1:3050");
        assert_eq!(Api::new(" https://x.example ").base, "https://x.example");
    }
}
