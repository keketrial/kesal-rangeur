//! Essai à blanc : décide, pour chaque élément qui arrive, ce qui serait fait. Ne touche à rien.

use crate::config::{Category, Config, Input};
use crate::library::{self, LibEntry};
use crate::parser::{self, Release};
use crate::tmdb::{self, Candidate, Media, Tmdb};
use crate::naming::{self, Formats};
use crate::norm;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

const TEMP_EXT: &[&str] = &["!sync", "part", "!qb", "tmp", "crdownload", "partial", "download"];

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "etat", rename_all = "snake_case")]
pub enum Settle {
    Pret,
    /// Transfert en cours (fichier temporaire présent).
    Transfert { fichier: String },
    /// Modifié récemment : encore `minutes` minutes à attendre.
    Attente { minutes: u64 },
}

#[cfg(unix)]
fn ctime_of(md: &std::fs::Metadata) -> Option<SystemTime> {
    use std::os::unix::fs::MetadataExt;
    let secs = u64::try_from(md.ctime()).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}
#[cfg(not(unix))]
fn ctime_of(_md: &std::fs::Metadata) -> Option<SystemTime> {
    None
}

/// Un élément est prêt si aucun fichier temporaire n'est présent et que rien n'a changé depuis `settle_minutes`.
pub fn check_settled(path: &Path, settle_minutes: u64) -> Settle {
    let mut newest = SystemTime::UNIX_EPOCH;
    for e in WalkDir::new(path) {
        let Ok(e) = e else {
            return Settle::Transfert { fichier: "dossier illisible (droits ?)".into() };
        };
        let name = e.file_name().to_string_lossy().to_lowercase();
        if let Some(ext) = name.rsplit_once('.').map(|x| x.1) {
            if TEMP_EXT.contains(&ext) {
                return Settle::Transfert { fichier: e.file_name().to_string_lossy().to_string() };
            }
        }
        if let Ok(md) = e.metadata() {
            if let Ok(m) = md.modified() {
                // Resilio garde la date d'origine : une date dans le futur (horloge de la seedbox en avance) ne doit pas bloquer
                // éternellement. On prend alors la date de changement locale (ctime), qui est le vrai moment d'arrivée.
                let m = if m > SystemTime::now() + Duration::from_secs(60) { ctime_of(&md).unwrap_or_else(SystemTime::now) } else { m };
                newest = newest.max(m);
            }
        }
    }
    let age = SystemTime::now().duration_since(newest).unwrap_or(Duration::ZERO);
    let need = Duration::from_secs(settle_minutes * 60);
    if age >= need {
        Settle::Pret
    } else {
        Settle::Attente { minutes: ((need - age).as_secs() + 59) / 60 }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Reconnu {
    pub tmdb_id: u64,
    pub media: Media,
    pub titre: String,
    pub titre_original: String,
    pub annee: Option<u16>,
    pub categorie: Category,
    pub affiche: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Sera rangé : `vers` est le chemin final.
    Ranger {
        vers: PathBuf,
        nouveau_dossier: bool,
        /// Saison du nom d'origine → saison de la fiche TMDB (ex. S02 → S01 pour « Nordholm 2 »).
        renumerotation: Option<(u16, u16)>,
        renommages: Vec<Renommage>,
        /// Chemins exacts (nommage activé) ; vide = noms d'origine conservés.
        fichiers: Vec<Placement>,
    },
    /// Déjà présent.
    Doublon { existant: PathBuf, raison: String },
    /// Doute : à valider à la main.
    ATrier { raison: String, propositions: Vec<String> },
    /// Pas encore prêt.
    Attendre { raison: String },
    /// Rien à faire (fichier parasite…).
    Ignorer { raison: String },
    /// Déjà rangé pendant cette session.
    Fait { vers: PathBuf },
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub source: PathBuf,
    /// Dossier d'arrivée dont l'élément provient.
    pub entree: PathBuf,
    pub nom: String,
    pub release: Release,
    pub reconnu: Option<Reconnu>,
    /// Fiches TMDB proposées (pour « À trier » et pour l'affiche).
    pub candidats: Vec<Candidate>,
    pub decision: Decision,
}

/// Index des films déjà rangés (pour les doublons).
#[derive(Clone)]
pub struct MovieIndex {
    pub entries: Vec<(String, Option<u16>, u32, PathBuf)>, // titre, année, qualité, chemin
}

impl MovieIndex {
    pub fn scan(roots: &[PathBuf]) -> MovieIndex {
        let mut entries = Vec::new();
        for root in roots {
            let Ok(rd) = std::fs::read_dir(root) else { continue };
            for e in rd.filter_map(|e| e.ok()) {
                let name = e.file_name().to_string_lossy().to_string();
                if library::is_junk_name(&name) {
                    continue;
                }
                let p = e.path();
                if p.is_file() && !p.extension().and_then(|x| x.to_str()).map_or(false, parser::is_video_ext) {
                    continue;
                }
                let r = parser::parse(&name);
                if r.title.is_empty() {
                    continue;
                }
                entries.push((r.title.clone(), r.year, r.quality_score(), p));
            }
        }
        MovieIndex { entries }
    }
    /// Cherche un film déjà présent qui porte l'un des titres donnés (même année à 1 près).
    pub fn find(&self, titles: &[&str], year: Option<u16>) -> Option<&(String, Option<u16>, u32, PathBuf)> {
        self.entries.iter().find(|(t, y, _, _)| {
            let year_ok = match (year, y) {
                (Some(a), Some(b)) => a.abs_diff(*b) <= 1,
                _ => true,
            };
            year_ok && titles.iter().any(|q| !q.is_empty() && norm::similarity(q, t) >= 0.93)
        })
    }
}

/// Nettoie un nom pour en faire un nom de dossier.
pub fn safe_name(s: &str) -> String {
    let cleaned = s
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // Windows/SMB : pas de point ni d'espace final, pas de nom réservé (CON, NUL…)
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    const RESERVED: &[&str] = &["con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "lpt1", "lpt2", "lpt3"];
    if RESERVED.contains(&cleaned.to_lowercase().as_str()) { format!("_{cleaned}") } else { cleaned }
}

/// Remplace le numéro de saison `from` par `to` dans un nom (S02E01 → S01E01, S02 → S01).
pub fn renumber_season(name: &str, from: u16, to: u16) -> String {
    let re = regex::Regex::new(&format!(r"(?i)(^|[^A-Za-z0-9])S0*{from}(E\d|[^A-Za-z0-9]|$)")).unwrap();
    // deux passes : des occurrences voisines (« S02.S02 ») se chevauchent sur le séparateur
    let once = |t: &str| re.replace_all(t, |c: &regex::Captures| format!("{}S{:02}{}", &c[1], to, &c[2])).to_string();
    once(&once(name))
}

/// Un fichier à ranger : d'où il vient, où il ira (chemins complets). Rempli seulement si le nommage est activé.
#[derive(Debug, Clone, Serialize)]
pub struct Placement {
    pub src: PathBuf,
    pub dst: PathBuf,
}

/// Fichiers d'un élément (chemins relatifs et tailles). Un fichier seul donne son propre nom.
fn list_files(source: &Path) -> Vec<(PathBuf, u64)> {
    if source.is_file() {
        let sz = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);
        return vec![(PathBuf::from(source.file_name().unwrap_or_default()), sz)];
    }
    WalkDir::new(source)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && !library::is_junk_name(&e.file_name().to_string_lossy()))
        .map(|e| (e.path().strip_prefix(source).unwrap_or(e.path()).to_path_buf(), e.metadata().map(|m| m.len()).unwrap_or(0)))
        .collect()
}

fn abs_src(source: &Path, rel: &Path) -> PathBuf {
    if source.is_file() { source.to_path_buf() } else { source.join(rel) }
}

/// Un renommage prévu, relatif au dossier de la release.
#[derive(Debug, Clone, Serialize)]
pub struct Renommage {
    pub ancien: String,
    pub nouveau: String,
}

/// Liste des fichiers vidéo à renommer dans un dossier de release (chemins relatifs).
fn renames_for(dir: &Path, from: u16, to: u16) -> Vec<Renommage> {
    let mut out = Vec::new();
    for e in WalkDir::new(dir).min_depth(1).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()) {
        let rel = e.path().strip_prefix(dir).unwrap_or(e.path()).to_string_lossy().to_string();
        let nouveau = renumber_season(&rel, from, to);
        if nouveau != rel {
            out.push(Renommage { ancien: rel, nouveau });
        }
    }
    out
}

/// Choix fait à la main (« À trier ») : retenu pour les prochaines fois.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub media: Media,
    pub id: u64,
    pub category: Option<Category>,
}

/// Clé d'un choix retenu : titre + année + (pour une saison seule) le numéro de saison. Sans la saison, un choix
/// fait pour « Nordholm S01 » s'appliquerait aussi à S02, S03 et S04, qui sont d'autres fiches.
pub fn choice_key(rel: &Release) -> String {
    let kind = match rel.seasons.as_slice() {
        [s] => format!("tv:S{s}"),
        _ if rel.is_series_like() => "tv".to_string(),
        _ => "any".to_string(),
    };
    format!("{}|{}|{}", norm::key(&rel.title), rel.year.map(|y| y.to_string()).unwrap_or_default(), kind)
}

/// Clé « série » d'un choix (sans la saison) : réutilisée si la fiche choisie possède la saison qui arrive.
pub fn series_key(rel: &Release) -> String {
    format!("{}|{}|tv", norm::key(&rel.title), rel.year.map(|y| y.to_string()).unwrap_or_default())
}

pub struct Planner {
    pub cfg: Arc<Config>,
    pub tmdb: Arc<Tmdb>,
    movies: MovieIndex,
    pub series: BTreeMap<Category, Vec<LibEntry>>,
    choices: Mutex<HashMap<String, Choice>>,
    choices_path: PathBuf,
}

fn category_from_mode(mode: &str) -> Option<Category> {
    serde_json::from_value(serde_json::Value::String(mode.to_string())).ok()
}

impl Planner {
    /// Lit les bibliothèques existantes (peut prendre une vingtaine de secondes sur un NAS : à lancer hors du thread async).
    pub fn new(cfg: Arc<Config>, tmdb: Arc<Tmdb>) -> Planner {
        let mut roots: Vec<PathBuf> = cfg.film_libraries.clone();
        if let Some(d) = cfg.destinations.get(&Category::Film) {
            roots.push(d.clone());
        }
        let mut series = BTreeMap::new();
        for (cat, dir) in &cfg.destinations {
            if *cat != Category::Film {
                series.insert(*cat, library::scan_series(dir).unwrap_or_default());
            }
        }
        let choices_path = cfg.data_dir.join("choices.json");
        let choices = std::fs::read_to_string(&choices_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let movies = MovieIndex::scan(&roots);
        Planner { cfg, tmdb, movies, series, choices: Mutex::new(choices), choices_path }
    }

    /// Même bibliothèques déjà lues, autres réglages (sert aux essais de configuration : pas de nouvelle lecture du NAS).
    pub fn variant(&self, cfg: Arc<Config>, tmdb: Arc<Tmdb>, rescan_films: bool) -> Planner {
        let movies = if rescan_films {
            let mut roots: Vec<PathBuf> = cfg.film_libraries.clone();
            if let Some(d) = cfg.destinations.get(&Category::Film) {
                roots.push(d.clone());
            }
            MovieIndex::scan(&roots)
        } else {
            self.movies.clone()
        };
        Planner { choices_path: self.choices_path.clone(), choices: Mutex::new(self.choices.lock().unwrap().clone()), series: self.series.clone(), movies, cfg, tmdb }
    }

    pub fn movie_count(&self) -> usize {
        self.movies.entries.len()
    }

    pub fn set_choice(&self, rel: &Release, ch: Choice) {
        let mut m = self.choices.lock().unwrap();
        if ch.media == Media::Tv && rel.seasons.len() == 1 {
            m.insert(series_key(rel), ch.clone()); // la prochaine saison de cette série ne redemandera pas
        }
        m.insert(choice_key(rel), ch);
        if let Ok(t) = serde_json::to_string_pretty(&*m) {
            let _ = std::fs::create_dir_all(&self.cfg.data_dir);
            let _ = std::fs::write(&self.choices_path, t);
        }
    }

    pub fn choices_count(&self) -> usize {
        self.choices.lock().unwrap().len()
    }

    fn rule_category(&self, titles: &[&str]) -> Option<Category> {
        self.cfg.rules.iter().find_map(|r| titles.iter().any(|t| norm::similarity(t, &r.title) >= 0.95).then_some(r.category))
    }

    /// Cherche le dossier de série existant qui correspond (toutes catégories confondues).
    fn find_series(&self, titles: &[&str], year: Option<u16>) -> Option<&LibEntry> {
        let mut best: Option<(&LibEntry, f32)> = None;
        for list in self.series.values() {
            for e in list {
                if e.title.is_empty() {
                    continue;
                }
                let year_ok = match (year, e.year) {
                    (Some(a), Some(b)) => a.abs_diff(b) <= 1,
                    _ => true,
                };
                if !year_ok {
                    continue;
                }
                let s = titles.iter().map(|t| norm::similarity(t, &e.title)).fold(0.0, f32::max);
                if s >= 0.9 && best.map_or(true, |(_, bs)| s > bs) {
                    best = Some((e, s));
                }
            }
        }
        best.map(|(e, _)| e)
    }

    /// Film déjà présent ? On essaie l'année de TMDB puis celle du nom de fichier (les fichiers portent souvent une autre année).
    fn find_movie(&self, names: &[&str], tmdb_year: Option<u16>, rel_year: Option<u16>) -> Option<&(String, Option<u16>, u32, PathBuf)> {
        self.movies.find(names, tmdb_year.or(rel_year)).or_else(|| match (rel_year, tmdb_year) {
            (Some(r), Some(t)) if r != t => self.movies.find(names, Some(r)),
            _ => None,
        })
    }

    /// Même chose pour les dossiers de série.
    fn find_series_years(&self, names: &[&str], tmdb_year: Option<u16>, rel_year: Option<u16>) -> Option<&LibEntry> {
        self.find_series(names, tmdb_year.or(rel_year)).or_else(|| match (rel_year, tmdb_year) {
            (Some(r), Some(t)) if r != t => self.find_series(names, Some(r)),
            _ => None,
        })
    }

    pub async fn plan_input(&self, input: &Input) -> anyhow::Result<Vec<Item>> {
        let mut names: Vec<_> = std::fs::read_dir(&input.path)?.filter_map(|e| e.ok()).collect();
        names.sort_by_key(|e| e.file_name());
        let mut items = Vec::new();
        for e in names {
            items.push(self.plan_one(input, &e.path()).await);
        }
        Ok(items)
    }

    /// Décide pour un seul élément du dossier d'arrivée.
    pub async fn plan_one(&self, input: &Input, path: &Path) -> Item {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let release = parser::parse(&name);
        let mut item = Item {
            source: path.to_path_buf(),
            entree: input.path.clone(),
            nom: name.clone(),
            release,
            reconnu: None,
            candidats: vec![],
            decision: Decision::Ignorer { raison: String::new() },
        };
        if library::is_junk_name(&name) {
            item.decision = Decision::Ignorer { raison: "fichier système".into() };
            return item;
        }
        if std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            item.decision = Decision::Ignorer { raison: "lien symbolique (ignoré par sécurité)".into() };
            return item;
        }
        if path.is_file() && !path.extension().and_then(|x| x.to_str()).map_or(false, parser::is_video_ext) {
            item.decision = Decision::Ignorer { raison: "pas une vidéo".into() };
            return item;
        }
        match tokio::task::block_in_place(|| check_settled(path, input.settle_minutes)) {
            Settle::Pret => {}
            Settle::Transfert { fichier } => {
                item.decision = Decision::Attendre { raison: format!("transfert en cours ({fichier})") };
                return item;
            }
            Settle::Attente { minutes } => {
                item.decision = Decision::Attendre { raison: format!("modifié récemment, encore {minutes} min") };
                return item;
            }
        }
        item.decision = self.decide(&mut item, input).await;
        item
    }

    /// Film : nom et dossier calculés d'après le format choisi.
    fn name_movie(&self, item: &Item, c: &Candidate, fmt: &Formats, dest_root: &Path, props: &[String]) -> Decision {
        let files = tokio::task::block_in_place(|| list_files(&item.source));
        let (map, _ignored) = match naming::map_movie(&files, fmt, &c.title, c.year, &item.release) {
            Ok(x) => x,
            Err(e) => return Decision::ATrier { raison: format!("nommage impossible : {e}"), propositions: props.to_vec() },
        };
        let fichiers: Vec<Placement> = map.iter().map(|(rel, dst_rel)| Placement { src: abs_src(&item.source, rel), dst: dest_root.join(dst_rel) }).collect();
        if fichiers.iter().all(|f| f.dst.exists()) {
            return Decision::Doublon { existant: fichiers[0].dst.clone(), raison: "déjà présent (même nom)".into() };
        }
        if let Some(f) = fichiers.iter().find(|f| f.dst.exists()) {
            return Decision::ATrier { raison: format!("un fichier existe déjà : {}", f.dst.display()), propositions: props.to_vec() };
        }
        let first = map[0].1.components().next().map(|c| c.as_os_str().to_owned()).unwrap_or_default();
        let vers = if map[0].1.components().count() > 1 { dest_root.join(first) } else { fichiers[0].dst.clone() };
        let renommages = map.iter().map(|(rel, dst)| Renommage { ancien: rel.to_string_lossy().to_string(), nouveau: dst.to_string_lossy().to_string() }).collect();
        Decision::Ranger { vers, nouveau_dossier: false, renumerotation: None, renommages, fichiers }
    }

    /// Série : chaque épisode est placé dans son dossier de saison avec son nouveau nom (titres d'épisodes TMDB).
    #[allow(clippy::too_many_arguments)]
    async fn name_series(&self, item: &Item, c: &Candidate, fmt: &Formats, base: PathBuf, nouveau: bool, renum: Option<(u16, u16)>, existing_style: Option<String>, props: &[String]) -> Decision {
        let files: Vec<PathBuf> = tokio::task::block_in_place(|| list_files(&item.source)).into_iter().map(|(p, _)| p).collect();
        // saisons dont on a besoin (après renumérotation) pour récupérer les titres d'épisodes
        let hint = if let [s] = item.release.seasons.as_slice() { Some(*s) } else { None };
        let mut wanted = std::collections::BTreeSet::new();
        for f in &files {
            if let Some((s0, _)) = naming::episode_of(f, hint) {
                wanted.insert(match renum { Some((from, to)) if from == s0 => to, _ => s0 });
            }
        }
        let mut titles: std::collections::HashMap<u16, std::collections::HashMap<u16, String>> = Default::default();
        for s in wanted {
            if let Ok(t) = self.tmdb.season_titles(c.id, s).await {
                titles.insert(s, t);
            }
        }
        let (map, _ignored) = match naming::map_series(&files, &naming::SeriesIn { fmt, name: &c.title, year: c.year, release: &item.release, renum, existing_style, titles: &titles }) {
            Ok(x) => x,
            Err(e) => return Decision::ATrier { raison: format!("nommage impossible : {e}"), propositions: props.to_vec() },
        };
        let fichiers: Vec<Placement> = map.iter().map(|(rel, dst_rel)| Placement { src: abs_src(&item.source, rel), dst: base.join(dst_rel) }).collect();
        if fichiers.iter().all(|f| f.dst.exists()) {
            return Decision::Doublon { existant: base, raison: "tous les épisodes existent déjà (même nom)".into() };
        }
        if let Some(f) = fichiers.iter().find(|f| f.dst.exists()) {
            return Decision::ATrier { raison: format!("un fichier existe déjà : {}", f.dst.display()), propositions: props.to_vec() };
        }
        let renommages = map.iter().map(|(rel, dst)| Renommage { ancien: rel.to_string_lossy().to_string(), nouveau: dst.to_string_lossy().to_string() }).collect();
        Decision::Ranger { vers: base, nouveau_dossier: nouveau, renumerotation: renum, renommages, fichiers }
    }

    async fn decide(&self, item: &mut Item, input: &Input) -> Decision {
        let rel = item.release.clone();
        if rel.title.is_empty() {
            return Decision::ATrier { raison: "titre illisible dans le nom".into(), propositions: vec![] };
        }
        // Un choix déjà fait à la main passe avant tout (par saison, ou pour la série si la fiche a cette saison).
        let (per_season, per_series) = {
            let m = self.choices.lock().unwrap();
            (m.get(&choice_key(&rel)).cloned(), m.get(&series_key(&rel)).cloned())
        };
        let mut choice = per_season;
        if choice.is_none() {
            if let (Some(ch), [season]) = (per_series.as_ref(), rel.seasons.as_slice()) {
                if let Ok(list) = self.tmdb.tv_seasons(ch.id).await {
                    if list.contains(season) {
                        choice = per_series.clone();
                    }
                }
            }
        }
        // Déjà dans la bibliothèque ? Le nom et l'année suffisent, sans même interroger TMDB (et sans ambiguïté d'homonymes).
        if choice.is_none() && per_series.is_none() {
            if rel.is_movie_like() {
                if let Some((_, _, q, p)) = self.movies.find(&[rel.title.as_str()], rel.year) {
                    let raison = if rel.quality_score() > *q + 2 { "déjà présent (la nouvelle version est de meilleure qualité)" } else { "déjà présent" };
                    return Decision::Doublon { existant: p.clone(), raison: raison.into() };
                }
            } else if !rel.seasons.is_empty() && rel.episodes.is_empty() {
                if let Some(lib) = self.find_series(&[rel.title.as_str()], rel.year) {
                    if rel.seasons.iter().all(|s| lib.seasons.contains(s)) {
                        return Decision::Doublon { existant: lib.path.clone(), raison: format!("saison(s) {:?} déjà présente(s)", rel.seasons) };
                    }
                }
            }
        }
        let cands: Vec<Candidate> = if let Some(ch) = &choice {
            match self.tmdb.details(ch.media, ch.id).await {
                Ok(mut c) => {
                    c.sim = 1.0;
                    c.score = 1.0;
                    vec![c]
                }
                Err(e) => return Decision::ATrier { raison: format!("TMDB : {e}"), propositions: vec![] },
            }
        } else {
            match self.tmdb.identify(&rel).await {
                Ok(c) => c,
                Err(e) => return Decision::ATrier { raison: format!("TMDB : {e}"), propositions: vec![] },
            }
        };
        item.candidats = cands.clone();
        let props: Vec<String> = cands
            .iter()
            .take(4)
            .map(|c| format!("{} ({}) [{}] id {}", c.title, c.year.map(|y| y.to_string()).unwrap_or("?".into()), if c.media == Media::Tv { "série" } else { "film" }, c.id))
            .collect();
        if cands.is_empty() || !tmdb::is_confident(&cands) {
            // Série dont le dossier existe déjà dans la bibliothèque : lui seul tranche (homonymes, titre introuvable…).
            // Seulement si on garde les noms d'origine : renommer exige la bonne fiche pour les titres d'épisodes.
            if rel.is_series_like() && self.cfg.naming.formats().is_none() {
                if let Some(lib) = self.find_series(&[rel.title.as_str()], rel.year) {
                    if !rel.seasons.is_empty() && rel.episodes.is_empty() && rel.seasons.iter().all(|s| lib.seasons.contains(s)) {
                        return Decision::Doublon { existant: lib.path.clone(), raison: format!("saison(s) {:?} déjà présente(s)", rel.seasons) };
                    }
                    return Decision::Ranger { vers: lib.path.join(&item.nom), nouveau_dossier: false, renumerotation: None, renommages: vec![], fichiers: vec![] };
                }
            }
            if cands.is_empty() {
                return Decision::ATrier { raison: "aucune fiche TMDB trouvée".into(), propositions: vec![] };
            }
            return Decision::ATrier { raison: "plusieurs fiches possibles ou titre trop différent".into(), propositions: props };
        }
        let c = &cands[0];
        let names = [c.title.as_str(), c.original_title.as_str(), rel.title.as_str()];
        let mut cat = category_from_mode(&input.mode).unwrap_or_else(|| c.category());
        if let Some(r) = self.rule_category(&names) {
            cat = r;
        }
        if let Some(fc) = choice.as_ref().and_then(|c| c.category) {
            cat = fc;
        }
        item.reconnu = Some(Reconnu { tmdb_id: c.id, media: c.media, titre: c.title.clone(), titre_original: c.original_title.clone(), annee: c.year, categorie: cat, affiche: c.poster.clone() });
        let dest_root = self.cfg.destinations.get(&cat);
        let no_dest = || Decision::ATrier { raison: format!("pas de dossier de destination pour « {} »", cat.label()), propositions: props.clone() };

        if c.media == Media::Movie {
            if let Some((_, _, q, p)) = self.find_movie(&names, c.year, rel.year) {
                let raison = if rel.quality_score() > *q + 2 { "déjà présent (la nouvelle version est de meilleure qualité)" } else { "déjà présent" };
                return Decision::Doublon { existant: p.clone(), raison: raison.into() };
            }
            let Some(dest_root) = dest_root else { return no_dest() };
            if let Some(fmt) = self.cfg.naming.formats() {
                return self.name_movie(item, c, &fmt, dest_root, &props);
            }
            return Decision::Ranger { vers: dest_root.join(&item.nom), nouveau_dossier: false, renumerotation: None, renommages: vec![], fichiers: vec![] };
        }

        // Série. Une fiche TMDB qui n'a qu'une saison (téléfilms numérotés, « Nordholm 2 ») alors que la release
        // annonce S02, S03… : on garde la fiche et on renumérote en S01.
        let mut renum: Option<(u16, u16)> = None;
        if let [n] = rel.seasons.as_slice() {
            if *n > 1 {
                match self.tmdb.tv_seasons(c.id).await {
                    Ok(list) if !list.contains(n) => {
                        if list == [1] {
                            renum = Some((*n, 1));
                        } else {
                            return Decision::ATrier { raison: format!("la fiche TMDB n'a pas de saison {n} (saisons : {list:?})"), propositions: props };
                        }
                    }
                    Ok(_) => {}
                    Err(e) => return Decision::ATrier { raison: format!("TMDB : {e}"), propositions: props },
                }
            }
        }
        let seasons: Vec<u16> = match renum {
            Some((_, to)) => vec![to],
            None => rel.seasons.clone(),
        };
        let (final_name, renommages) = match renum {
            Some((from, to)) => (renumber_season(&item.nom, from, to), renames_for(&item.source, from, to)),
            None => (item.nom.clone(), vec![]),
        };
        match self.find_series_years(&names, c.year, rel.year) {
            Some(lib) => {
                if !seasons.is_empty() && rel.episodes.is_empty() && seasons.iter().all(|s| lib.seasons.contains(s)) {
                    return Decision::Doublon { existant: lib.path.clone(), raison: format!("saison(s) {seasons:?} déjà présente(s)") };
                }
                if !rel.episodes.is_empty() && seasons.len() == 1 && rel.episodes.iter().all(|e| lib.episodes.contains(&(seasons[0], *e))) {
                    return Decision::Doublon { existant: lib.path.clone(), raison: "épisode déjà présent".into() };
                }
                if seasons.is_empty() && rel.complete && !lib.seasons.is_empty() {
                    // intégrale : déjà présente si la bibliothèque a toutes les saisons de la fiche TMDB
                    let have: Vec<u16> = lib.seasons.iter().copied().collect();
                    return match self.tmdb.tv_seasons(c.id).await {
                        Ok(all) if !all.is_empty() && all.iter().all(|s| lib.seasons.contains(s)) => Decision::Doublon { existant: lib.path.clone(), raison: format!("intégrale déjà présente (saisons {have:?})") },
                        Ok(all) => {
                            let missing: Vec<u16> = all.into_iter().filter(|s| !lib.seasons.contains(s)).collect();
                            Decision::ATrier { raison: format!("intégrale reçue ; la série existe déjà (saisons {have:?}, il manque {missing:?})"), propositions: vec![lib.path.display().to_string()] }
                        }
                        Err(_) => Decision::ATrier { raison: format!("intégrale reçue, mais la série existe déjà (saisons {have:?})"), propositions: vec![lib.path.display().to_string()] },
                    };
                }
                if let Some(fmt) = self.cfg.naming.formats() {
                    return self.name_series(item, c, &fmt, lib.path.clone(), false, renum, Some(naming::detect_season_style(&lib.path)).flatten(), &props).await;
                }
                Decision::Ranger { vers: lib.path.join(&final_name), nouveau_dossier: false, renumerotation: renum, renommages, fichiers: vec![] }
            }
            None => {
                let Some(dest_root) = dest_root else { return no_dest() };
                let dir = match c.year {
                    Some(y) => format!("{} ({y})", safe_name(&c.title)),
                    None => safe_name(&c.title),
                };
                if let Some(fmt) = self.cfg.naming.formats() {
                    let ctx = naming::Ctx { n: c.title.clone(), y: c.year, ..Default::default() };
                    let folder = naming::render(&fmt.series_folder, &ctx).map(|v| v.join(" ")).unwrap_or(dir.clone());
                    return self.name_series(item, c, &fmt, dest_root.join(folder), true, renum, None, &props).await;
                }
                Decision::Ranger { vers: dest_root.join(dir).join(&final_name), nouveau_dossier: true, renumerotation: renum, renommages, fichiers: vec![] }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noms_windows() {
        assert_eq!(safe_name("Titre..."), "Titre");
        assert_eq!(safe_name("Qui ?  "), "Qui");
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("Mission: Impossible"), "Mission Impossible");
    }

    #[cfg(unix)]
    #[test]
    fn date_dans_le_futur_ne_bloque_pas_a_jamais() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("a.mkv");
        std::fs::write(&f, "x").unwrap();
        // ctime = maintenant (fichier tout juste créé) : le futur lointain de mtime est ignoré, on attend donc « normalement »
        let fut = SystemTime::now() + Duration::from_secs(6 * 3600);
        std::fs::OpenOptions::new().write(true).open(&f).unwrap().set_modified(fut).unwrap();
        match check_settled(&f, 30) {
            Settle::Attente { minutes } => assert!(minutes <= 30, "attente bornée par le délai : {minutes}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(check_settled(&f, 0), Settle::Pret), "avec 0 minute de calme, un fichier à date future est prêt");
    }

    #[test]
    fn choix_retenu_distingue_les_saisons() {
        let a = parser::parse("Meurtres.à.Nordholm.S01.FRENCH.1080p-X");
        let b = parser::parse("Meurtres.à.Nordholm.S02.FRENCH.1080p-X");
        let film = parser::parse("Verity.2026.MULTi.1080p-X.mkv");
        assert_ne!(choice_key(&a), choice_key(&b));
        assert_eq!(choice_key(&a), choice_key(&parser::parse("Meurtres.à.Nordholm.S01.MULTi.720p-Y")));
        assert!(choice_key(&film).ends_with("|any"));
    }

    #[test]
    fn renumerotation_des_noms() {
        assert_eq!(
            renumber_season("Meurtres.à.Nordholm.S02E01.Une.famille.disparue.FRENCH-55H.mkv", 2, 1),
            "Meurtres.à.Nordholm.S01E01.Une.famille.disparue.FRENCH-55H.mkv"
        );
        assert_eq!(renumber_season("Meurtres.à.Nordholm.S02.FRENCH.1080p-55H", 2, 1), "Meurtres.à.Nordholm.S01.FRENCH.1080p-55H");
        // dossier/fichier : les deux numéros changent
        assert_eq!(renumber_season("Show.S02/Show.S02E02.mkv", 2, 1), "Show.S01/Show.S01E02.mkv");
        // pas de faux positif
        assert_eq!(renumber_season("Show.S20E01.mkv", 2, 1), "Show.S20E01.mkv");
        assert_eq!(renumber_season("Les.S2.Aventures.S03E01.mkv", 3, 1), "Les.S2.Aventures.S01E01.mkv");
    }
}
