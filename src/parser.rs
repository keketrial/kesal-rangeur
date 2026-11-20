//! Analyse du nom d'une release (dossier ou fichier) : titre, année, saison, qualité, langue, équipe.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Release {
    /// Nom d'origine, sans l'extension vidéo.
    pub raw: String,
    /// Titre nettoyé (points remplacés par des espaces). Vide pour un dossier du genre « S01 ».
    pub title: String,
    pub year: Option<u16>,
    /// Saisons concernées (une seule pour S01, plusieurs pour S01-S17).
    pub seasons: Vec<u16>,
    pub episodes: Vec<u16>,
    /// COMPLETE / INTEGRALE / plusieurs saisons.
    pub complete: bool,
    pub resolution: Option<String>,
    pub source: Option<String>,
    pub codec: Option<String>,
    pub hdr: bool,
    pub langs: Vec<String>,
    pub group: Option<String>,
    /// Le nom contient « DOC » / « DOCU » : indice pour classer en documentaire.
    pub doc_hint: bool,
}

impl Release {
    /// Le nom évoque une série (saison, épisode, pack).
    pub fn is_series_like(&self) -> bool {
        !self.seasons.is_empty() || !self.episodes.is_empty() || self.complete
    }
    /// Le nom évoque un film (année, pas de saison).
    pub fn is_movie_like(&self) -> bool {
        !self.is_series_like() && self.year.is_some()
    }
    /// Un score de qualité grossier pour comparer deux versions d'un même média.
    pub fn quality_score(&self) -> u32 {
        let res = match self.resolution.as_deref() {
            Some("2160p") => 40,
            Some("1080p") => 30,
            Some("720p") => 20,
            Some("576p") | Some("480p") => 10,
            _ => 15,
        };
        let src = match self.source.as_deref() {
            Some("Remux") => 8,
            Some("BluRay") => 6,
            Some("WEB-DL") | Some("WEB") => 5,
            Some("WEBRip") => 4,
            Some("HDTV") => 3,
            Some("DVD") => 2,
            _ => 3,
        };
        let multi = if self.langs.iter().any(|l| l == "MULTI") { 3 } else { 0 };
        res + src + multi + if self.hdr { 1 } else { 0 }
    }
}

const VIDEO_EXT: &[&str] = &[
    "mkv", "mp4", "avi", "m4v", "mov", "wmv", "ts", "m2ts", "mpg", "mpeg", "webm", "flv", "iso",
];

pub fn is_video_ext(ext: &str) -> bool {
    VIDEO_EXT.contains(&ext.to_lowercase().as_str())
}

fn re_season_ep() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)^s(\d{1,3})(?:e(\d{1,4})(?:e?-?e?(\d{1,4}))?)?$").unwrap()
    })
}
fn re_x_ep() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)^(\d{1,2})x(\d{2,3})$").unwrap())
}
fn re_resolution() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)^(\d{3,4})[pi]$").unwrap())
}
fn re_year() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^(19\d\d|20\d\d)$").unwrap())
}

#[derive(Debug, PartialEq, Clone)]
enum Tok {
    Year(u16),
    SeasonEp(u16, Option<u16>, Option<u16>),
    SeasonWord, // « Saison » / « Season » : le nombre suit
    Complete,
    Resolution(String),
    Source(String),
    Codec(String),
    Lang(String),
    Hdr,
    Doc,
    BareRes(String), // « 1080 » sans « p »
    Service, // plateforme de streaming (NF, AMZN…)
    Junk, // audio, drapeaux (REPACK, 10bit…) : fait partie de la partie technique
    Word,
}

fn classify(t: &str) -> Tok {
    let mut l = t.to_lowercase();
    // « L'intégrale », « D'Artagnan » : on juge le mot après l'apostrophe
    if let Some((a, b)) = l.clone().split_once(['\'', '’']) {
        if a.chars().count() <= 2 && !b.is_empty() {
            l = b.to_string();
        }
    }
    if re_year().is_match(&l) {
        return Tok::Year(l.parse().unwrap());
    }
    if let Some(c) = re_season_ep().captures(&l) {
        let s = c[1].parse().unwrap();
        let e = c.get(2).map(|m| m.as_str().parse().unwrap());
        let e2 = c.get(3).map(|m| m.as_str().parse().unwrap());
        return Tok::SeasonEp(s, e, e2);
    }
    if let Some(c) = re_x_ep().captures(&l) {
        return Tok::SeasonEp(c[1].parse().unwrap(), Some(c[2].parse().unwrap()), None);
    }
    if re_resolution().is_match(&l) {
        let n: u32 = l[..l.len() - 1].parse().unwrap();
        if n >= 360 {
            return Tok::Resolution(format!("{n}p"));
        }
    }
    match l.as_str() {
        "4k" | "uhd" => Tok::Resolution("2160p".into()),
        "saison" | "season" | "seizoen" | "temporada" => Tok::SeasonWord,
        "complete" | "integrale" | "intégrale" | "completa" | "integral" | "complet" => Tok::Complete,
        "remux" => Tok::Source("Remux".into()),
        "bluray" | "blu" | "bdrip" | "brrip" | "bd" | "bdremux" => Tok::Source("BluRay".into()),
        "webdl" | "web" => Tok::Source("WEB".into()),
        // plateformes : ne coupent jamais le titre (« Red Rose », « Max »)
        "amzn" | "nf" | "dsnp" | "atvp" | "hmax" | "max" | "pcok" | "hulu" | "pmtp" | "cr"
        | "stan" | "itv" | "ctv" | "red" | "sho" | "hbo" => Tok::Service,
        "webrip" => Tok::Source("WEBRip".into()),
        "hdtv" | "pdtv" | "tvrip" | "hdrip" => Tok::Source("HDTV".into()),
        "dvdrip" | "dvd" | "dvdr" | "dvd5" | "dvd9" => Tok::Source("DVD".into()),
        "x264" | "h264" | "avc" => Tok::Codec("H.264".into()),
        "x265" | "h265" | "hevc" => Tok::Codec("H.265".into()),
        "av1" => Tok::Codec("AV1".into()),
        "xvid" | "divx" | "mpeg" | "mpeg2" | "vc1" => Tok::Codec(l.to_uppercase()),
        "multi" | "multi2" | "multilang" | "multilingue" => Tok::Lang("MULTI".into()),
        "french" | "vff" | "vf" | "vf2" | "vfi" | "vfq" | "vof"
        | "truefrench" | "vfb" => Tok::Lang("FR".into()),
        "vostfr" | "subfrench" | "vostf" => Tok::Lang("VOSTFR".into()),
        "english" | "vo" | "eng" => Tok::Lang("EN".into()),
        "german" | "ger" | "deutsch" => Tok::Lang("DE".into()),
        "spanish" | "castellano" | "latino" | "espanol" | "español" => Tok::Lang("ES".into()),
        "1080" | "720" | "2160" | "480" | "576" => Tok::BareRes(l.clone()),
        "italian" | "ita" => Tok::Lang("IT".into()),
        "hdr" | "hdr10" | "hdr10plus" | "dv" | "dolby" | "dovi" => Tok::Hdr,
        "doc" | "docu" | "documentaire" | "documentary" => Tok::Doc,
        "sdr" | "10bit" | "10bits" | "8bit" | "ac3" | "eac3" | "dd" | "ddp" | "ddp2" | "ddp5"
        | "dd5" | "dd2" | "dts" | "dtshd" | "aac" | "aac2" | "atmos" | "truehd" | "flac" | "mp3"
        | "ad" | "internal" | "repack" | "proper" | "extended" | "unrated" | "remastered"
        | "imax" | "hybrid" | "hfr" | "dl" | "ma" | "hd" | "sd" | "v2" | "v3" | "limited"
        | "nfo" | "readnfo" | "subbed" | "dubbed" | "srt" | "h" | "x" => Tok::Junk,
        _ => {
            // DDP5, DD5, AAC2, H264… suivis de chiffres
            if l.starts_with("ddp") || l.starts_with("dd5") || l.starts_with("aac") {
                Tok::Junk
            } else {
                Tok::Word
            }
        }
    }
}

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| matches!(c, '.' | '_' | ' ' | '-' | '[' | ']' | '(' | ')' | '{' | '}' | '+'))
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// Analyse un nom de dossier ou de fichier.
pub fn parse(name: &str) -> Release {
    let mut raw = name.trim().to_string();
    if let Some((stem, ext)) = raw.rsplit_once('.') {
        if is_video_ext(ext) {
            raw = stem.to_string();
        }
    }
    let toks = tokenize(&raw);
    let kinds: Vec<Tok> = toks.iter().map(|t| classify(t)).collect();
    let mut r = Release { raw: raw.clone(), ..Default::default() };

    // Premier marqueur technique : tout ce qui précède est le titre.
    // Une année compte comme marqueur sauf si c'est la seule chose avant elle (« 1995.2024 »).
    let is_marker = |i: usize| -> bool {
        match &kinds[i] {
            Tok::Word | Tok::Service | Tok::Junk => false,
            Tok::BareRes(_) => i > 0,
            Tok::Year(_) => true,
            Tok::SeasonWord => {
                // « Saison 2 » seulement si un nombre suit
                toks.get(i + 1).and_then(|n| n.parse::<u16>().ok()).map_or(false, |n| n > 0 && n < 100)
            }
            Tok::Doc => i > 0,
            Tok::Complete => i > 0,
            Tok::SeasonEp(..) => true,
            _ => i > 0,
        }
    };
    let mut first_marker = toks.len();
    for i in 0..toks.len() {
        if i == 0 && matches!(kinds[0], Tok::Year(_)) {
            // un titre qui commence par une année (« 1995 », « 2080 Nos Futurs »)
            continue;
        }
        if is_marker(i) {
            // une année isolée suivie d'un mot ordinaire sans marqueur plus loin n'est pas un marqueur
            if let Tok::Year(_) = kinds[i] {
                let later_marker = (i + 1..toks.len()).any(|j| {
                    !matches!(kinds[j], Tok::Word | Tok::Year(_)) && is_marker(j)
                });
                let next_is_word = kinds.get(i + 1) == Some(&Tok::Word);
                if next_is_word && !later_marker {
                    continue;
                }
                // « Blade Runner 2049 2017 » : si une autre année suit directement, c'est elle l'année
                if let Some(Tok::Year(_)) = kinds.get(i + 1) {
                    continue;
                }
            }
            first_marker = i;
            break;
        }
    }
    // Année : celle qui est juste avant le premier marqueur non-année, sinon la 1re rencontrée après le titre.
    let mut title_end = first_marker;
    if first_marker < toks.len() {
        if let Tok::Year(y) = kinds[first_marker] {
            r.year = Some(y);
        }
    }
    // Un titre vide (nom qui commence par un marqueur) : dossier « S01 » ou « Saison_01 (2004) »
    if title_end > toks.len() {
        title_end = toks.len();
    }
    let mut tt: Vec<&str> = toks[..title_end].iter().map(|t| t.as_str()).collect();
    while tt.len() > 1
        && matches!(
            tt.last().map(|t| t.to_lowercase()).as_deref(),
            Some("nf" | "amzn" | "dsnp" | "atvp" | "hmax" | "pcok" | "hulu" | "pmtp")
        )
    {
        tt.pop();
    }
    r.title = tt.join(" ");

    // Parcours de la partie technique
    let mut i = title_end;
    let mut season_marks: Vec<u16> = Vec::new();
    while i < toks.len() {
        match &kinds[i] {
            Tok::Year(y) => {
                if r.year.is_none() {
                    r.year = Some(*y);
                }
            }
            Tok::SeasonEp(s, e, e2) => {
                season_marks.push(*s);
                if let Some(e) = e {
                    r.episodes.push(*e);
                    if let Some(e2) = e2 {
                        if e2 > e {
                            for k in (*e + 1)..=*e2 {
                                r.episodes.push(k);
                            }
                        }
                    }
                }
            }
            Tok::SeasonWord => {
                if let Some(n) = toks.get(i + 1).and_then(|n| n.parse::<u16>().ok()).filter(|n| *n > 0 && *n < 100) {
                    season_marks.push(n);
                    i += 1;
                }
            }
            Tok::Complete => r.complete = true,
            Tok::Service => {
                if r.source.is_none() {
                    r.source = Some("WEB".into());
                }
            }
            Tok::BareRes(n) => {
                if r.resolution.is_none() {
                    r.resolution = Some(format!("{n}p"));
                }
            }
            Tok::Resolution(s) => {
                if r.resolution.is_none() {
                    r.resolution = Some(s.clone());
                }
            }
            Tok::Source(s) => {
                // Remux > BluRay > WEB…
                let better = match (&r.source, s.as_str()) {
                    (None, _) => true,
                    (Some(c), "Remux") => c != "Remux",
                    (Some(c), "WEBRip") => c == "WEB" && false,
                    _ => false,
                };
                if better {
                    r.source = Some(s.clone());
                }
            }
            Tok::Codec(c) => {
                if r.codec.is_none() {
                    r.codec = Some(c.clone());
                }
            }
            Tok::Lang(l) => {
                if !r.langs.contains(l) {
                    r.langs.push(l.clone());
                }
            }
            Tok::Hdr => r.hdr = true,
            Tok::Doc => r.doc_hint = true,
            _ => {}
        }
        i += 1;
    }
    // Saisons : S01-S17 → plage
    if !season_marks.is_empty() {
        let lo = *season_marks.iter().min().unwrap();
        let hi = *season_marks.iter().max().unwrap();
        if lo != hi {
            r.seasons = (lo..=hi).collect();
            r.complete = true;
        } else {
            r.seasons = vec![lo];
        }
    }
    // Équipe : dernier mot, s'il suit un tiret et vient après un marqueur
    if title_end < toks.len() {
        if let Some(last) = toks.last() {
            if kinds.last() == Some(&Tok::Word) && toks.len() > title_end + 1 {
                if let Some(pos) = raw.rfind(last.as_str()) {
                    let before = raw[..pos].chars().rev().find(|c| !c.is_whitespace());
                    if matches!(before, Some('-') | Some(' ')) || before.is_some() {
                        r.group = Some(last.clone());
                    }
                }
            }
        }
    }
    // Clean-up : retirer les « - » traînants du titre
    r.title = r.title.trim().to_string();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn film_simple() {
        let r = parse("10.Minutes.Gone.2019.MULTi.1080p.BluRay.x264-LOST.mkv");
        assert_eq!(r.title, "10 Minutes Gone");
        assert_eq!(r.year, Some(2019));
        assert_eq!(r.resolution.as_deref(), Some("1080p"));
        assert_eq!(r.source.as_deref(), Some("BluRay"));
        assert_eq!(r.codec.as_deref(), Some("H.264"));
        assert!(r.langs.contains(&"MULTI".to_string()));
        assert_eq!(r.group.as_deref(), Some("LOST"));
        assert!(r.is_movie_like());
    }

    #[test]
    fn titre_qui_est_une_annee() {
        let r = parse("1995.2024.FRENCH.1080p.AMZN.WEB.DDP5.1.H265-TFA.mkv");
        assert_eq!(r.title, "1995");
        assert_eq!(r.year, Some(2024));
    }

    #[test]
    fn titre_commence_par_nombre_puis_annee() {
        let r = parse("2080.Nos.Futurs.2023.DOC.S01.FRENCH.1080p.WEB.x264-TFA");
        assert_eq!(r.title, "2080 Nos Futurs");
        assert_eq!(r.year, Some(2023));
        assert_eq!(r.seasons, vec![1]);
        assert!(r.doc_hint);
    }

    #[test]
    fn annee_dans_le_titre() {
        let r = parse("Blade.Runner.2049.2017.MULTi.1080p.BluRay.x265-X.mkv");
        assert_eq!(r.title, "Blade Runner 2049");
        assert_eq!(r.year, Some(2017));
    }

    #[test]
    fn serie_saison() {
        let r = parse("Poker.Face.2023.S02.MULTi.VFF.1080p.WEBRip.x264.Eac3-NOTAG");
        assert_eq!(r.title, "Poker Face");
        assert_eq!(r.year, Some(2023));
        assert_eq!(r.seasons, vec![2]);
        assert!(r.is_series_like());
    }

    #[test]
    fn serie_sans_annee() {
        let r = parse("Tapie.S01.FRENCH.1080p.WEB.x264-FW");
        assert_eq!(r.title, "Tapie");
        assert_eq!(r.year, None);
        assert_eq!(r.seasons, vec![1]);
    }

    #[test]
    fn plage_de_saisons() {
        let r = parse("American Dad.S01-S17.MULTI.TRUEFRENCH.1080p.WEB.x264-FTMVHD");
        assert_eq!(r.title, "American Dad");
        assert_eq!(r.seasons.len(), 17);
        assert!(r.complete);
    }

    #[test]
    fn episode() {
        let r = parse("Eric.2024.S01E03.MULTi.1080p.WEB.x264-FW.mkv");
        assert_eq!(r.title, "Eric");
        assert_eq!(r.seasons, vec![1]);
        assert_eq!(r.episodes, vec![3]);
    }

    #[test]
    fn double_episode() {
        let r = parse("Show.S01E01E02.1080p.WEB.mkv");
        assert_eq!(r.episodes, vec![1, 2]);
    }

    #[test]
    fn pack_complet() {
        let r = parse("Californication.INTEGRALE.MULTi.1080p.BluRay.x264-ZT");
        assert_eq!(r.title, "Californication");
        assert!(r.complete);
        assert!(r.is_series_like());
    }

    #[test]
    fn dossier_saison_seul() {
        let r = parse("Saison_01 (2004)");
        assert_eq!(r.title, "");
        assert_eq!(r.seasons, vec![1]);
        assert_eq!(r.year, Some(2004));
        let r = parse("S01");
        assert_eq!(r.title, "");
        assert_eq!(r.seasons, vec![1]);
    }

    #[test]
    fn nom_simple_sans_marqueur() {
        let r = parse("bureau des legendes");
        assert_eq!(r.title, "bureau des legendes");
        assert!(!r.is_series_like() && !r.is_movie_like());
    }

    #[test]
    fn separateurs_espaces() {
        let r = parse("21 Jump Street 2012 MULTI VFF 1080p BluRay HDR10 E-AC3 x265-Winks.mkv");
        assert_eq!(r.title, "21 Jump Street");
        assert_eq!(r.year, Some(2012));
        assert!(r.hdr);
    }

    #[test]
    fn style_mhdgz() {
        let r = parse("1001 Pattes - Multi -1080p mHDgz.mkv");
        assert_eq!(r.title, "1001 Pattes");
        assert_eq!(r.resolution.as_deref(), Some("1080p"));
    }

    #[test]
    fn titre_avec_mot_de_plateforme_ou_de_langue() {
        let r = parse("Red.Rose.S01.MULTI.1080p.WEBRip.DDP5.1.x264");
        assert_eq!(r.title, "Red Rose");
        let r = parse("Outreau.Un.Cauchemar.Francais.2024.S01.VOF.DOC.WEB-DL.1080.H265-KmK");
        assert_eq!(r.title, "Outreau Un Cauchemar Francais");
        assert_eq!(r.resolution.as_deref(), Some("1080p"));
        assert_eq!(r.group.as_deref(), Some("KmK"));
    }

    #[test]
    fn saison_dans_le_titre() {
        let r = parse("Hors.Saison.2024.FRENCH.1080p.WEB-DL.H264-Slay3R.mkv");
        assert_eq!(r.title, "Hors Saison");
        assert_eq!(r.year, Some(2024));
        assert!(r.seasons.is_empty());
    }

    #[test]
    fn integrale_apostrophe() {
        let r = parse("Caméra.Café.L'intégrale.French.Dvdrip");
        assert_eq!(r.title, "Caméra Café");
        assert!(r.complete);
    }

    #[test]
    fn mot_saison() {
        let r = parse("Friends Saison 3 MULTI 1080p");
        assert_eq!(r.title, "Friends");
        assert_eq!(r.seasons, vec![3]);
    }
}
