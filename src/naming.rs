//! Nommage des fichiers rangés, dans l'esprit de FileBot : variables {n} {y} {ny} {s} {s00} {e} {e00}
//! {s00e00} {sxe} {t} {vf} {source} {vc} {group}.
//!
//! Préréglages :
//! * `keep` (défaut) : noms d'origine conservés ;
//! * `filebot` : les exemples de la documentation FileBot (`{n} ({y})`, `{n} - {s00e00} - {t}`) ;
//! * `plex` : un dossier par film, `Season 01/` (ou le style déjà présent) pour les séries ;
//! * `custom` : vos propres formats.

use crate::parser::{self, Release};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    #[default]
    Keep,
    Filebot,
    Plex,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Naming {
    #[serde(default)]
    pub preset: Preset,
    /// Film, chemin relatif à la destination (peut contenir des « / » pour un dossier par film).
    #[serde(default = "d_movie")]
    pub movie: String,
    /// Épisode, relatif au dossier de la série.
    #[serde(default = "d_episode")]
    pub episode: String,
    /// Dossier de saison (« auto » = style déjà présent dans la série, sinon « Season 01 » ; vide = pas de dossier).
    #[serde(default = "d_season")]
    pub season_folder: String,
    /// Nom d'un NOUVEAU dossier de série (un dossier existant est toujours réutilisé tel quel).
    #[serde(default = "d_series")]
    pub series_folder: String,
}
fn d_movie() -> String {
    "{n} ({y})/{n} ({y})".into()
}
fn d_episode() -> String {
    "{n} - {s00e00} - {t}".into()
}
fn d_season() -> String {
    "auto".into()
}
fn d_series() -> String {
    "{n} ({y})".into()
}

impl Default for Naming {
    fn default() -> Self {
        Naming { preset: Preset::Keep, movie: d_movie(), episode: d_episode(), season_folder: d_season(), series_folder: d_series() }
    }
}

/// Formats réellement appliqués (None = noms d'origine conservés).
#[derive(Debug, Clone, PartialEq)]
pub struct Formats {
    pub movie: String,
    pub episode: String,
    pub season_folder: String,
    pub series_folder: String,
}

impl Naming {
    pub fn formats(&self) -> Option<Formats> {
        match self.preset {
            Preset::Keep => None,
            Preset::Filebot => Some(Formats { movie: "{n} ({y})".into(), episode: "{n} - {s00e00} - {t}".into(), season_folder: String::new(), series_folder: "{n} ({y})".into() }),
            Preset::Plex => Some(Formats { movie: "{n} ({y})/{n} ({y})".into(), episode: "{n} - {s00e00} - {t}".into(), season_folder: "auto".into(), series_folder: "{n} ({y})".into() }),
            Preset::Custom => Some(Formats { movie: self.movie.clone(), episode: self.episode.clone(), season_folder: self.season_folder.clone(), series_folder: self.series_folder.clone() }),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub n: String,
    pub y: Option<u16>,
    pub s: Option<u16>,
    pub e: Vec<u16>,
    pub t: Vec<String>,
    pub vf: Option<String>,
    pub source: Option<String>,
    pub vc: Option<String>,
    pub group: Option<String>,
    /// Codec audio (EAC3, DTS…).
    pub audio: Option<String>,
    /// « HDR » si le nom l'indique.
    pub hdr: Option<String>,
    /// Langues lues dans le nom (FR, EN…), séparées par un espace.
    pub lang: Option<String>,
}

const KNOWN: &[&str] = &["n", "y", "ny", "s", "s00", "e", "e00", "s00e00", "sxe", "t", "vf", "source", "vc", "group", "audio", "hdr", "lang"];

/// Rend une valeur utilisable dans un nom de fichier.
fn clean_value(v: &str) -> String {
    v.replace(':', " -").replace('/', "-").replace('\\', "-").replace(['*', '?', '"', '<', '>', '|'], "")
}

fn join_nums(v: &[u16], pad: bool) -> String {
    v.iter().map(|x| if pad { format!("{x:02}") } else { x.to_string() }).collect::<Vec<_>>().join("-")
}

fn value(tok: &str, c: &Ctx) -> String {
    let s = c.s.unwrap_or(0);
    match tok {
        "n" => clean_value(&c.n),
        "y" => c.y.map(|y| y.to_string()).unwrap_or_default(),
        "ny" => match c.y {
            Some(y) => format!("{} ({y})", clean_value(&c.n)),
            None => clean_value(&c.n),
        },
        "s" => c.s.map(|s| s.to_string()).unwrap_or_default(),
        "s00" => c.s.map(|s| format!("{s:02}")).unwrap_or_default(),
        "e" => join_nums(&c.e, false),
        "e00" => join_nums(&c.e, true),
        "s00e00" => match c.e.as_slice() {
            [] => format!("S{s:02}"),
            [one] => format!("S{s:02}E{one:02}"),
            many => format!("S{s:02}E{}", many.iter().map(|x| format!("{x:02}")).collect::<Vec<_>>().join("-E")),
        },
        "sxe" => match c.e.as_slice() {
            [] => format!("{s}x"),
            many => format!("{s}x{}", join_nums(many, true)),
        },
        "t" => clean_value(&c.t.iter().filter(|t| !t.trim().is_empty()).cloned().collect::<Vec<_>>().join(" & ")),
        "vf" => c.vf.clone().unwrap_or_default(),
        "source" => c.source.clone().unwrap_or_default(),
        "vc" => c.vc.clone().unwrap_or_default(),
        "group" => c.group.clone().unwrap_or_default(),
        "audio" => c.audio.clone().unwrap_or_default(),
        "hdr" => c.hdr.clone().unwrap_or_default(),
        "lang" => c.lang.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

/// Retire les restes d'une variable vide : « Titre - S01E01 - » → « Titre - S01E01 », « () » vides, espaces doubles.
fn tidy(seg: &str) -> String {
    let parts: Vec<&str> = seg.split(" - ").map(|p| p.trim()).filter(|p| !p.is_empty() && *p != "-").collect();
    let mut s = parts.join(" - ");
    s = s.replace("()", "").replace("[]", "");
    let ws = Regex::new(r"\s{2,}").unwrap();
    ws.replace_all(&s, " ").trim().trim_end_matches(['.', ' ', '-']).trim().to_string()
}

/// Vérifie qu'un format n'utilise que des variables connues.
pub fn validate(tpl: &str) -> Result<(), String> {
    let re = Regex::new(r"\{([^{}]*)\}").unwrap();
    for c in re.captures_iter(tpl) {
        if !KNOWN.contains(&&c[1]) {
            return Err(format!("variable inconnue : {{{}}} (variables permises : {})", &c[1], KNOWN.iter().map(|k| format!("{{{k}}}")).collect::<Vec<_>>().join(" ")));
        }
    }
    let opens = tpl.matches('{').count();
    let closes = tpl.matches('}').count();
    if opens != closes {
        return Err("accolades { } non appariées".into());
    }
    Ok(())
}

/// Rend un format ; chaque « / » sépare un dossier. Renvoie les segments nettoyés (jamais vides).
pub fn render(tpl: &str, c: &Ctx) -> Result<Vec<String>, String> {
    validate(tpl)?;
    let re = Regex::new(r"\{([^{}]*)\}").unwrap();
    let mut out = Vec::new();
    for seg in tpl.split('/') {
        let filled = re.replace_all(seg, |cap: &regex::Captures| value(&cap[1], c)).to_string();
        let t = tidy(&filled);
        if !t.is_empty() {
            out.push(t);
        }
    }
    if out.is_empty() {
        return Err("le format donne un nom vide".into());
    }
    Ok(out)
}

const SUB_EXT: &[&str] = &["srt", "ass", "ssa", "sub", "idx", "sup", "vtt"];

pub fn is_sub(name: &str) -> bool {
    name.rsplit_once('.').map_or(false, |(_, e)| SUB_EXT.contains(&e.to_lowercase().as_str()))
}
pub fn is_video(name: &str) -> bool {
    name.rsplit_once('.').map_or(false, |(_, e)| parser::is_video_ext(e))
}

/// « .fr », « .en.forced » avant l'extension d'un sous-titre.
fn sub_suffix(name: &str) -> String {
    let re = Regex::new(r"(?i)(\.[a-z]{2,3}(?:\.(?:forced|sdh|cc))?)\.([a-z]{3})$").unwrap();
    re.captures(name).map(|c| c[1].to_string()).unwrap_or_default()
}
fn ext_of(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| format!(".{e}")).unwrap_or_default()
}

/// Style de dossier de saison déjà présent dans une série (« S01 », « Saison_01 (2004) », « Season 1 »…) → format.
pub fn detect_season_style(series_dir: &Path) -> Option<String> {
    let re = Regex::new(r"^(\D*?)(\d{1,3})(.*)$").unwrap();
    let mut styles: Vec<String> = Vec::new();
    for e in std::fs::read_dir(series_dir).ok()?.filter_map(|e| e.ok()) {
        if !e.path().is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        let r = parser::parse(&name);
        if !r.title.is_empty() || r.seasons.len() != 1 {
            continue;
        }
        if let Some(c) = re.captures(&name) {
            let tok = if c[2].len() >= 2 { "{s00}" } else { "{s}" };
            styles.push(format!("{}{tok}", &c[1]));
        }
    }
    styles.sort();
    styles.first().cloned()
}

fn season_of_path(rel: &Path, hint: Option<u16>) -> Option<u16> {
    let comps: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    for c in comps.iter().rev().skip(1) {
        let r = parser::parse(c);
        if let Some(s) = r.seasons.first() {
            if r.seasons.len() == 1 {
                return Some(*s);
            }
        }
    }
    hint
}

/// Saison et épisode(s) d'un fichier, d'après son nom puis ses dossiers parents.
pub fn episode_of(rel: &Path, hint_season: Option<u16>) -> Option<(u16, Vec<u16>)> {
    let name = rel.file_name()?.to_string_lossy().to_string();
    let r = parser::parse(&name);
    if !r.episodes.is_empty() {
        let s = r.seasons.first().copied().or_else(|| season_of_path(rel, hint_season))?;
        return Some((s, r.episodes));
    }
    // « E05 », « Episode 5 », « 05 - Titre » dans un dossier de saison
    let s = season_of_path(rel, hint_season)?;
    let re = Regex::new(r"(?i)(?:^|[ ._\-\[(])(?:e|ep|episode)[ ._]?(\d{1,3})(?:[ ._\-\])]|$)").unwrap();
    if let Some(c) = re.captures(&name) {
        return Some((s, vec![c[1].parse().ok()?]));
    }
    let re2 = Regex::new(r"^(\d{1,3})[ ._\-]").unwrap();
    re2.captures(&name).and_then(|c| c[1].parse().ok()).map(|e| (s, vec![e]))
}

pub struct SeriesIn<'a> {
    pub fmt: &'a Formats,
    pub name: &'a str,
    pub year: Option<u16>,
    pub release: &'a Release,
    /// Saison du nom d'origine → saison de la fiche TMDB (Nordholm : 2 → 1).
    pub renum: Option<(u16, u16)>,
    /// Style de saison déjà présent dans la série (si elle existe).
    pub existing_style: Option<String>,
    /// Titres d'épisodes TMDB : saison → épisode → titre.
    pub titles: &'a HashMap<u16, HashMap<u16, String>>,
}

/// Associe chaque fichier d'une release de série à son nouveau chemin (relatif au dossier de la série).
/// Les fichiers qui ne sont ni vidéo ni sous-titre sont ignorés. Erreur si un épisode n'est pas identifiable.
pub fn map_series(files: &[PathBuf], i: &SeriesIn) -> Result<(Vec<(PathBuf, PathBuf)>, Vec<PathBuf>), String> {
    let hint = if let [s] = i.release.seasons.as_slice() { Some(*s) } else { None };
    let mut out = Vec::new();
    let mut ignored = Vec::new();
    let mut unresolved = Vec::new();
    for rel in files {
        let name = rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| i.name.to_string());
        let vid = is_video(&name);
        let sub = is_sub(&name);
        if !vid && !sub {
            ignored.push(rel.clone());
            continue;
        }
        // un fichier posé seul à la racine garde le nom de l'élément comme nom d'origine
        let Some((s0, eps)) = episode_of(rel, hint).or_else(|| if rel.as_os_str().is_empty() { episode_of(Path::new(i.name), hint) } else { None }) else {
            unresolved.push(name);
            continue;
        };
        let s = match i.renum {
            Some((from, to)) if from == s0 => to,
            _ => s0,
        };
        let titles: Vec<String> = eps.iter().map(|e| i.titles.get(&s).and_then(|m| m.get(e)).cloned().unwrap_or_default()).collect();
        let pr = parser::parse(&name);
        let ctx = Ctx {
            n: i.name.to_string(),
            y: i.year,
            s: Some(s),
            e: eps,
            t: titles,
            vf: pr.resolution.clone().or_else(|| i.release.resolution.clone()),
            source: pr.source.clone().or_else(|| i.release.source.clone()),
            vc: pr.codec.clone().or_else(|| i.release.codec.clone()),
            group: pr.group.clone().or_else(|| i.release.group.clone()),
            audio: pr.audio.clone().or_else(|| i.release.audio.clone()),
            hdr: if pr.hdr || i.release.hdr { Some("HDR".into()) } else { None },
            lang: { let l = if pr.langs.is_empty() { &i.release.langs } else { &pr.langs }; if l.is_empty() { None } else { Some(l.join(" ")) } },
        };
        let mut segs: Vec<String> = Vec::new();
        let sf = i.fmt.season_folder.trim();
        let sf = if sf == "auto" { i.existing_style.clone().unwrap_or_else(|| "Season {s00}".into()) } else { sf.to_string() };
        if !sf.is_empty() {
            segs.extend(render(&sf, &ctx)?);
        }
        let mut file = render(&i.fmt.episode, &ctx)?;
        let last = file.pop().unwrap();
        segs.extend(file);
        let suffix = if sub { format!("{}{}", sub_suffix(&name), ext_of(&name)) } else { ext_of(&name) };
        // sous-titre « Show.S01E01.fr.srt » : le suffixe de langue fait partie de l'extension
        let suffix = if sub { suffix } else { ext_of(&name) };
        segs.push(format!("{last}{suffix}"));
        out.push((rel.clone(), segs.iter().collect::<PathBuf>()));
    }
    if !unresolved.is_empty() {
        return Err(format!("épisode(s) non identifiable(s) dans le nom : {}", unresolved.iter().take(3).cloned().collect::<Vec<_>>().join(", ")));
    }
    if out.is_empty() {
        return Err("aucun fichier vidéo reconnu".into());
    }
    // deux fichiers ne doivent pas arriver au même endroit
    let mut seen = std::collections::HashSet::new();
    for (_, d) in &out {
        if !seen.insert(d.clone()) {
            return Err(format!("deux fichiers auraient le même nom : {}", d.display()));
        }
    }
    check_len(&out)?;
    Ok((out, ignored))
}

/// « Part.1 », « CD2 », « Disc 1 », « pt3 » dans un nom de fichier → numéro de la partie.
fn part_of(name: &str) -> Option<u32> {
    let re = Regex::new(r"(?i)(?:^|[ ._\-\[(])(?:part|pt|cd|disc|disk)[ ._\-]?(\d{1,2})(?:[ ._\-\])]|$)").unwrap();
    re.captures(name).and_then(|c| c[1].parse().ok())
}

/// Aucun élément de chemin ne doit dépasser 255 octets (limite de la plupart des systèmes de fichiers).
fn check_len(map: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    for (_, d) in map {
        for c in d.components() {
            let n = c.as_os_str().len();
            if n > 255 {
                return Err(format!("nom trop long ({n} octets, maximum 255) : {}", c.as_os_str().to_string_lossy().chars().take(60).collect::<String>()));
            }
        }
    }
    Ok(())
}

/// Film : le plus gros fichier vidéo donne le nom ; les sous-titres le suivent. Un film en plusieurs parties
/// (Part.1, Part.2 / CD1, CD2) donne « Titre (année) - part1 », « - part2 » (convention de Plex).
pub fn map_movie(files: &[(PathBuf, u64)], fmt: &Formats, title: &str, year: Option<u16>, release: &Release) -> Result<(Vec<(PathBuf, PathBuf)>, Vec<PathBuf>), String> {
    let videos: Vec<&(PathBuf, u64)> = files.iter().filter(|(p, _)| p.file_name().map_or(false, |n| is_video(&n.to_string_lossy()))).collect();
    let biggest = videos.iter().map(|v| v.1).max().ok_or("aucun fichier vidéo dans le dossier du film")?;
    let mains: Vec<&&(PathBuf, u64)> = videos.iter().filter(|v| v.1 * 10 >= biggest * 3).collect();
    let name_of = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    // numéro de partie de chaque vidéo principale (seulement s'il y en a plusieurs)
    let mut parts: HashMap<PathBuf, u32> = HashMap::new();
    if mains.len() > 1 {
        let nums: Vec<Option<u32>> = mains.iter().map(|m| part_of(&name_of(&m.0))).collect();
        let distinct: std::collections::HashSet<u32> = nums.iter().flatten().copied().collect();
        if nums.iter().any(|n| n.is_none()) || distinct.len() != mains.len() {
            return Err(format!("plusieurs vidéos de taille comparable dans le dossier du film ({}), sans numéro de partie reconnaissable", mains.len()));
        }
        for (m, n) in mains.iter().zip(nums) {
            parts.insert(m.0.clone(), n.unwrap());
        }
    }
    let ctx = Ctx { n: title.to_string(), y: year, vf: release.resolution.clone(), source: release.source.clone(), vc: release.codec.clone(), group: release.group.clone(), audio: release.audio.clone(), hdr: if release.hdr { Some("HDR".into()) } else { None }, lang: if release.langs.is_empty() { None } else { Some(release.langs.join(" ")) }, ..Default::default() };
    let mut segs = render(&fmt.movie, &ctx)?;
    let base = segs.pop().unwrap();
    let mut out = Vec::new();
    let mut ignored = Vec::new();
    for (rel, _) in files {
        let name = name_of(rel);
        let dst_name = if mains.iter().any(|m| &m.0 == rel) {
            match parts.get(rel) {
                Some(n) => format!("{base} - part{n}{}", ext_of(&name)),
                None => format!("{base}{}", ext_of(&name)),
            }
        } else if is_sub(&name) {
            match (parts.is_empty(), part_of(&name)) {
                (true, _) => format!("{base}{}{}", sub_suffix(&name), ext_of(&name)),
                (false, Some(n)) if parts.values().any(|p| *p == n) => format!("{base} - part{n}{}{}", sub_suffix(&name), ext_of(&name)),
                _ => {
                    ignored.push(rel.clone());
                    continue;
                }
            }
        } else {
            ignored.push(rel.clone());
            continue;
        };
        let mut p: PathBuf = segs.iter().collect();
        p.push(dst_name);
        out.push((rel.clone(), p));
    }
    let mut seen = std::collections::HashSet::new();
    for (_, d) in &out {
        if !seen.insert(d.clone()) {
            return Err(format!("deux fichiers auraient le même nom : {}", d.display()));
        }
    }
    check_len(&out)?;
    Ok((out, ignored))
}

/// Exemple affiché dans les Réglages.
pub fn preview(fmt: &Formats) -> Result<(String, String), String> {
    let movie_ctx = Ctx { n: "Avatar".into(), y: Some(2009), vf: Some("1080p".into()), source: Some("BluRay".into()), vc: Some("x265".into()), group: Some("GRP".into()), audio: Some("EAC3".into()), hdr: Some("HDR".into()), lang: Some("FR".into()), ..Default::default() };
    let m = render(&fmt.movie, &movie_ctx)?.join("/") + ".mkv";
    let ep_ctx = Ctx { n: "Dark Angel".into(), y: Some(2000), s: Some(3), e: vec![1], t: vec!["Labyrinthe".into()], vf: Some("1080p".into()), source: Some("WEB".into()), vc: Some("x264".into()), group: Some("GRP".into()), audio: Some("AC3".into()), hdr: None, lang: Some("FR".into()) };
    let mut segs = Vec::new();
    let sf = fmt.season_folder.trim();
    let sf = if sf == "auto" { "Season {s00}" } else { sf };
    if !sf.is_empty() {
        segs.extend(render(sf, &ep_ctx)?);
    }
    segs.extend(render(&fmt.episode, &ep_ctx)?);
    let series_folder = render(&fmt.series_folder, &ep_ctx)?.join("/");
    Ok((m, format!("{series_folder}/{}.mkv", segs.join("/"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn variables_techniques_au_choix() {
        let c = Ctx { n: "Avatar".into(), y: Some(2009), vf: Some("1080p".into()), source: Some("BluRay".into()), vc: Some("H.265".into()), audio: Some("EAC3".into()), hdr: Some("HDR".into()), lang: Some("FR".into()), ..Default::default() };
        assert_eq!(render("{n} ({y}) - {vf} {source} {vc} {audio} {hdr} {lang} {group}", &c).unwrap(), vec!["Avatar (2009) - 1080p BluRay H.265 EAC3 HDR FR".to_string()]);
        // rien de coché côté technique : pas de reste
        let m = Ctx { n: "Avatar".into(), y: Some(2009), ..Default::default() };
        assert_eq!(render("{n} ({y}) - {vf} {audio} {hdr}", &m).unwrap(), vec!["Avatar (2009)".to_string()]);
    }

    fn ctx_ep() -> Ctx {
        Ctx { n: "Dark Angel".into(), y: Some(2000), s: Some(3), e: vec![1], t: vec!["Labyrinth".into()], ..Default::default() }
    }

    #[test]
    fn exemples_de_la_documentation_filebot() {
        // {n} - {s00e00} - {t} → « Dark Angel - S03E01 - Labyrinth » ; {n} ({y}) ; {ny} → « Avatar (2009) »
        assert_eq!(render("{n} - {s00e00} - {t}", &ctx_ep()).unwrap(), vec!["Dark Angel - S03E01 - Labyrinth"]);
        let m = Ctx { n: "The Man from Earth".into(), y: Some(2007), ..Default::default() };
        assert_eq!(render("{n} ({y})", &m).unwrap(), vec!["The Man from Earth (2007)"]);
        let a = Ctx { n: "Avatar".into(), y: Some(2009), ..Default::default() };
        assert_eq!(render("{ny}", &a).unwrap(), vec!["Avatar (2009)"]);
        assert_eq!(render("{sxe} - {t}", &ctx_ep()).unwrap(), vec!["3x01 - Labyrinth"]);
        assert_eq!(render("Season {s}/{n}", &ctx_ep()).unwrap(), vec!["Season 3", "Dark Angel"]);
    }

    #[test]
    fn dossier_par_film_et_caracteres_interdits() {
        let m = Ctx { n: "Mission: Impossible / Fallout".into(), y: Some(2018), ..Default::default() };
        assert_eq!(render("{n} ({y})/{n} ({y})", &m).unwrap(), vec!["Mission - Impossible - Fallout (2018)", "Mission - Impossible - Fallout (2018)"]);
        let q = Ctx { n: "Qui est-ce ?".into(), y: None, ..Default::default() };
        assert_eq!(render("{n} ({y})", &q).unwrap(), vec!["Qui est-ce"], "année absente : pas de parenthèses vides");
    }

    #[test]
    fn titre_d_episode_absent_ne_laisse_pas_de_tiret() {
        let mut c = ctx_ep();
        c.t = vec![String::new()];
        assert_eq!(render("{n} - {s00e00} - {t}", &c).unwrap(), vec!["Dark Angel - S03E01"]);
    }

    #[test]
    fn plusieurs_episodes_dans_un_fichier() {
        let mut c = ctx_ep();
        c.e = vec![1, 2];
        c.t = vec!["Un".into(), "Deux".into()];
        assert_eq!(render("{n} - {s00e00} - {t}", &c).unwrap(), vec!["Dark Angel - S03E01-E02 - Un & Deux"]);
        assert_eq!(render("{e00}", &c).unwrap(), vec!["01-02"]);
    }

    #[test]
    fn variable_inconnue_refusee() {
        assert!(validate("{n} {zzz}").is_err());
        assert!(validate("{n").is_err());
        assert!(validate("{n} ({y}) {vf}").is_ok());
    }

    #[test]
    fn style_de_saison_detecte() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("Saison_01 (2004)")).unwrap();
        assert_eq!(detect_season_style(t.path()).as_deref(), Some("Saison_{s00}"));
        let t2 = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t2.path().join("S01")).unwrap();
        assert_eq!(detect_season_style(t2.path()).as_deref(), Some("S{s00}"));
        let t3 = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t3.path().join("Season 1")).unwrap();
        assert_eq!(detect_season_style(t3.path()).as_deref(), Some("Season {s}"));
        let t4 = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t4.path().join("Evil.2019.S01.MULTi.1080p-FW")).unwrap();
        assert_eq!(detect_season_style(t4.path()), None, "un dossier de release n'est pas un dossier de saison");
    }

    #[test]
    fn episode_depuis_nom_ou_dossier() {
        assert_eq!(episode_of(Path::new("Show.S02E05.mkv"), None), Some((2, vec![5])));
        assert_eq!(episode_of(Path::new("Saison 3/E07.mkv"), None), Some((3, vec![7])));
        assert_eq!(episode_of(Path::new("S01/04 - Titre.mkv"), None), Some((1, vec![4])));
        assert_eq!(episode_of(Path::new("Show 1x05.mkv"), None), Some((1, vec![5])));
        assert_eq!(episode_of(Path::new("Random.mkv"), Some(1)), None);
    }

    fn fmt_plex() -> Formats {
        Naming { preset: Preset::Plex, ..Default::default() }.formats().unwrap()
    }

    #[test]
    fn nordholm_renumerote_et_renomme() {
        let rel = parser::parse("Meurtres.à.Nordholm.S02.FRENCH.1080p.WEB.DDP2.0.H265-55H");
        let mut titles: HashMap<u16, HashMap<u16, String>> = HashMap::new();
        titles.insert(1, HashMap::from([(1, "La famille disparue (1)".to_string()), (2, "La famille disparue (2)".to_string())]));
        let files = vec![
            PathBuf::from("Meurtres.à.Nordholm.S02E01.Une.famille.disparue.(Partie.1).FRENCH.1080p.WEB.DDP2.0.H265-55H.mkv"),
            PathBuf::from("Meurtres.à.Nordholm.S02E02.FiNAL.Une.famille.disparue.(Partie.2).FRENCH.1080p.WEB.DDP2.0.H265-55H.mkv"),
            PathBuf::from("Meurtres.à.Nordholm.S02.nfo"),
        ];
        let fmt = fmt_plex();
        let (map, ign) = map_series(&files, &SeriesIn { fmt: &fmt, name: "Meurtres à Nordholm 2", year: Some(2019), release: &rel, renum: Some((2, 1)), existing_style: None, titles: &titles }).unwrap();
        assert_eq!(map[0].1, PathBuf::from("Season 01/Meurtres à Nordholm 2 - S01E01 - La famille disparue (1).mkv"));
        assert_eq!(map[1].1, PathBuf::from("Season 01/Meurtres à Nordholm 2 - S01E02 - La famille disparue (2).mkv"));
        assert_eq!(ign.len(), 1, "le .nfo est ignoré");
    }

    #[test]
    fn style_existant_respecte() {
        let rel = parser::parse("Dawson.S02.MULTi.1080p-X");
        let titles = HashMap::new();
        let fmt = fmt_plex();
        let (map, _) = map_series(&[PathBuf::from("Dawson.S02E03.mkv")], &SeriesIn { fmt: &fmt, name: "Dawson", year: Some(1998), release: &rel, renum: None, existing_style: Some("S{s00}".into()), titles: &titles }).unwrap();
        assert_eq!(map[0].1, PathBuf::from("S02/Dawson - S02E03.mkv"));
    }

    #[test]
    fn episode_non_identifiable_refuse() {
        let rel = parser::parse("Show.MULTi.1080p-X");
        let titles = HashMap::new();
        let fmt = fmt_plex();
        let r = map_series(&[PathBuf::from("bonus.mkv")], &SeriesIn { fmt: &fmt, name: "Show", year: None, release: &rel, renum: None, existing_style: None, titles: &titles });
        assert!(r.is_err());
    }

    #[test]
    fn sous_titres_gardent_leur_langue() {
        let rel = parser::parse("Show.S01.1080p-X");
        let titles = HashMap::new();
        let fmt = fmt_plex();
        let (map, _) = map_series(&[PathBuf::from("Show.S01E01.mkv"), PathBuf::from("Show.S01E01.fr.srt")], &SeriesIn { fmt: &fmt, name: "Show", year: None, release: &rel, renum: None, existing_style: None, titles: &titles }).unwrap();
        assert_eq!(map[1].1, PathBuf::from("Season 01/Show - S01E01.fr.srt"));
    }

    #[test]
    fn film_dossier_et_sous_titres() {
        let rel = parser::parse("Avatar.2009.MULTi.1080p.BluRay.x265-GRP");
        let fmt = fmt_plex();
        let files = vec![(PathBuf::from("Avatar.2009.mkv"), 5_000_000_000), (PathBuf::from("Avatar.2009.fr.srt"), 50_000), (PathBuf::from("sample.mkv"), 20_000_000), (PathBuf::from("info.nfo"), 10)];
        let (map, ign) = map_movie(&files, &fmt, "Avatar", Some(2009), &rel).unwrap();
        assert_eq!(map[0].1, PathBuf::from("Avatar (2009)/Avatar (2009).mkv"));
        assert_eq!(map[1].1, PathBuf::from("Avatar (2009)/Avatar (2009).fr.srt"));
        assert_eq!(ign.len(), 2, "sample et nfo ignorés");
    }

    #[test]
    fn film_en_plusieurs_parties() {
        let rel = parser::parse("Oradour.2026.VFF.1080p.WEB.H264-PiCKLES");
        let fmt = fmt_plex();
        let files = vec![
            (PathBuf::from("Oradour.2026.Part.1.VFF.1080p.WEB.H264-PiCKLES.mp4"), 1_866_018_808),
            (PathBuf::from("Oradour.2026.Part.2.VFF.1080p.WEB.H264-PiCKLES.mp4"), 2_029_086_848),
        ];
        let (map, _) = map_movie(&files, &fmt, "Oradour", Some(2026), &rel).unwrap();
        assert_eq!(map[0].1, PathBuf::from("Oradour (2026)/Oradour (2026) - part1.mp4"));
        assert_eq!(map[1].1, PathBuf::from("Oradour (2026)/Oradour (2026) - part2.mp4"));
        let cd = vec![(PathBuf::from("Film.CD1.avi"), 700_000_000), (PathBuf::from("Film.CD2.avi"), 690_000_000)];
        let (m2, _) = map_movie(&cd, &fmt, "Film", Some(2000), &rel).unwrap();
        assert_eq!(m2[1].1, PathBuf::from("Film (2000)/Film (2000) - part2.avi"));
    }

    #[test]
    fn plusieurs_videos_sans_numero_refuse() {
        let rel = parser::parse("Film.2000.1080p-X");
        let fmt = fmt_plex();
        let files = vec![(PathBuf::from("film.mkv"), 700_000_000), (PathBuf::from("film bis.mkv"), 690_000_000)];
        assert!(map_movie(&files, &fmt, "Film", Some(2000), &rel).is_err());
        // deux fichiers qui prétendent être la même partie
        let dup = vec![(PathBuf::from("Film.CD1.mkv"), 700_000_000), (PathBuf::from("Film.part1.mkv"), 690_000_000)];
        assert!(map_movie(&dup, &fmt, "Film", Some(2000), &rel).is_err());
    }

    #[test]
    fn nom_trop_long_refuse() {
        let rel = parser::parse("Film.2000.1080p-X");
        let fmt = fmt_plex();
        let long = "Un titre extrêmement long ".repeat(12);
        let files = vec![(PathBuf::from("film.mkv"), 1_000_000)];
        let e = map_movie(&files, &fmt, &long, Some(2000), &rel).unwrap_err();
        assert!(e.contains("trop long"), "{e}");
    }

    #[test]
    fn apercu_des_reglages() {
        let (m, e) = preview(&Naming { preset: Preset::Filebot, ..Default::default() }.formats().unwrap()).unwrap();
        assert_eq!(m, "Avatar (2009).mkv");
        assert_eq!(e, "Dark Angel (2000)/Dark Angel - S03E01 - Labyrinthe.mkv");
        let (m, e) = preview(&fmt_plex()).unwrap();
        assert_eq!(m, "Avatar (2009)/Avatar (2009).mkv");
        assert_eq!(e, "Dark Angel (2000)/Season 03/Dark Angel - S03E01 - Labyrinthe.mkv");
    }
}
