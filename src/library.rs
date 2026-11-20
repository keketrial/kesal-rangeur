//! Index de la bibliothèque existante : quels dossiers de séries ou de films sont déjà là.

use crate::{norm, parser};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Dossiers parasites à ignorer partout.
pub fn is_junk_name(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with('@')
        || name.starts_with('#')
        || name.eq_ignore_ascii_case("Plex Versions")
        || name.eq_ignore_ascii_case("Thumbs.db")
        || name.eq_ignore_ascii_case("lost+found")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibEntry {
    pub path: PathBuf,
    pub name: String,
    /// Titre lu dans le nom du dossier (vide si le dossier ne contient qu'une saison).
    pub title: String,
    pub year: Option<u16>,
    /// Saisons trouvées (nom du dossier, sous-dossiers, noms de fichiers).
    pub seasons: BTreeSet<u16>,
    /// Épisodes connus (saison, épisode) quand les noms de fichiers les donnent.
    pub episodes: BTreeSet<(u16, u16)>,
    /// Nombre de fichiers vidéo.
    pub videos: usize,
    /// Le dossier annonce une intégrale.
    pub complete: bool,
    /// Meilleure qualité vue dans ce dossier.
    pub best_quality: u32,
}

impl LibEntry {
    pub fn key(&self) -> String {
        norm::key(&self.title)
    }
}

/// Lit chaque dossier de premier niveau de `root` (séries). Les fichiers vidéo posés en vrac sont ignorés ici.
pub fn scan_series(root: &Path) -> std::io::Result<Vec<LibEntry>> {
    let mut out = Vec::new();
    let mut names: Vec<_> = std::fs::read_dir(root)?.filter_map(|e| e.ok()).collect();
    names.sort_by_key(|e| e.file_name());
    for e in names {
        let name = e.file_name().to_string_lossy().to_string();
        if is_junk_name(&name) || !e.path().is_dir() {
            continue;
        }
        out.push(scan_folder(&e.path(), &name));
    }
    Ok(out)
}

pub fn scan_folder(path: &Path, name: &str) -> LibEntry {
    let top = parser::parse(name);
    let mut ent = LibEntry {
        path: path.to_path_buf(),
        name: name.to_string(),
        title: top.title.clone(),
        year: top.year,
        seasons: top.seasons.iter().copied().collect(),
        episodes: BTreeSet::new(),
        videos: 0,
        complete: top.complete,
        best_quality: top.quality_score(),
    };
    for w in WalkDir::new(path).min_depth(1).max_depth(3).into_iter().filter_map(|e| e.ok()) {
        let n = w.file_name().to_string_lossy().to_string();
        if is_junk_name(&n) {
            continue;
        }
        let is_file = w.file_type().is_file();
        if is_file {
            let ext = w.path().extension().and_then(|e| e.to_str()).unwrap_or("");
            if !parser::is_video_ext(ext) {
                continue;
            }
            ent.videos += 1;
        }
        let p = parser::parse(&n);
        for s in &p.seasons {
            ent.seasons.insert(*s);
            for e in &p.episodes {
                ent.episodes.insert((*s, *e));
            }
        }
        if p.complete && !is_file {
            ent.complete = true;
        }
        ent.best_quality = ent.best_quality.max(if p.resolution.is_some() { p.quality_score() } else { 0 });
    }
    ent
}
