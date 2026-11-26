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

/// Vrai si le dossier contient au moins une vidéo non vide (profondeur 3). Un dossier vide n'est pas une copie de quoi que ce soit.
pub fn has_video(path: &Path) -> bool {
    WalkDir::new(path).min_depth(1).max_depth(3).into_iter().filter_map(|e| e.ok()).any(|w| {
        w.file_type().is_file()
            && w.path().extension().and_then(|e| e.to_str()).map_or(false, parser::is_video_ext)
            && w.metadata().map_or(false, |m| m.len() > 0)
            && !is_junk_name(&w.file_name().to_string_lossy())
    })
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
    // Une saison n'existe que si une vidéo s'y trouve : un dossier « S01 » vide (créé par Resilio, ou laissé par un
    // ancien rangement raté) ne doit pas faire croire que la saison est déjà là. On retient donc les sous-dossiers
    // à part et on ne les compte qu'à la fin, s'ils contiennent une vidéo.
    let mut video_paths: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<(PathBuf, parser::Release)> = Vec::new();
    for w in WalkDir::new(path).min_depth(1).max_depth(3).into_iter().filter_map(|e| e.ok()) {
        let n = w.file_name().to_string_lossy().to_string();
        if is_junk_name(&n) {
            continue;
        }
        let is_file = w.file_type().is_file();
        if is_file {
            let ext = w.path().extension().and_then(|e| e.to_str()).unwrap_or("");
            if !parser::is_video_ext(ext) || w.metadata().map_or(true, |m| m.len() == 0) {
                continue;
            }
            ent.videos += 1;
            video_paths.push(w.path().to_path_buf());
        }
        let p = parser::parse(&n);
        if !is_file {
            dirs.push((w.path().to_path_buf(), p));
            continue;
        }
        for s in &p.seasons {
            ent.seasons.insert(*s);
            for e in &p.episodes {
                ent.episodes.insert((*s, *e));
            }
        }
        ent.best_quality = ent.best_quality.max(if p.resolution.is_some() { p.quality_score() } else { 0 });
    }
    for (dir, p) in dirs {
        if !video_paths.iter().any(|v| v.starts_with(&dir)) {
            continue;
        }
        for s in &p.seasons {
            ent.seasons.insert(*s);
        }
        if p.complete {
            ent.complete = true;
        }
        ent.best_quality = ent.best_quality.max(if p.resolution.is_some() { p.quality_score() } else { 0 });
    }
    if ent.videos == 0 {
        // coquille vide : aucune saison, aucune intégrale
        ent.seasons.clear();
        ent.complete = false;
    }
    ent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dossier_de_saison_vide_ne_compte_pas() {
        let t = tempfile::tempdir().unwrap();
        // cas réel : un ancien rangement raté a laissé « The Altruists (2026)/…S01… » sans aucun fichier
        let show = t.path().join("The Altruists (2026)");
        std::fs::create_dir_all(show.join("The.Altruists.S01.MULTi.AD.1080p.NF.WEB.AV1-BYOR")).unwrap();
        let e = scan_folder(&show, "The Altruists (2026)");
        assert!(e.seasons.is_empty(), "saison fantôme : {:?}", e.seasons);
        assert_eq!(e.videos, 0);
        assert!(!has_video(&show));
        // une vidéo de 0 octet n'y change rien
        std::fs::write(show.join("The.Altruists.S01.MULTi.AD.1080p.NF.WEB.AV1-BYOR/The.Altruists.S01E01.mkv"), "").unwrap();
        assert!(scan_folder(&show, "The Altruists (2026)").seasons.is_empty());
        // dès que la vidéo est vraiment là, la saison existe
        std::fs::write(show.join("The.Altruists.S01.MULTi.AD.1080p.NF.WEB.AV1-BYOR/The.Altruists.S01E01.mkv"), "x").unwrap();
        let e = scan_folder(&show, "The Altruists (2026)");
        assert!(e.seasons.contains(&1) && e.videos == 1);
        assert!(has_video(&show));
    }

    #[test]
    fn saison_vide_a_cote_d_une_saison_pleine() {
        let t = tempfile::tempdir().unwrap();
        let show = t.path().join("Show (2020)");
        std::fs::create_dir_all(show.join("Show.S02.1080p")).unwrap();
        std::fs::create_dir_all(show.join("Show.S01.1080p")).unwrap();
        std::fs::write(show.join("Show.S01.1080p/Show.S01E01.mkv"), "x").unwrap();
        let e = scan_folder(&show, "Show (2020)");
        assert_eq!(e.seasons.iter().copied().collect::<Vec<_>>(), vec![1], "seule la saison 1 existe vraiment");
    }

    #[test]
    fn film_dossier_vide_pas_dans_l_index() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("Film (2026)")).unwrap();
        std::fs::create_dir_all(t.path().join("Autre (2025)")).unwrap();
        std::fs::write(t.path().join("Autre (2025)/Autre.2025.mkv"), "x").unwrap();
        let idx = crate::plan::MovieIndex::scan(&[t.path().to_path_buf()]);
        assert_eq!(idx.entries.len(), 1);
        assert!(idx.find(&["Autre"], Some(2025)).is_some() && idx.find(&["Film"], Some(2026)).is_none());
    }
}
