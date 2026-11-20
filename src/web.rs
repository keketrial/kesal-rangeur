//! Serveur web : API JSON + interface intégrée au binaire.

use crate::config::{Category, Config, Input, Rule, Secrets};
use crate::library::LibEntry;
use crate::plan::{Choice, Decision, Item, Planner};
use crate::tmdb::{Candidate, Media, Tmdb};
use crate::{norm, parser};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub t: u64,
    pub niveau: &'static str,
    pub msg: String,
}

#[derive(Default)]
pub struct ScanState {
    pub running: bool,
    pub last: Option<u64>,
    pub error: Option<String>,
}

pub struct AppState {
    pub cfg_path: PathBuf,
    pub cfg: RwLock<Config>,
    pub tmdb: RwLock<Option<Arc<Tmdb>>>,
    pub planner: RwLock<Option<Arc<Planner>>>,
    pub items: RwLock<Vec<Item>>,
    pub scan: StdMutex<ScanState>,
    pub log: StdMutex<VecDeque<LogLine>>,
}

impl AppState {
    pub fn log(&self, niveau: &'static str, msg: impl Into<String>) {
        let msg = msg.into();
        eprintln!("[{niveau}] {msg}");
        let mut l = self.log.lock().unwrap();
        l.push_back(LogLine { t: now(), niveau, msg });
        while l.len() > 500 {
            l.pop_front();
        }
    }
}

type S = Arc<AppState>;
type R<T> = Result<Json<T>, (StatusCode, String)>;

fn err500<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn bad<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}

pub async fn build_tmdb(cfg: &Config) -> Option<Arc<Tmdb>> {
    let secret = cfg.tmdb_secret().ok()?;
    Tmdb::new(secret, cfg.language.clone(), &cfg.data_dir).ok().map(Arc::new)
}

pub async fn serve(cfg_path: PathBuf, bind: String) -> anyhow::Result<()> {
    let cfg = if cfg_path.exists() { Config::load(&cfg_path)? } else { default_config() };
    let tmdb = build_tmdb(&cfg).await;
    let st: S = Arc::new(AppState {
        cfg_path,
        cfg: RwLock::new(cfg),
        tmdb: RwLock::new(tmdb),
        planner: RwLock::new(None),
        items: RwLock::new(vec![]),
        scan: StdMutex::new(ScanState::default()),
        log: StdMutex::new(VecDeque::new()),
    });
    st.log("info", format!("kesal-rangeur {} démarré sur {bind}", env!("CARGO_PKG_VERSION")));
    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/app.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("../web/app.js")) }))
        .route("/i18n.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("../web/i18n.js")) }))
        .route("/style.css", get(|| async { ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], include_str!("../web/style.css")) }))
        .route("/api/status", get(api_status))
        .route("/api/scan", post(api_scan))
        .route("/api/plan", get(api_plan))
        .route("/api/choice", post(api_choice))
        .route("/api/tmdb/search", get(api_search))
        .route("/api/tmdb/test", post(api_tmdb_test))
        .route("/api/settings", get(api_settings_get).post(api_settings_set))
        .route("/api/fs", get(api_fs))
        .route("/api/library", get(api_library))
        .route("/api/log", get(api_log))
        .route("/api/help", get(api_help))
        .with_state(st);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn default_config() -> Config {
    Config {
        language: "fr-FR".into(),
        tmdb_key_file: String::new(),
        integrations: Default::default(),
        data_dir: "data".into(),
        inputs: vec![],
        destinations: BTreeMap::new(),
        film_libraries: vec![],
        rules: vec![],
    }
}

// ---------- état ----------

async fn api_status(State(st): State<S>) -> R<Value> {
    let cfg = st.cfg.read().await;
    let items = st.items.read().await;
    let mut counts = json!({"ranger":0,"doublon":0,"a_trier":0,"attendre":0,"ignorer":0});
    for it in items.iter() {
        let k = match it.decision {
            Decision::Ranger { .. } => "ranger",
            Decision::Doublon { .. } => "doublon",
            Decision::ATrier { .. } => "a_trier",
            Decision::Attendre { .. } => "attendre",
            Decision::Ignorer { .. } => "ignorer",
        };
        counts[k] = json!(counts[k].as_u64().unwrap_or(0) + 1);
    }
    let (scanning, dernier, erreur) = {
        let sc = st.scan.lock().unwrap();
        (sc.running, sc.last, sc.error.clone())
    };
    let secrets = Secrets::load(&cfg.data_dir);
    let planner = st.planner.read().await;
    let tmdb_ok = st.tmdb.read().await.is_some();
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "essai": true,
        "scanning": scanning,
        "dernier_scan": dernier,
        "erreur": erreur,
        "tmdb_ok": tmdb_ok,
        "entrees": cfg.inputs.len(),
        "destinations": cfg.destinations.len(),
        "plex": !cfg.integrations.plex_url.is_empty() && !secrets.plex.is_empty(),
        "jellyfin": !cfg.integrations.jellyfin_url.is_empty() && !secrets.jellyfin.is_empty(),
        "compteurs": counts,
        "total": items.len(),
        "films_connus": planner.as_ref().map(|p| p.movie_count()),
        "choix_retenus": planner.as_ref().map(|p| p.choices_count()),
        "langue": cfg.language,
    })))
}

async fn api_scan(State(st): State<S>) -> R<Value> {
    {
        let mut sc = st.scan.lock().unwrap();
        if sc.running {
            return Ok(Json(json!({"deja": true})));
        }
        sc.running = true;
        sc.error = None;
    }
    let st2 = st.clone();
    tokio::spawn(async move { run_scan(st2).await });
    Ok(Json(json!({"demarre": true})))
}

async fn run_scan(st: S) {
    st.log("info", "Analyse des dossiers d'arrivée…");
    let cfg = Arc::new(st.cfg.read().await.clone());
    let result: Result<(Arc<Planner>, Vec<Item>), String> = async {
        let tmdb = st.tmdb.read().await.clone().ok_or("clé TMDB manquante ou invalide : voir Réglages")?;
        if cfg.inputs.is_empty() {
            return Err("aucun dossier d'arrivée : voir Réglages".to_string());
        }
        let (c2, t2) = (cfg.clone(), tmdb.clone());
        let planner = Arc::new(tokio::task::spawn_blocking(move || Planner::new(c2, t2)).await.map_err(|e| e.to_string())?);
        let mut all = Vec::new();
        for input in &cfg.inputs {
            match planner.plan_input(input).await {
                Ok(mut v) => all.append(&mut v),
                Err(e) => st.log("erreur", format!("{} : {e}", input.path.display())),
            }
        }
        tmdb.save_cache();
        Ok((planner, all))
    }
    .await;
    match result {
        Ok((planner, items)) => {
            let n = items.len();
            *st.planner.write().await = Some(planner);
            *st.items.write().await = items;
            {
                let mut sc = st.scan.lock().unwrap();
                sc.running = false;
                sc.last = Some(now());
            }
            st.log("info", format!("Analyse terminée : {n} élément(s)"));
        }
        Err(e) => {
            {
                let mut sc = st.scan.lock().unwrap();
                sc.running = false;
                sc.error = Some(e.clone());
            }
            st.log("erreur", e);
        }
    }
}

async fn api_plan(State(st): State<S>) -> R<Value> {
    let items = st.items.read().await;
    Ok(Json(json!({ "items": &*items })))
}

// ---------- À trier ----------

#[derive(Deserialize)]
struct ChoiceReq {
    nom: String,
    tmdb_id: u64,
    media: Media,
    category: Option<Category>,
}

async fn api_choice(State(st): State<S>, Json(req): Json<ChoiceReq>) -> R<Value> {
    let planner = st.planner.read().await.clone().ok_or_else(|| bad("lancez d'abord une analyse"))?;
    let (source, entree, release) = {
        let items = st.items.read().await;
        let it = items.iter().find(|i| i.nom == req.nom).ok_or_else(|| bad("élément introuvable"))?;
        (it.source.clone(), it.entree.clone(), it.release.clone())
    };
    planner.set_choice(&release, Choice { media: req.media, id: req.id_or(), category: req.category });
    let cfg = st.cfg.read().await;
    let input = cfg.inputs.iter().find(|i| i.path == entree).cloned().ok_or_else(|| bad("dossier d'arrivée inconnu"))?;
    drop(cfg);
    let item = planner.plan_one(&input, &source).await;
    st.log("info", format!("Choix retenu pour « {} » : fiche TMDB {}", req.nom, req.tmdb_id));
    let mut items = st.items.write().await;
    if let Some(slot) = items.iter_mut().find(|i| i.nom == req.nom) {
        *slot = item.clone();
    }
    Ok(Json(json!({ "item": item })))
}

impl ChoiceReq {
    fn id_or(&self) -> u64 {
        self.tmdb_id
    }
}

#[derive(Deserialize)]
struct SearchQ {
    q: String,
    kind: Option<String>,
}

async fn api_search(State(st): State<S>, Query(q): Query<SearchQ>) -> R<Vec<Candidate>> {
    let tmdb = st.tmdb.read().await.clone().ok_or_else(|| bad("clé TMDB manquante"))?;
    let kind = q.kind.unwrap_or_else(|| "all".into());
    let mut out = Vec::new();
    if kind != "tv" {
        out.extend(tmdb.search_kind(Media::Movie, &q.q).await.map_err(err500)?);
    }
    if kind != "movie" {
        out.extend(tmdb.search_kind(Media::Tv, &q.q).await.map_err(err500)?);
    }
    for c in &mut out {
        c.sim = norm::similarity(&q.q, &c.title).max(norm::similarity(&q.q, &c.original_title));
    }
    out.sort_by(|a, b| (b.sim + b.popularity.min(100.0) / 500.0).partial_cmp(&(a.sim + a.popularity.min(100.0) / 500.0)).unwrap());
    out.truncate(12);
    Ok(Json(out))
}

async fn api_tmdb_test(State(st): State<S>) -> R<Value> {
    let tmdb = st.tmdb.read().await.clone();
    match tmdb {
        None => Ok(Json(json!({"ok": false, "message": "Aucune clé enregistrée."}))),
        Some(t) => match t.test().await {
            Ok(()) => Ok(Json(json!({"ok": true, "message": "La clé fonctionne."}))),
            Err(e) => Ok(Json(json!({"ok": false, "message": e.to_string()}))),
        },
    }
}

// ---------- réglages ----------

#[derive(Serialize, Deserialize)]
struct SettingsIn {
    language: String,
    inputs: Vec<Input>,
    destinations: BTreeMap<Category, PathBuf>,
    film_libraries: Vec<PathBuf>,
    rules: Vec<Rule>,
    plex_url: String,
    jellyfin_url: String,
    ha_url: String,
    /// Secrets : vides = inchangés.
    #[serde(default)]
    tmdb_key: String,
    #[serde(default)]
    plex_token: String,
    #[serde(default)]
    jellyfin_key: String,
    #[serde(default)]
    ha_token: String,
}

fn hint(s: &str) -> Value {
    if s.is_empty() {
        json!({"ok": false})
    } else {
        let tail: String = s.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
        json!({"ok": true, "fin": tail})
    }
}

async fn api_settings_get(State(st): State<S>) -> R<Value> {
    let cfg = st.cfg.read().await;
    let mut sec = Secrets::load(&cfg.data_dir);
    if sec.tmdb.is_empty() {
        sec.tmdb = cfg.tmdb_secret().unwrap_or_default();
    }
    let missing: Vec<String> = cfg.destinations.values().chain(cfg.inputs.iter().map(|i| &i.path)).filter(|p| !p.exists()).map(|p| p.display().to_string()).collect();
    Ok(Json(json!({
        "language": cfg.language,
        "inputs": cfg.inputs,
        "destinations": cfg.destinations,
        "film_libraries": cfg.film_libraries,
        "rules": cfg.rules,
        "plex_url": cfg.integrations.plex_url,
        "jellyfin_url": cfg.integrations.jellyfin_url,
        "ha_url": cfg.integrations.ha_url,
        "secrets": {"tmdb": hint(&sec.tmdb), "plex": hint(&sec.plex), "jellyfin": hint(&sec.jellyfin), "ha": hint(&sec.ha)},
        "dossiers_introuvables": missing,
        "categories": ["film","serie","dessin_anime","telerealite","documentaire","spectacle"],
    })))
}

async fn api_settings_set(State(st): State<S>, Json(s): Json<SettingsIn>) -> R<Value> {
    let mut cfg = st.cfg.write().await;
    cfg.language = s.language;
    cfg.inputs = s.inputs;
    cfg.destinations = s.destinations;
    cfg.film_libraries = s.film_libraries;
    cfg.rules = s.rules;
    cfg.integrations.plex_url = s.plex_url.trim().to_string();
    cfg.integrations.jellyfin_url = s.jellyfin_url.trim().to_string();
    cfg.integrations.ha_url = s.ha_url.trim().to_string();
    let mut sec = Secrets::load(&cfg.data_dir);
    for (dst, src) in [(&mut sec.tmdb, &s.tmdb_key), (&mut sec.plex, &s.plex_token), (&mut sec.jellyfin, &s.jellyfin_key), (&mut sec.ha, &s.ha_token)] {
        if !src.trim().is_empty() {
            *dst = src.trim().to_string();
        }
    }
    sec.save(&cfg.data_dir).map_err(err500)?;
    cfg.save(&st.cfg_path).map_err(err500)?;
    let new_tmdb = build_tmdb(&cfg).await;
    let ok = new_tmdb.is_some();
    drop(cfg);
    *st.tmdb.write().await = new_tmdb;
    st.log("info", "Réglages enregistrés");
    Ok(Json(json!({"ok": true, "tmdb": ok})))
}

// ---------- explorateur de dossiers ----------

#[derive(Deserialize)]
struct FsQ {
    path: Option<String>,
}

async fn api_fs(Query(q): Query<FsQ>) -> R<Value> {
    let path = PathBuf::from(q.path.filter(|p| !p.is_empty()).unwrap_or_else(|| "/".into()));
    let path = if path.is_dir() { path } else { PathBuf::from("/") };
    let mut dirs: Vec<String> = std::fs::read_dir(&path)
        .map_err(err500)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.') && !n.starts_with('@') && !n.starts_with('#'))
        .collect();
    dirs.sort_by_key(|d| d.to_lowercase());
    dirs.truncate(1000);
    let videos = std::fs::read_dir(&path).map(|r| r.filter_map(|e| e.ok()).filter(|e| e.path().extension().and_then(|x| x.to_str()).map_or(false, parser::is_video_ext)).count()).unwrap_or(0);
    Ok(Json(json!({
        "path": path,
        "parent": path.parent().map(|p| p.to_path_buf()),
        "dirs": dirs,
        "videos": videos,
    })))
}

// ---------- bibliothèque ----------

#[derive(Serialize)]
struct LibRow<'a> {
    nom: &'a str,
    titre: &'a str,
    annee: Option<u16>,
    saisons: Vec<u16>,
    videos: usize,
    integrale: bool,
    chemin: &'a Path,
    suspect: bool,
}

async fn api_library(State(st): State<S>) -> R<Value> {
    let planner = st.planner.read().await.clone();
    let Some(p) = planner else {
        return Ok(Json(json!({"pret": false})));
    };
    let mut cats = BTreeMap::new();
    for (cat, list) in &p.series {
        let rows: Vec<LibRow> = list
            .iter()
            .map(|e: &LibEntry| LibRow {
                nom: &e.name,
                titre: &e.title,
                annee: e.year,
                saisons: e.seasons.iter().copied().collect(),
                videos: e.videos,
                integrale: e.complete,
                chemin: &e.path,
                suspect: e.seasons.is_empty() && e.videos > 0,
            })
            .collect();
        cats.insert(cat.label().to_string(), rows);
    }
    Ok(Json(json!({"pret": true, "categories": cats, "films": p.movie_count()})))
}

async fn api_log(State(st): State<S>) -> R<Value> {
    let l: Vec<LogLine> = st.log.lock().unwrap().iter().rev().cloned().collect();
    Ok(Json(json!({ "lignes": l })))
}

#[derive(Deserialize)]
struct HelpQ {
    lang: Option<String>,
}

async fn api_help(Query(q): Query<HelpQ>) -> impl IntoResponse {
    let html = match q.lang.as_deref() {
        _ => include_str!("../web/help.fr.html"),
    };
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html)
}
