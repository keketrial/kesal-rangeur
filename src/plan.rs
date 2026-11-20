//! Essai à blanc : décide, pour chaque élément qui arrive, ce qui serait fait. Ne touche à rien.

use crate::config::{Category, Config, Input};
use crate::library::{self, LibEntry};
use crate::parser::{self, Release};
use crate::tmdb::{self, Candidate, Media, Tmdb};
use crate::norm;
use serde::Serialize;
use std::collections::BTreeMap;
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

/// Un élément est prêt si aucun fichier temporaire n'est présent et que rien n'a changé depuis `settle_minutes`.
pub fn check_settled(path: &Path, settle_minutes: u64) -> Settle {
    let mut newest = SystemTime::UNIX_EPOCH;
    for e in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if let Some(ext) = name.rsplit_once('.').map(|x| x.1) {
            if TEMP_EXT.contains(&ext) {
                return Settle::Transfert { fichier: e.file_name().to_string_lossy().to_string() };
            }
        }
        if let Ok(m) = e.metadata().map_err(std::io::Error::from).and_then(|m| m.modified()) {
            newest = newest.max(m);
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Sera rangé : `vers` est le chemin final.
    Ranger { vers: PathBuf, nouveau_dossier: bool },
    /// Déjà présent.
    Doublon { existant: PathBuf, raison: String },
    /// Doute : à valider à la main.
    ATrier { raison: String, propositions: Vec<String> },
    /// Pas encore prêt.
    Attendre { raison: String },
    /// Rien à faire (fichier parasite…).
    Ignorer { raison: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub source: PathBuf,
    pub nom: String,
    pub release: Release,
    pub reconnu: Option<Reconnu>,
    pub decision: Decision,
}

/// Index des films déjà rangés (pour les doublons).
pub struct MovieIndex {
    entries: Vec<(String, Option<u16>, u32, PathBuf)>, // titre, année, qualité, chemin
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
    s.chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct Planner<'a> {
    pub cfg: &'a Config,
    pub tmdb: &'a Tmdb,
    movies: MovieIndex,
    series: BTreeMap<Category, Vec<LibEntry>>,
}

fn category_from_mode(mode: &str) -> Option<Category> {
    serde_json::from_value(serde_json::Value::String(mode.to_string())).ok()
}

impl<'a> Planner<'a> {
    pub fn new(cfg: &'a Config, tmdb: &'a Tmdb) -> Planner<'a> {
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
        Planner { cfg, tmdb, movies: MovieIndex::scan(&roots), series }
    }

    fn rule_category(&self, titles: &[&str]) -> Option<Category> {
        self.cfg.rules.iter().find_map(|r| {
            titles.iter().any(|t| norm::similarity(t, &r.title) >= 0.95).then_some(r.category)
        })
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

    pub async fn plan_input(&self, input: &Input) -> anyhow::Result<Vec<Item>> {
        let mut names: Vec<_> = std::fs::read_dir(&input.path)?.filter_map(|e| e.ok()).collect();
        names.sort_by_key(|e| e.file_name());
        let mut items = Vec::new();
        for e in names {
            let name = e.file_name().to_string_lossy().to_string();
            let path = e.path();
            let release = parser::parse(&name);
            let mut item = Item { source: path.clone(), nom: name.clone(), release: release.clone(), reconnu: None, decision: Decision::Ignorer { raison: String::new() } };
            if library::is_junk_name(&name) {
                item.decision = Decision::Ignorer { raison: "fichier système".into() };
                items.push(item);
                continue;
            }
            if path.is_file() && !path.extension().and_then(|x| x.to_str()).map_or(false, parser::is_video_ext) {
                item.decision = Decision::Ignorer { raison: "pas une vidéo".into() };
                items.push(item);
                continue;
            }
            match check_settled(&path, input.settle_minutes) {
                Settle::Pret => {}
                Settle::Transfert { fichier } => {
                    item.decision = Decision::Attendre { raison: format!("transfert en cours ({fichier})") };
                    items.push(item);
                    continue;
                }
                Settle::Attente { minutes } => {
                    item.decision = Decision::Attendre { raison: format!("modifié récemment, encore {minutes} min") };
                    items.push(item);
                    continue;
                }
            }
            item.decision = self.decide(&mut item, input).await;
            items.push(item);
        }
        Ok(items)
    }

    async fn decide(&self, item: &mut Item, input: &Input) -> Decision {
        let rel = item.release.clone();
        if rel.title.is_empty() {
            return Decision::ATrier { raison: "titre illisible dans le nom".into(), propositions: vec![] };
        }
        let cands: Vec<Candidate> = match self.tmdb.identify(&rel).await {
            Ok(c) => c,
            Err(e) => return Decision::ATrier { raison: format!("TMDB : {e}"), propositions: vec![] },
        };
        let props: Vec<String> = cands
            .iter()
            .take(4)
            .map(|c| format!("{} ({}) [{}] id {}", c.title, c.year.map(|y| y.to_string()).unwrap_or("?".into()), if c.media == Media::Tv { "série" } else { "film" }, c.id))
            .collect();
        if cands.is_empty() {
            return Decision::ATrier { raison: "aucune fiche TMDB trouvée".into(), propositions: vec![] };
        }
        if !tmdb::is_confident(&cands) {
            return Decision::ATrier { raison: "plusieurs fiches possibles ou titre trop différent".into(), propositions: props };
        }
        let c = &cands[0];
        let names = [c.title.as_str(), c.original_title.as_str(), rel.title.as_str()];
        let mut cat = category_from_mode(&input.mode).unwrap_or_else(|| c.category());
        if let Some(r) = self.rule_category(&names) {
            cat = r;
        }
        item.reconnu = Some(Reconnu { tmdb_id: c.id, media: c.media, titre: c.title.clone(), titre_original: c.original_title.clone(), annee: c.year, categorie: cat });
        let Some(dest_root) = self.cfg.destinations.get(&cat) else {
            return Decision::ATrier { raison: format!("pas de dossier de destination pour « {} »", cat.label()), propositions: props };
        };

        if c.media == Media::Movie {
            if let Some((_, _, q, p)) = self.movies.find(&names, c.year.or(rel.year)) {
                let raison = if rel.quality_score() > *q + 2 { "déjà présent (la nouvelle version est de meilleure qualité)" } else { "déjà présent" };
                return Decision::Doublon { existant: p.clone(), raison: raison.into() };
            }
            return Decision::Ranger { vers: dest_root.join(&item.nom), nouveau_dossier: false };
        }

        // Série
        match self.find_series(&names, c.year.or(rel.year)) {
            Some(lib) => {
                if !rel.seasons.is_empty() && rel.episodes.is_empty() && rel.seasons.iter().all(|s| lib.seasons.contains(s)) {
                    return Decision::Doublon { existant: lib.path.clone(), raison: format!("saison(s) {:?} déjà présente(s)", rel.seasons) };
                }
                if !rel.episodes.is_empty() && rel.seasons.len() == 1 && rel.episodes.iter().all(|e| lib.episodes.contains(&(rel.seasons[0], *e))) {
                    return Decision::Doublon { existant: lib.path.clone(), raison: "épisode déjà présent".into() };
                }
                if rel.seasons.is_empty() && rel.complete && !lib.seasons.is_empty() {
                    return Decision::ATrier { raison: format!("intégrale reçue, mais la série existe déjà (saisons {:?})", lib.seasons.iter().collect::<Vec<_>>()), propositions: vec![lib.path.display().to_string()] };
                }
                Decision::Ranger { vers: lib.path.join(&item.nom), nouveau_dossier: false }
            }
            None => {
                let dir = match c.year {
                    Some(y) => format!("{} ({y})", safe_name(&c.title)),
                    None => safe_name(&c.title),
                };
                Decision::Ranger { vers: dest_root.join(dir).join(&item.nom), nouveau_dossier: true }
            }
        }
    }
}
