//! Réglages (fichier TOML).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Film,
    Serie,
    DessinAnime,
    Telerealite,
    Documentaire,
    Spectacle,
}

impl Category {
    pub fn label(&self) -> &'static str {
        match self {
            Category::Film => "film",
            Category::Serie => "série",
            Category::DessinAnime => "dessin animé",
            Category::Telerealite => "télé / téléréalité",
            Category::Documentaire => "documentaire",
            Category::Spectacle => "spectacle",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
    /// « auto » (l'appli devine) ou une catégorie imposée (« film », « serie »…).
    #[serde(default = "d_auto")]
    pub mode: String,
    /// « copy » (reflink / lien physique, l'original reste) ou « move ».
    #[serde(default = "d_copy")]
    pub action: String,
    /// Minutes sans changement avant de ranger.
    #[serde(default = "d_settle")]
    pub settle_minutes: u64,
}
fn d_auto() -> String {
    "auto".into()
}
fn d_copy() -> String {
    "copy".into()
}
fn d_settle() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Titre (comparé sans accents ni casse) auquel la règle s'applique.
    pub title: String,
    pub category: Category,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Langue des titres TMDB : fr-FR, es-ES, en-US…
    #[serde(default = "d_lang")]
    pub language: String,
    /// Fichier contenant la clé API TMDB (v3) ou le jeton v4.
    pub tmdb_key_file: String,
    #[serde(default = "d_data")]
    pub data_dir: PathBuf,
    #[serde(default)]
    pub inputs: Vec<Input>,
    pub destinations: BTreeMap<Category, PathBuf>,
    /// Dossiers de films supplémentaires à lire pour repérer les doublons.
    #[serde(default)]
    pub film_libraries: Vec<PathBuf>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}
fn d_lang() -> String {
    "fr-FR".into()
}
fn d_data() -> PathBuf {
    "data".into()
}

pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(h) = std::env::var("HOME") {
            return Path::new(&h).join(rest);
        }
    }
    PathBuf::from(p)
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let txt = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("lecture de {} : {e}", path.display()))?;
        Ok(toml::from_str(&txt)?)
    }
    pub fn tmdb_secret(&self) -> anyhow::Result<String> {
        let p = expand_home(&self.tmdb_key_file);
        let s = std::fs::read_to_string(&p)
            .map_err(|e| anyhow::anyhow!("clé TMDB introuvable ({}) : {e}", p.display()))?;
        Ok(s.trim().to_string())
    }
}
