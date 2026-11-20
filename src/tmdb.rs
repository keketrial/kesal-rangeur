//! Client TMDB : recherche, cache disque, limite de débit.

use crate::{config::Category, norm, parser::Release};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

const CACHE_TTL_SECS: u64 = 30 * 24 * 3600;
/// TMDB tolère ~40 requêtes/s ; on reste très en dessous.
const MIN_GAP: Duration = Duration::from_millis(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Media {
    Movie,
    Tv,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: u64,
    pub media: Media,
    /// Titre dans la langue choisie.
    pub title: String,
    pub original_title: String,
    pub year: Option<u16>,
    pub genres: Vec<u32>,
    pub countries: Vec<String>,
    pub poster: Option<String>,
    pub overview: String,
    pub popularity: f32,
    /// Ressemblance du titre cherché avec le titre (ou le titre original) : 0..1.
    pub sim: f32,
    /// Ressemblance + bonus d'année.
    pub score: f32,
}

impl Candidate {
    /// Catégorie déduite des genres TMDB (Animation 16, Documentaire 99, Reality 10764, Talk 10767).
    pub fn category(&self) -> Category {
        let g = |id: u32| self.genres.contains(&id);
        match self.media {
            Media::Tv if g(10764) || g(10767) => Category::Telerealite,
            _ if g(99) => Category::Documentaire,
            _ if g(16) => Category::DessinAnime,
            Media::Movie => Category::Film,
            Media::Tv => Category::Serie,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    entries: HashMap<String, (u64, Value)>,
}

pub struct Tmdb {
    http: reqwest::Client,
    secret: String,
    pub language: String,
    cache: StdMutex<Cache>,
    cache_path: PathBuf,
    last: Mutex<Instant>,
    pub requests: StdMutex<u32>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Tmdb {
    pub fn new(secret: String, language: String, data_dir: &std::path::Path) -> Result<Tmdb> {
        std::fs::create_dir_all(data_dir)?;
        let cache_path = data_dir.join("tmdb-cache.json");
        let cache = std::fs::read_to_string(&cache_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Ok(Tmdb {
            http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build()?,
            secret,
            language,
            cache: StdMutex::new(cache),
            cache_path,
            last: Mutex::new(Instant::now() - MIN_GAP),
            requests: StdMutex::new(0),
        })
    }

    pub fn save_cache(&self) {
        if let Ok(c) = self.cache.lock() {
            if let Ok(t) = serde_json::to_string(&*c) {
                let _ = std::fs::write(&self.cache_path, t);
            }
        }
    }

    /// GET avec cache. `params` ne contient jamais la clé (ajoutée ici, jamais écrite dans le cache).
    pub async fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Value> {
        let mut key = path.to_string();
        for (k, v) in params {
            key.push_str(&format!("&{k}={v}"));
        }
        if let Some((t, v)) = self.cache.lock().unwrap().entries.get(&key) {
            if now().saturating_sub(*t) < CACHE_TTL_SECS {
                return Ok(v.clone());
            }
        }
        {
            let mut last = self.last.lock().await;
            let el = last.elapsed();
            if el < MIN_GAP {
                tokio::time::sleep(MIN_GAP - el).await;
            }
            *last = Instant::now();
        }
        *self.requests.lock().unwrap() += 1;
        let url = format!("https://api.themoviedb.org/3{path}");
        let mut req = self.http.get(&url).query(params);
        if self.secret.starts_with("eyJ") {
            req = req.bearer_auth(&self.secret);
        } else {
            req = req.query(&[("api_key", self.secret.as_str())]);
        }
        let resp = req.send().await.map_err(|e| anyhow!("TMDB injoignable : {}", e.without_url()))?;
        let status = resp.status();
        if status.as_u16() == 429 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            return Err(anyhow!("TMDB : trop de requêtes (429), réessayer"));
        }
        if !status.is_success() {
            return Err(anyhow!("TMDB a répondu {status} pour {path}"));
        }
        let v: Value = resp.json().await?;
        self.cache.lock().unwrap().entries.insert(key, (now(), v.clone()));
        Ok(v)
    }

    pub async fn search_kind(&self, media: Media, query: &str) -> Result<Vec<Candidate>> {
        let (path, tkey, okey, dkey) = match media {
            Media::Movie => ("/search/movie", "title", "original_title", "release_date"),
            Media::Tv => ("/search/tv", "name", "original_name", "first_air_date"),
        };
        let v = self.get(path, &[("query", query), ("language", &self.language), ("include_adult", "false")]).await?;
        let mut out = Vec::new();
        for r in v["results"].as_array().into_iter().flatten() {
            out.push(Candidate {
                id: r["id"].as_u64().unwrap_or(0),
                media,
                title: r[tkey].as_str().unwrap_or("").to_string(),
                original_title: r[okey].as_str().unwrap_or("").to_string(),
                year: r[dkey].as_str().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()),
                genres: r["genre_ids"].as_array().into_iter().flatten().filter_map(|g| g.as_u64()).map(|g| g as u32).collect(),
                countries: r["origin_country"].as_array().into_iter().flatten().filter_map(|c| c.as_str()).map(String::from).collect(),
                poster: r["poster_path"].as_str().map(String::from),
                overview: r["overview"].as_str().unwrap_or("").to_string(),
                popularity: r["popularity"].as_f64().unwrap_or(0.0) as f32,
                sim: 0.0,
                score: 0.0,
            });
        }
        Ok(out)
    }

    /// Fiche complète d'un film ou d'une série (pour un choix déjà fait).
    pub async fn details(&self, media: Media, id: u64) -> Result<Candidate> {
        let (path, tkey, okey, dkey) = match media {
            Media::Movie => (format!("/movie/{id}"), "title", "original_title", "release_date"),
            Media::Tv => (format!("/tv/{id}"), "name", "original_name", "first_air_date"),
        };
        let r = self.get(&path, &[("language", &self.language)]).await?;
        Ok(Candidate {
            id,
            media,
            title: r[tkey].as_str().unwrap_or("").to_string(),
            original_title: r[okey].as_str().unwrap_or("").to_string(),
            year: r[dkey].as_str().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()),
            genres: r["genres"].as_array().into_iter().flatten().filter_map(|g| g["id"].as_u64()).map(|g| g as u32).collect(),
            countries: r["origin_country"].as_array().into_iter().flatten().filter_map(|c| c.as_str()).map(String::from).collect(),
            poster: r["poster_path"].as_str().map(String::from),
            overview: r["overview"].as_str().unwrap_or("").to_string(),
            popularity: r["popularity"].as_f64().unwrap_or(0.0) as f32,
            sim: 0.0,
            score: 0.0,
        })
    }

    /// Vérifie que la clé fonctionne (ignore le cache).
    pub async fn test(&self) -> Result<()> {
        self.cache.lock().unwrap().entries.remove("/configuration");
        self.get("/configuration", &[]).await.map(|_| ())
    }

    /// Titres des épisodes d'une saison (dans la langue choisie). Les titres génériques (« Épisode 3 ») sont ignorés.
    pub async fn season_titles(&self, id: u64, season: u16) -> Result<HashMap<u16, String>> {
        let v = self.get(&format!("/tv/{id}/season/{season}"), &[("language", &self.language)]).await?;
        let generic = regex::Regex::new(r"(?i)^(épisode|episode|episodio|folge|aflevering|capítulo|capitulo)\s*\d+$").unwrap();
        Ok(v["episodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| Some((e["episode_number"].as_u64()? as u16, e["name"].as_str()?.trim().to_string())))
            .filter(|(_, n)| !n.is_empty() && !generic.is_match(n))
            .collect())
    }

    /// Numéros de saison (hors épisodes spéciaux) d'une série TMDB.
    pub async fn tv_seasons(&self, id: u64) -> Result<Vec<u16>> {
        let v = self.get(&format!("/tv/{id}"), &[("language", &self.language)]).await?;
        Ok(v["seasons"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["season_number"].as_u64())
            .filter(|n| *n > 0)
            .map(|n| n as u16)
            .collect())
    }

    /// Cherche une release et classe les candidats, du meilleur au moins bon.
    pub async fn identify(&self, rel: &Release) -> Result<Vec<Candidate>> {
        let query = rel.title.clone();
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut cands = Vec::new();
        if rel.is_series_like() {
            cands.extend(self.search_kind(Media::Tv, &query).await?);
        } else if rel.is_movie_like() {
            cands.extend(self.search_kind(Media::Movie, &query).await?);
        } else {
            cands.extend(self.search_kind(Media::Movie, &query).await?);
            cands.extend(self.search_kind(Media::Tv, &query).await?);
        }
        score_candidates(rel, &mut cands);
        cands.truncate(8);
        Ok(cands)
    }
}

/// Calcule `sim` et `score` de chaque candidat et les trie, du meilleur au moins bon.
pub fn score_candidates(rel: &Release, cands: &mut Vec<Candidate>) {
    let query = rel.title.clone();
    let qkey = norm::key(&query);
    for c in cands.iter_mut() {
        c.sim = norm::similarity(&query, &c.title).max(norm::similarity(&query, &c.original_title));
        let adj = match (rel.year, c.year) {
            (Some(a), Some(b)) => match a.abs_diff(b) {
                0 => 0.10,
                1 => 0.05,
                _ => -0.25,
            },
            _ => 0.0,
        };
        // « Meurtres à Nordholm » S03 → la fiche « Meurtres à Nordholm 3 » (une fiche TMDB par saison)
        let mut numbered = 0.0;
        if let [s] = rel.seasons.as_slice() {
            let want = format!("{qkey} {s}");
            if norm::key(&c.title) == want || norm::key(&c.original_title) == want {
                c.sim = 1.0;
                numbered = 0.15;
            }
        }
        c.score = c.sim + adj + numbered + (c.popularity.min(100.0) / 2000.0);
    }
    cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
}

/// Le meilleur candidat est-il assez net pour ranger sans demander ?
pub fn is_confident(c: &[Candidate]) -> bool {
    match c {
        [] => false,
        [a] => a.sim >= 0.85,
        [a, b, ..] => {
            // net par l'écart de score, ou parce qu'il est de loin le plus populaire (fiche jumelle vide, etc.)
            let clear_gap = a.score - b.score >= 0.08;
            let far_more_popular = a.popularity >= 10.0 && a.popularity >= 5.0 * b.popularity && a.sim + 0.02 >= b.sim;
            a.sim >= 0.85 && (clear_gap || far_more_popular)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn cand(id: u64, title: &str, year: u16, pop: f32) -> Candidate {
        Candidate { id, media: Media::Tv, title: title.into(), original_title: title.into(), year: Some(year), genres: vec![], countries: vec![], poster: None, overview: String::new(), popularity: pop, sim: 0.0, score: 0.0 }
    }

    #[test]
    fn fiche_numerotee_par_saison() {
        let rel = parse("Meurtres.à.Nordholm.S03.FRENCH.1080p.WEB.H265-55H");
        let mut c = vec![cand(1, "Meurtres à Nordholm 1", 2015, 2.0), cand(3, "Meurtres à Nordholm 3", 2020, 2.0), cand(4, "Meurtres à Nordholm 4", 2023, 2.0)];
        score_candidates(&rel, &mut c);
        assert_eq!(c[0].id, 3);
        assert!(is_confident(&c));
    }

    #[test]
    fn deux_fiches_pareilles_sans_ecart_de_popularite() {
        let rel = parse("The.Body.2026.S01.MULTi.1080p.WEB-X");
        let mut c = vec![cand(1, "The Body", 2026, 3.0), cand(2, "The Body", 2026, 2.0)];
        score_candidates(&rel, &mut c);
        assert!(!is_confident(&c));
    }

    #[test]
    fn fiche_de_loin_la_plus_populaire() {
        let rel = parse("The.Body.2026.S01.MULTi.1080p.WEB-X");
        let mut c = vec![cand(1, "The Body", 2026, 0.3), cand(2, "The Body", 2026, 188.0)];
        score_candidates(&rel, &mut c);
        assert_eq!(c[0].id, 2);
        assert!(is_confident(&c));
    }

    #[test]
    fn remake_avec_autre_annee_ecarte() {
        let rel = parse("Shameless.US.2011.S01.1080p");
        let mut c = vec![cand(1, "Shameless", 2004, 50.0), cand(2, "Shameless", 2011, 40.0)];
        score_candidates(&rel, &mut c);
        assert_eq!(c[0].id, 2);
    }
}
