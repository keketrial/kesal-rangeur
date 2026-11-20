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
    /// Fichier contenant la clé API TMDB (v3) ou le jeton v4 (facultatif : la clé saisie dans les Réglages est
    /// rangée dans `secrets.json`, dans le dossier de données).
    #[serde(default)]
    pub tmdb_key_file: String,
    #[serde(default)]
    pub integrations: Integrations,
    /// Nommage des fichiers rangés (par défaut : noms d'origine conservés).
    #[serde(default)]
    pub naming: crate::naming::Naming,
    /// Mode essai à blanc : l'appli analyse et propose, mais n'écrit rien. ACTIVÉ PAR DÉFAUT.
    #[serde(default = "d_true")]
    pub essai: bool,
    /// Méthode de copie : auto (reflink → lien physique → copie), reflink, hardlink ou copy.
    #[serde(default)]
    pub copy_method: crate::exec::Method,
    /// Analyse automatique toutes les N minutes (0 = seulement à la demande).
    #[serde(default)]
    pub scan_interval_min: u64,
    /// Après chaque analyse, range seul ce qui est sûr (ignoré en mode essai).
    #[serde(default)]
    pub auto_range: bool,
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
/// Adresses des services à prévenir (les jetons sont dans `secrets.json`, jamais dans ce fichier).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Integrations {
    #[serde(default)]
    pub plex_url: String,
    #[serde(default)]
    pub jellyfin_url: String,
    #[serde(default)]
    pub ha_url: String,
    /// Service de notification Home Assistant, ex. « notify.mobile_app_mon_iphone » (vide = pas de notification).
    #[serde(default)]
    pub ha_service: String,
    /// Forcer la bibliothèque Plex d'une catégorie (numéro de section) si la détection par nom de dossier ne suffit pas.
    #[serde(default)]
    pub plex_sections: BTreeMap<Category, String>,
}

/// Secrets saisis dans l'interface : fichier à part, lisible par le seul propriétaire.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Secrets {
    #[serde(default)]
    pub tmdb: String,
    #[serde(default)]
    pub plex: String,
    #[serde(default)]
    pub jellyfin: String,
    #[serde(default)]
    pub ha: String,
}

impl Secrets {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("secrets.json")
    }
    pub fn load(data_dir: &Path) -> Secrets {
        std::fs::read_to_string(Self::path(data_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let p = Self::path(data_dir);
        std::fs::write(&p, serde_json::to_string_pretty(self).unwrap_or_default())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

fn d_true() -> bool {
    true
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
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let txt = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("lecture de {} : {e}", path.display()))?;
        Ok(toml::from_str(&txt)?)
    }
    pub fn tmdb_secret(&self) -> anyhow::Result<String> {
        let saved = Secrets::load(&self.data_dir).tmdb;
        if !saved.trim().is_empty() {
            return Ok(saved.trim().to_string());
        }
        if self.tmdb_key_file.is_empty() {
            return Err(anyhow::anyhow!("clé TMDB non renseignée (Réglages)"));
        }
        let p = expand_home(&self.tmdb_key_file);
        let s = std::fs::read_to_string(&p)
            .map_err(|e| anyhow::anyhow!("clé TMDB introuvable ({}) : {e}", p.display()))?;
        Ok(s.trim().to_string())
    }
}
