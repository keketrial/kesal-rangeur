//! Serveur web : API JSON + interface intégrée au binaire.

use crate::config::{Category, Config, Input, Rule, Secrets};
use crate::exec::{self, Action, Entry, History};
use crate::naming::{self, Naming};
use crate::notify;
use crate::library::LibEntry;
use crate::plan::{Choice, Decision, Item, Planner};
use crate::tmdb::{Candidate, Media, Tmdb};
use crate::{norm, parser};
use axum::extract::{ConnectInfo, Query, Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
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
    pub history: History,
    /// Mot de passe facultatif (variable KESAL_PASSWORD) : protège toute l'interface.
    pub password: Option<String>,
    /// Essais de mot de passe ratés (par adresse) : blocage temporaire contre les devinettes.
    pub logins: StdMutex<LoginGuard>,
    /// Noms d'hôte supplémentaires autorisés (KESAL_ALLOWED_HOSTS), ex. un nom de domaine derrière un tunnel.
    pub allowed_hosts: Vec<String>,
    /// Éléments en cours de rangement (évite les doubles clics) et file d'attente : une copie à la fois.
    pub busy: StdMutex<HashSet<String>>,
    pub gate: tokio::sync::Semaphore,
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
    let data_dir = cfg.data_dir.clone();
    let st: S = Arc::new(AppState {
        cfg_path,
        cfg: RwLock::new(cfg),
        tmdb: RwLock::new(tmdb),
        planner: RwLock::new(None),
        items: RwLock::new(vec![]),
        scan: StdMutex::new(ScanState::default()),
        log: StdMutex::new(VecDeque::new()),
        history: History::load(&data_dir),
        password: std::env::var("KESAL_PASSWORD").ok().filter(|p| !p.is_empty()),
        allowed_hosts: std::env::var("KESAL_ALLOWED_HOSTS").unwrap_or_default().split(',').map(|h| h.trim().to_lowercase()).filter(|h| !h.is_empty()).collect(),
        logins: StdMutex::new(LoginGuard::default()),
        busy: StdMutex::new(HashSet::new()),
        gate: tokio::sync::Semaphore::new(1),
    });
    st.log("info", format!("kesal-rangeur {} démarré sur {bind}", env!("CARGO_PKG_VERSION")));
    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/app.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("../web/app.js")) }))
        .route("/i18n.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("../web/i18n.js")) }))
        .route("/tmdb-logo.svg", get(|| async { ([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "public, max-age=86400")], include_str!("../web/tmdb-logo.svg")) }))
        .route("/style.css", get(|| async { ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], include_str!("../web/style.css")) }))
        .route("/api/status", get(api_status))
        .route("/api/scan", post(api_scan))
        .route("/api/plan", get(api_plan))
        .route("/api/report", get(api_report))
        .route("/api/mode", post(api_mode))
        .route("/api/choice", post(api_choice))
        .route("/api/tmdb/search", get(api_search))
        .route("/api/tmdb/test", post(api_tmdb_test))
        .route("/api/settings", get(api_settings_get).post(api_settings_set))
        .route("/api/fs", get(api_fs))
        .route("/api/library", get(api_library))
        .route("/api/log", get(api_log))
        .route("/api/help", get(api_help))
        .route("/api/run", post(api_run))
        .route("/api/history", get(api_history))
        .route("/api/undo", post(api_undo))
        .route("/api/services/test", post(api_services_test))
        .route("/api/naming/preview", post(api_naming_preview))
        .layer(middleware::from_fn_with_state(st.clone(), guard))
        .with_state(st.clone());
    if st.password.is_none() {
        st.log("info", "Pas de mot de passe (KESAL_PASSWORD) : gardez cette interface sur votre réseau local.");
    }
    // Nettoyage des dossiers provisoires abandonnés par un arrêt brutal, puis analyse périodique (si réglée) : rien n'est écrit tant que l'essai à blanc est actif.
    tokio::spawn(sweep_loop(st.clone()));
    tokio::spawn(scheduler(st.clone()));
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}

pub fn default_config() -> Config {
    Config {
        language: "fr-FR".into(),
        tmdb_key_file: String::new(),
        integrations: Default::default(),
        naming: Default::default(),
        essai: true,
        copy_method: Default::default(),
        scan_interval_min: 0,
        auto_range: false,
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
    let mut counts = json!({"ranger":0,"doublon":0,"a_trier":0,"attendre":0,"ignorer":0,"fait":0});
    for it in items.iter() {
        let k = match it.decision {
            Decision::Ranger { .. } => "ranger",
            Decision::Doublon { .. } => "doublon",
            Decision::ATrier { .. } => "a_trier",
            Decision::Attendre { .. } => "attendre",
            Decision::Ignorer { .. } => "ignorer",
            Decision::Fait { .. } => "fait",
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
        "essai": cfg.essai,
        "auto_range": cfg.auto_range,
        "intervalle_min": cfg.scan_interval_min,
        "en_cours": st.busy.lock().unwrap().len(),
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
    tokio::spawn(async move { run_scan_guarded(st2).await });
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
            let (essai, auto) = {
                let c = st.cfg.read().await;
                (c.essai, c.auto_range)
            };
            if auto && !essai {
                run_ready(st.clone(), None, false).await;
            }
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

// ---------- Simulation complète : tout ce qui se passerait, sans rien écrire ----------

async fn api_report(State(st): State<S>) -> R<Value> {
    let items = st.items.read().await.clone();
    let cfg = st.cfg.read().await;
    let (essai, auto, interval, methode) = (cfg.essai, cfg.auto_range, cfg.scan_interval_min, format!("{:?}", cfg.copy_method));
    drop(cfg);
    let dernier = st.scan.lock().unwrap().last;
    const MAX_LIGNES: usize = 400;
    let (mut fichiers, mut octets, mut renommes, mut nouveaux, mut existants) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut lignes = Vec::new();
    let mut compte = std::collections::BTreeMap::<&str, u64>::new();
    for it in &items {
        let titre = it.reconnu.as_ref().map(|r| match r.annee { Some(a) => format!("{} ({a})", r.titre), None => r.titre.clone() });
        let (kind, vers, raison, nb, taille, changes) = match &it.decision {
            Decision::Ranger { vers, renommages, fichiers: places, .. } => {
                // « nouveau » = le dossier de destination n'existe pas encore sur le disque (vérifié, pas déduit).
                let nouveau_dossier = &!vers.exists();
                let lf = crate::plan::list_files(&it.source);
                let nb = lf.len() as u64;
                let taille: u64 = lf.iter().map(|(_, n)| *n).sum();
                fichiers += nb;
                octets += taille;
                if *nouveau_dossier { nouveaux += 1 } else { existants += 1 }
                let mut ch: Vec<(String, String)> = if !places.is_empty() {
                    places.iter().map(|p| (p.src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), p.dst.to_string_lossy().to_string())).collect()
                } else {
                    renommages.iter().map(|r| (r.ancien.clone(), r.nouveau.clone())).collect()
                };
                ch.retain(|(a, b)| a != b);
                renommes += ch.len() as u64;
                ch.truncate(60);
                ("ranger", Some(vers.to_string_lossy().to_string()), if *nouveau_dossier { "nouveau dossier".to_string() } else { "dossier existant".to_string() }, nb, taille, ch)
            }
            Decision::Doublon { existant, raison } => ("doublon", Some(existant.to_string_lossy().to_string()), raison.clone(), 0, 0, vec![]),
            Decision::ATrier { raison, .. } => ("a_trier", None, raison.clone(), 0, 0, vec![]),
            Decision::Attendre { raison } => ("attendre", None, raison.clone(), 0, 0, vec![]),
            Decision::Ignorer { raison } => ("ignorer", None, raison.clone(), 0, 0, vec![]),
            Decision::Fait { vers } => ("fait", Some(vers.to_string_lossy().to_string()), String::new(), 0, 0, vec![]),
        };
        *compte.entry(kind).or_default() += 1;
        if lignes.len() < MAX_LIGNES * 4 {
            lignes.push(json!({
                "nom": it.nom, "decision": kind, "titre": titre, "vers": vers, "raison": raison,
                "fichiers": nb, "octets": taille, "changements": changes.iter().map(|(a, b)| json!({"de": a, "vers": b})).collect::<Vec<_>>(),
            }));
        }
    }
    Ok(Json(json!({
        "dernier_scan": dernier, "essai": essai, "auto_range": auto, "intervalle_min": interval, "methode": methode,
        "totaux": { "elements": items.len(), "compte": compte, "fichiers": fichiers, "octets": octets, "renommes": renommes,
                    "nouveaux_dossiers": nouveaux, "dossiers_existants": existants },
        "lignes": lignes,
    })))
}

#[derive(Deserialize)]
struct ModeReq {
    /// "simulation" | "manuel" | "auto"
    mode: String,
    interval_min: Option<u64>,
    confirm: Option<bool>,
}

/// Choisit le mode de fonctionnement. Passer en réel exige : une analyse déjà faite (donc une simulation vue) + une confirmation explicite.
async fn api_mode(State(st): State<S>, Json(req): Json<ModeReq>) -> R<Value> {
    let mut cfg = st.cfg.write().await;
    match req.mode.as_str() {
        "simulation" => {
            cfg.essai = true;
            cfg.auto_range = false;
        }
        "manuel" | "auto" => {
            if req.confirm != Some(true) {
                return Err(bad("confirmation requise pour passer en mode réel"));
            }
            if st.scan.lock().unwrap().last.is_none() {
                return Err(bad("lancez d'abord une analyse et relisez la simulation avant de passer en mode réel"));
            }
            cfg.essai = false;
            cfg.auto_range = req.mode == "auto";
            if req.mode == "auto" {
                let n = req.interval_min.unwrap_or(if cfg.scan_interval_min > 0 { cfg.scan_interval_min } else { 15 });
                cfg.scan_interval_min = n.clamp(1, 1440);
            }
        }
        _ => return Err(bad("mode inconnu")),
    }
    cfg.save(&st.cfg_path).map_err(err500)?;
    let (essai, auto, n) = (cfg.essai, cfg.auto_range, cfg.scan_interval_min);
    drop(cfg);
    st.log("info", format!("Mode choisi : {} (essai à blanc : {}, rangement automatique : {}, analyse toutes les {} min)", req.mode, essai, auto, n));
    Ok(Json(json!({ "ok": true, "essai": essai, "auto_range": auto, "intervalle_min": n })))
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
    #[serde(default)]
    ha_service: String,
    essai: bool,
    copy_method: exec::Method,
    scan_interval_min: u64,
    auto_range: bool,
    #[serde(default)]
    naming: Naming,
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
    json!({"ok": !s.is_empty()})
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
        "ha_service": cfg.integrations.ha_service,
        "naming": cfg.naming,
        "essai": cfg.essai,
        "copy_method": cfg.copy_method,
        "scan_interval_min": cfg.scan_interval_min,
        "auto_range": cfg.auto_range,
        "secrets": {"tmdb": hint(&sec.tmdb), "plex": hint(&sec.plex), "jellyfin": hint(&sec.jellyfin), "ha": hint(&sec.ha)},
        "dossiers_introuvables": missing,
        "categories": ["film","serie","dessin_anime","telerealite","documentaire","spectacle"],
    })))
}

async fn api_settings_set(State(st): State<S>, Json(s): Json<SettingsIn>) -> R<Value> {
    if let Some(f) = s.naming.formats() {
        naming::preview(&f).map_err(|e| bad(format!("format de nom invalide : {e}")))?;
    }
    for (nom, u) in [("Plex", &s.plex_url), ("Jellyfin", &s.jellyfin_url), ("Home Assistant", &s.ha_url)] {
        let u = u.trim();
        if !u.is_empty() && !(u.starts_with("http://") || u.starts_with("https://")) {
            return Err(bad(format!("adresse de {nom} invalide : elle doit commencer par http:// ou https://")));
        }
    }
    let mut cfg = st.cfg.write().await;
    let old_urls = cfg.integrations.clone();
    cfg.naming = s.naming.clone();
    cfg.language = s.language;
    cfg.inputs = s.inputs;
    cfg.destinations = s.destinations;
    cfg.film_libraries = s.film_libraries;
    cfg.rules = s.rules;
    cfg.integrations.plex_url = s.plex_url.trim().to_string();
    cfg.integrations.jellyfin_url = s.jellyfin_url.trim().to_string();
    cfg.integrations.ha_url = s.ha_url.trim().to_string();
    cfg.integrations.ha_service = s.ha_service.trim().to_string();
    if cfg.essai != s.essai {
        let msg = if s.essai { "Mode essai à blanc ACTIVÉ" } else { "Mode essai à blanc DÉSACTIVÉ : les rangements seront réels" };
        drop(cfg);
        st.log("info", msg);
        cfg = st.cfg.write().await;
    }
    cfg.essai = s.essai;
    cfg.copy_method = s.copy_method;
    cfg.scan_interval_min = s.scan_interval_min;
    cfg.auto_range = s.auto_range;
    let mut sec = Secrets::load(&cfg.data_dir);
    for (dst, src) in [(&mut sec.tmdb, &s.tmdb_key), (&mut sec.plex, &s.plex_token), (&mut sec.jellyfin, &s.jellyfin_key), (&mut sec.ha, &s.ha_token)] {
        if !src.trim().is_empty() {
            *dst = src.trim().to_string();
        }
    }
    // Un jeton n'est valable que pour l'adresse à laquelle il a été saisi : si l'adresse change sans nouveau jeton, l'ancien est effacé
    // (sinon, changer l'adresse suffirait à envoyer le jeton ailleurs).
    let mut effaces = Vec::new();
    if cfg.integrations.plex_url.trim_end_matches('/') != old_urls.plex_url.trim_end_matches('/') && s.plex_token.trim().is_empty() && !sec.plex.is_empty() {
        sec.plex.clear();
        effaces.push("Plex");
    }
    if cfg.integrations.jellyfin_url.trim_end_matches('/') != old_urls.jellyfin_url.trim_end_matches('/') && s.jellyfin_key.trim().is_empty() && !sec.jellyfin.is_empty() {
        sec.jellyfin.clear();
        effaces.push("Jellyfin");
    }
    if cfg.integrations.ha_url.trim_end_matches('/') != old_urls.ha_url.trim_end_matches('/') && s.ha_token.trim().is_empty() && !sec.ha.is_empty() {
        sec.ha.clear();
        effaces.push("Home Assistant");
    }
    sec.save(&cfg.data_dir).map_err(err500)?;
    cfg.save(&st.cfg_path).map_err(err500)?;
    let new_tmdb = build_tmdb(&cfg).await;
    let ok = new_tmdb.is_some();
    drop(cfg);
    *st.tmdb.write().await = new_tmdb;
    st.log("info", "Réglages enregistrés");
    if !effaces.is_empty() {
        st.log("info", format!("Adresse modifiée : le jeton de {} a été effacé, saisissez-le à nouveau.", effaces.join(", ")));
    }
    Ok(Json(json!({"ok": true, "tmdb": ok, "jetons_effaces": effaces})))
}

// ---------- explorateur de dossiers ----------

#[derive(Deserialize)]
struct FsQ {
    path: Option<String>,
}

async fn api_fs(Query(q): Query<FsQ>) -> R<Value> {
    let path = PathBuf::from(q.path.filter(|p| !p.is_empty()).unwrap_or_else(|| "/".into()));
    let path = if path.is_dir() { path } else { PathBuf::from("/") };
    let mut dirs: Vec<String> = tokio::task::block_in_place(|| std::fs::read_dir(&path))
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
    let liste_films: Vec<Value> = p
        .movie_list()
        .into_iter()
        .map(|(titre, annee, chemin)| {
            let nom = chemin.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let dossier = chemin.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            json!({"titre": titre, "annee": annee, "nom": nom, "dossier": dossier})
        })
        .collect();
    Ok(Json(json!({"pret": true, "categories": cats, "films": p.movie_count(), "liste_films": liste_films})))
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

// ---------- rangement réel (refusé tant que l'essai à blanc est actif) ----------

async fn scheduler(st: S) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let (every, last, running) = {
            let c = st.cfg.read().await;
            let sc = st.scan.lock().unwrap();
            (c.scan_interval_min, sc.last.unwrap_or(0), sc.running)
        };
        if every > 0 && !running && now().saturating_sub(last) >= every * 60 {
            st.scan.lock().unwrap().running = true;
            run_scan_guarded(st.clone()).await;
        }
    }
}

#[derive(Deserialize)]
struct RunReq {
    nom: Option<String>,
    /// Essai sur UN seul élément, permis même en simulation : copie forcée (l'original reste), annulable.
    test: Option<bool>,
}

async fn api_run(State(st): State<S>, Json(req): Json<RunReq>) -> R<Value> {
    let test = req.test == Some(true);
    if st.cfg.read().await.essai && !(test && req.nom.is_some()) {
        return Err((StatusCode::FORBIDDEN, "Mode simulation actif : rien n'est écrit. Passez en mode Manuel ou Auto (en haut) quand vous êtes prêt, ou utilisez « Essayer sur cet élément ».".into()));
    }
    let st2 = st.clone();
    tokio::spawn(async move { run_ready(st2, req.nom, test).await });
    Ok(Json(json!({"demarre": true})))
}

/// Range les éléments « À ranger » (un seul si `only` est donné). Une copie à la fois.
async fn run_ready(st: S, only: Option<String>, test: bool) {
    let todo: Vec<Item> = st
        .items
        .read()
        .await
        .iter()
        .filter(|i| matches!(i.decision, Decision::Ranger { .. }) && only.as_ref().map_or(true, |n| &i.nom == n))
        .cloned()
        .collect();
    for it in todo {
        // Un élément qui vient d'échouer n'est pas relancé à chaque analyse (rangement automatique) : une heure de pause.
        // Un clic de l'utilisateur (« Réessayer ») passe toujours.
        if only.is_none() && st.history.last_error_age(&it.nom).map_or(false, |age| age < 3600) {
            continue;
        }
        {
            let mut b = st.busy.lock().unwrap();
            if !b.insert(it.nom.clone()) {
                continue;
            }
        }
        let _permit = st.gate.acquire().await;
        run_one(&st, &it, test).await;
        st.busy.lock().unwrap().remove(&it.nom);
    }
}

async fn run_one(st: &S, it: &Item, test: bool) {
    let cfg = st.cfg.read().await.clone();
    if cfg.essai && !test {
        return; // double sécurité : en simulation, seul l'essai sur un élément est permis
    }
    let Decision::Ranger { vers, renommages, fichiers: placements, .. } = &it.decision else { return };
    let plan: Vec<(PathBuf, PathBuf)> = placements.iter().map(|p| (p.src.clone(), p.dst.clone())).collect();
    let Some(mut input) = cfg.inputs.iter().find(|i| i.path == it.entree).cloned() else { return };
    if test {
        // Essai : toujours une COPIE (l'original reste en place), quoi qu'indique le réglage du dossier d'arrivée.
        input.action = "copy".into();
        st.log("info", format!("Essai sur un seul élément : « {} » (copie, annulable depuis l'Historique)", it.nom));
    }
    let (titre, cat) = it.reconnu.as_ref().map(|r| (r.titre.clone(), Some(r.categorie))).unwrap_or((it.nom.clone(), None));
    // Délais entre les essais (secondes). KESAL_RETRY_DELAYS="0,1,1" sert aux tests.
    let delays: Vec<u64> = std::env::var("KESAL_RETRY_DELAYS").ok().map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect()).filter(|v: &Vec<u64>| !v.is_empty()).unwrap_or_else(|| vec![0, 20, 120]);
    let mut last_err = String::new();
    for (n, d) in delays.iter().enumerate() {
        if *d > 0 {
            st.log("info", format!("Nouvel essai dans {d} s pour « {} »…", it.nom));
            tokio::time::sleep(std::time::Duration::from_secs(*d)).await;
        }
        let plan2 = plan.clone();
        let (src, dst, ren, nom, titre2, action, method, settle) = (it.source.clone(), vers.clone(), renommages.clone(), it.nom.clone(), titre.clone(), Action::from_str(&input.action), cfg.copy_method, input.settle_minutes);
        let res = tokio::task::spawn_blocking(move || {
            exec::execute(&exec::Request { nom: &nom, titre: &titre2, categorie: cat, source: &src, vers: &dst, renommages: &ren, action, method, settle_minutes: settle, plan: &plan2 })
        })
        .await;
        match res {
            Ok(Ok(entry)) => {
                st.log("info", format!("Rangé : « {} » → {} ({} fichier(s), {})", it.nom, entry.vers.display(), entry.fichiers.len(), entry.fichiers.first().map(|f| f.methode.as_str()).unwrap_or("")));
                st.history.add(entry);
                let mut items = st.items.write().await;
                if let Some(slot) = items.iter_mut().find(|x| x.nom == it.nom) {
                    slot.decision = Decision::Fait { vers: vers.clone() };
                }
                drop(items);
                after_success(st, &cfg, it, cat).await;
                return;
            }
            Ok(Err(e)) => {
                last_err = e.to_string();
                // Inutile de réessayer si ce n'est pas un souci passager.
                if !matches!(e, exec::ExecError::Io(_)) || n + 1 == delays.len() {
                    break;
                }
            }
            Err(e) => {
                last_err = e.to_string();
                break;
            }
        }
    }
    st.log("erreur", format!("Échec du rangement de « {} » : {last_err}", it.nom));
    st.history.add(Entry { id: format!("e-{}", exec::unique_id()), t: now(), nom: it.nom.clone(), titre, categorie: cat, action: Action::from_str(&input.action), vers: vers.clone(), fichiers: vec![], dossiers_crees: vec![], taille_totale: 0, plan: false, statut: "erreur".into(), motif: last_err });
}

async fn after_success(st: &S, cfg: &Config, it: &Item, cat: Option<Category>) {
    let sec = Secrets::load(&cfg.data_dir);
    if let Some(cat) = cat {
        if let Some(root) = cfg.destinations.get(&cat) {
            for (ok, msg) in notify::refresh(cfg, &sec, cat, root).await {
                st.log(if ok { "info" } else { "erreur" }, msg);
            }
        }
    }
    let titre = it.reconnu.as_ref().map(|r| r.titre.as_str()).unwrap_or(&it.nom);
    if let Err(e) = notify::home_assistant(cfg, &sec, "Nouveau média rangé", titre).await {
        st.log("erreur", format!("Home Assistant : {e}"));
    }
}

async fn api_history(State(st): State<S>) -> R<Value> {
    let mut v = st.history.all();
    v.reverse();
    Ok(Json(json!({ "entrees": v })))
}

#[derive(Deserialize)]
struct UndoReq {
    id: String,
}

async fn api_undo(State(st): State<S>, Json(req): Json<UndoReq>) -> R<Value> {
    let e = st.history.get(&req.id).ok_or_else(|| bad("rangement introuvable"))?;
    let e2 = e.clone();
    tokio::task::spawn_blocking(move || exec::undo(&e2)).await.map_err(err500)?.map_err(|er| bad(er))?;
    st.history.set_status(&req.id, "annule", "annulé à la demande");
    st.log("info", format!("Annulé : « {} »", e.nom));
    Ok(Json(json!({"ok": true})))
}

async fn api_services_test(State(st): State<S>) -> R<Value> {
    let cfg = st.cfg.read().await.clone();
    let sec = Secrets::load(&cfg.data_dir);
    Ok(Json(notify::test_all(&cfg, &sec).await))
}

#[derive(Deserialize)]
struct PreviewReq {
    naming: Naming,
}

/// Aperçu en direct des formats de nom (Réglages).
async fn api_naming_preview(Json(r): Json<PreviewReq>) -> R<Value> {
    match r.naming.formats() {
        None => Ok(Json(json!({"ok": true, "garde": true}))),
        Some(f) => match naming::preview(&f) {
            Ok((film, serie)) => Ok(Json(json!({"ok": true, "garde": false, "film": film, "serie": serie}))),
            Err(e) => Ok(Json(json!({"ok": false, "erreur": e}))),
        },
    }
}

// ---------- protections de l'interface ----------

/// Le nom d'hôte demandé est-il acceptable ? Une adresse IP, « localhost », un nom court (service Docker) ou « .local » : oui.
/// Un nom de domaine public : seulement s'il est dans KESAL_ALLOWED_HOSTS (protège du « DNS rebinding »).
pub fn host_allowed(host_header: &str, extra: &[String]) -> bool {
    let h = host_header.trim();
    let name = if let Some(rest) = h.strip_prefix('[') { rest.split(']').next().unwrap_or("") } else { h.rsplit_once(':').map(|(a, _)| a).unwrap_or(h) };
    let name = name.to_lowercase();
    if name.is_empty() {
        return false;
    }
    if name.parse::<std::net::IpAddr>().is_ok() || name == "localhost" || !name.contains('.') {
        return true;
    }
    if name.ends_with(".local") || name.ends_with(".lan") || name.ends_with(".home.arpa") {
        return true;
    }
    extra.iter().any(|x| *x == name)
}

/// L'en-tête Origin (envoyé par les navigateurs) doit correspondre à l'hôte demandé : refuse les requêtes venant d'un autre site.
pub fn origin_ok(origin: Option<&str>, host: &str) -> bool {
    match origin {
        None => true, // pas un navigateur (MCP, curl, scripts)
        Some(o) => {
            let auth = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://")).unwrap_or(o);
            auth.eq_ignore_ascii_case(host.trim())
        }
    }
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let bytes: Vec<u8> = s.bytes().filter(|c| *c != b'=').collect();
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// Comparaison en temps constant (évite de deviner le mot de passe par le temps de réponse).
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    let mut d = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        d |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    d == 0
}

fn basic_password_ok(auth_header: Option<&str>, password: &str) -> bool {
    let Some(h) = auth_header else { return false };
    let Some(b) = h.strip_prefix("Basic ").or_else(|| h.strip_prefix("basic ")) else { return false };
    let Some(raw) = b64_decode(b.trim()) else { return false };
    let text = String::from_utf8_lossy(&raw);
    let pass = text.split_once(':').map(|x| x.1).unwrap_or(&text);
    same_secret(pass.as_bytes(), password.as_bytes())
}

fn deny(code: StatusCode, msg: &str) -> Response {
    (code, msg.to_string()).into_response()
}

/// Adresse du client : celle de la connexion ; derrière un proxy local (tunnel, Docker), on lit l'en-tête transmis par le proxy.
fn client_key(peer: IpAddr, fwd: Option<&str>) -> String {
    let local = match peer {
        IpAddr::V4(a) => a.is_loopback() || a.is_private(),
        IpAddr::V6(a) => a.is_loopback() || (a.segments()[0] & 0xfe00) == 0xfc00,
    };
    if local {
        if let Some(ip) = fwd.and_then(|f| f.split(',').next()).map(|x| x.trim()).filter(|x| x.parse::<IpAddr>().is_ok()) {
            return ip.to_string();
        }
    }
    peer.to_string()
}

/// Essais de mot de passe ratés : 5 erreurs en 10 minutes → blocage de 15 minutes pour cette adresse.
#[derive(Default)]
pub struct LoginGuard {
    seen: HashMap<String, (u32, u64, u64)>, // adresse → (erreurs, début de fenêtre, bloqué jusqu'à)
}
impl LoginGuard {
    const MAX: u32 = 5;
    const WINDOW: u64 = 600;
    const LOCK: u64 = 900;
    /// Secondes restantes de blocage, s'il y en a.
    pub fn locked(&self, key: &str, now: u64) -> Option<u64> {
        self.seen.get(key).and_then(|e| (e.2 > now).then(|| e.2 - now))
    }
    pub fn fail(&mut self, key: &str, now: u64) {
        if self.seen.len() > 1000 {
            self.seen.retain(|_, e| e.2 > now || now.saturating_sub(e.1) < Self::WINDOW);
        }
        let e = self.seen.entry(key.to_string()).or_insert((0, now, 0));
        if now.saturating_sub(e.1) >= Self::WINDOW {
            *e = (0, now, 0);
        }
        e.0 += 1;
        if e.0 >= Self::MAX {
            e.2 = now + Self::LOCK;
            *e = (0, now, e.2);
        }
    }
    pub fn ok(&mut self, key: &str) {
        self.seen.remove(key);
    }
}

async fn guard(State(st): State<S>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    // on copie les en-têtes utiles avant toute attente (la requête ne doit pas être empruntée pendant `.await`)
    let (host, auth, origin, mutating, fwd) = {
        let get = |n: &str| req.headers().get(n).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        (get("host").unwrap_or_default(), get("authorization"), get("origin"), !matches!(*req.method(), axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS), get("cf-connecting-ip").or_else(|| get("x-forwarded-for")))
    };
    if !host_allowed(&host, &st.allowed_hosts) {
        return deny(StatusCode::FORBIDDEN, "Nom d'hôte non autorisé. Pour y accéder par un nom de domaine, ajoutez-le à KESAL_ALLOWED_HOSTS.");
    }
    // Bilan de santé (Docker) : ne révèle rien, donc accessible sans mot de passe.
    if req.uri().path() == "/healthz" {
        return (StatusCode::OK, "ok").into_response();
    }
    if let Some(pw) = &st.password {
        let key = client_key(peer.ip(), fwd.as_deref());
        if let Some(wait) = st.logins.lock().unwrap().locked(&key, now()) {
            let mut r = deny(StatusCode::TOO_MANY_REQUESTS, &format!("Trop d'essais de mot de passe. Réessayez dans {} min.", wait / 60 + 1));
            r.headers_mut().insert(header::RETRY_AFTER, header::HeaderValue::from(wait));
            return r;
        }
        if !basic_password_ok(auth.as_deref(), pw) {
            // Le navigateur envoie d'abord une requête sans identifiants : ce n'est pas une erreur de mot de passe.
            if auth.is_some() {
                st.logins.lock().unwrap().fail(&key, now());
                st.log("avert", format!("Mot de passe refusé depuis {key}"));
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            let mut r = deny(StatusCode::UNAUTHORIZED, "Mot de passe requis.");
            r.headers_mut().insert(header::WWW_AUTHENTICATE, header::HeaderValue::from_static("Basic realm=\"kesal-rangeur\", charset=\"UTF-8\""));
            return r;
        }
        st.logins.lock().unwrap().ok(&key);
    }
    if mutating && !origin_ok(origin.as_deref(), &host) {
        return deny(StatusCode::FORBIDDEN, "Requête refusée : elle ne vient pas de cette interface.");
    }
    let mut r = next.run(req).await;
    let h = r.headers_mut();
    h.insert(header::X_FRAME_OPTIONS, header::HeaderValue::from_static("DENY"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, header::HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, header::HeaderValue::from_static("no-referrer"));
    h.entry(header::CACHE_CONTROL).or_insert(header::HeaderValue::from_static("no-store"));
    r
}

/// Lance une analyse en la protégeant d'un plantage : l'état « en cours » ne reste jamais bloqué.
async fn run_scan_guarded(st: S) {
    let st2 = st.clone();
    if let Err(e) = tokio::spawn(run_scan(st2)).await {
        {
            let mut sc = st.scan.lock().unwrap();
            sc.running = false;
            sc.error = Some("l'analyse s'est interrompue de façon inattendue".into());
        }
        st.log("erreur", format!("Analyse interrompue : {e}"));
    }
}

/// Retire de temps en temps les dossiers provisoires abandonnés (arrêt brutal pendant une copie).
async fn sweep_loop(st: S) {
    loop {
        let (roots, essai) = {
            let c = st.cfg.read().await;
            let mut r: Vec<PathBuf> = c.destinations.values().cloned().collect();
            r.extend(c.film_libraries.iter().cloned());
            (r, c.essai)
        };
        if !essai {
            if let Ok(n) = tokio::task::spawn_blocking(move || exec::sweep_staging(&roots, 1800)).await {
                if n > 0 {
                    st.log("info", format!("{n} dossier(s) provisoire(s) abandonné(s) nettoyé(s)"));
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotes_autorises() {
        let extra = vec!["range.example.org".to_string()];
        for h in ["192.168.1.10:3050", "127.0.0.1:3050", "localhost:3050", "[::1]:3050", "rangeur:3050", "nas.local", "mon-nas.lan:3050", "range.example.org", "RANGE.example.org:443"] {
            assert!(host_allowed(h, &extra), "{h} devrait être autorisé");
        }
        for h in ["evil.com", "evil.com:3050", "192.168.1.10.evil.com", "attacker.example.net:3050", ""] {
            assert!(!host_allowed(h, &extra), "{h} devrait être refusé");
        }
    }

    #[test]
    fn origine_doit_correspondre() {
        assert!(origin_ok(None, "192.168.1.10:3050"));
        assert!(origin_ok(Some("http://192.168.1.10:3050"), "192.168.1.10:3050"));
        assert!(!origin_ok(Some("http://evil.com"), "192.168.1.10:3050"));
        assert!(!origin_ok(Some("https://192.168.1.10:3050.evil.com"), "192.168.1.10:3050"));
    }

    #[test]
    fn blocage_apres_cinq_erreurs() {
        let mut g = LoginGuard::default();
        for i in 0..4 {
            g.fail("1.2.3.4", 100 + i);
            assert!(g.locked("1.2.3.4", 110).is_none());
        }
        g.fail("1.2.3.4", 105);
        assert!(g.locked("1.2.3.4", 110).is_some());
        assert!(g.locked("5.6.7.8", 110).is_none(), "les autres adresses ne sont pas touchées");
        assert!(g.locked("1.2.3.4", 105 + 901).is_none(), "le blocage se lève au bout de 15 min");
        g.fail("9.9.9.9", 0);
        g.ok("9.9.9.9");
        for _ in 0..4 { g.fail("9.9.9.9", 1); }
        assert!(g.locked("9.9.9.9", 2).is_none(), "un bon mot de passe remet le compteur à zéro");
    }

    #[test]
    fn adresse_du_client() {
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let pub_ip: IpAddr = "8.8.8.8".parse().unwrap();
        assert_eq!(client_key(lo, Some("203.0.113.7, 10.0.0.1")), "203.0.113.7");
        assert_eq!(client_key(lo, Some("pas-une-ip")), "127.0.0.1");
        assert_eq!(client_key(pub_ip, Some("203.0.113.7")), "8.8.8.8", "un client distant ne peut pas se faire passer pour un autre");
    }

    #[test]
    fn mot_de_passe_basic() {
        // « user:secret » en base64 = dXNlcjpzZWNyZXQ=
        assert!(basic_password_ok(Some("Basic dXNlcjpzZWNyZXQ="), "secret"));
        assert!(!basic_password_ok(Some("Basic dXNlcjpzZWNyZXQ="), "autre"));
        assert!(!basic_password_ok(None, "secret"));
        assert!(!basic_password_ok(Some("Bearer xxx"), "secret"));
        assert!(!basic_password_ok(Some("Basic !!!"), "secret"));
        assert_eq!(b64_decode("YWJj").unwrap(), b"abc");
        assert_eq!(b64_decode("YWI=").unwrap(), b"ab");
    }
}
