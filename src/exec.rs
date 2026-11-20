//! Exécution réelle d'un rangement : copie sûre (reflink → lien physique → copie), vérification,
//! historique et annulation. Ce module n'est appelé que si le « mode essai à blanc » est désactivé.

use crate::config::Category;
use crate::plan::{check_settled, Renommage, Settle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// reflink, sinon lien physique, sinon copie
    #[default]
    Auto,
    Reflink,
    Hardlink,
    Copy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Copy,
    Move,
}

impl Action {
    pub fn from_str(s: &str) -> Action {
        if s == "move" { Action::Move } else { Action::Copy }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fichier {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub taille: u64,
    pub methode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub t: u64,
    pub nom: String,
    pub titre: String,
    pub categorie: Option<Category>,
    pub action: Action,
    pub vers: PathBuf,
    pub fichiers: Vec<Fichier>,
    /// Dossiers créés par ce rangement (pour les retirer à l'annulation, s'ils sont vides).
    pub dossiers_crees: Vec<PathBuf>,
    pub taille_totale: u64,
    /// « fait », « annule » ou « erreur »
    pub statut: String,
    pub motif: String,
}

#[derive(Debug)]
pub enum ExecError {
    Existe(PathBuf),
    SourceAbsente(PathBuf),
    PasPret(String),
    Io(String),
    Verification(String),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Existe(p) => write!(f, "la destination existe déjà : {}", p.display()),
            ExecError::SourceAbsente(p) => write!(f, "l'original a disparu : {}", p.display()),
            ExecError::PasPret(s) => write!(f, "pas prêt : {s}"),
            ExecError::Io(s) => write!(f, "{s}"),
            ExecError::Verification(s) => write!(f, "vérification échouée : {s}"),
        }
    }
}

fn io_err(ctx: &str, e: io::Error) -> ExecError {
    let msg = match e.raw_os_error() {
        Some(28) => "plus de place sur le disque de destination".to_string(),
        Some(13) | Some(1) => "accès refusé".to_string(),
        Some(30) => "destination en lecture seule".to_string(),
        _ => e.to_string(),
    };
    ExecError::Io(format!("{ctx} : {msg}"))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Identifiant unique d'un rangement : heure en nanosecondes + compteur (plusieurs rangements dans la même seconde).
pub fn unique_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}-{:x}", nanos, N.fetch_add(1, Ordering::Relaxed))
}

const FICLONE: libc::c_ulong = 0x4004_9409;

fn try_reflink(src: &Path, dst: &Path) -> io::Result<()> {
    let s = fs::File::open(src)?;
    let d = fs::OpenOptions::new().write(true).create_new(true).open(dst)?;
    let r = unsafe { libc::ioctl(d.as_raw_fd(), FICLONE, s.as_raw_fd()) };
    if r != 0 {
        let e = io::Error::last_os_error();
        drop(d);
        let _ = fs::remove_file(dst);
        return Err(e);
    }
    Ok(())
}

/// Copie un fichier avec la meilleure méthode disponible. Renvoie la méthode employée.
fn place_file(src: &Path, dst: &Path, pref: Method) -> Result<&'static str, ExecError> {
    let order: &[Method] = match pref {
        Method::Auto => &[Method::Reflink, Method::Hardlink, Method::Copy],
        Method::Reflink => &[Method::Reflink],
        Method::Hardlink => &[Method::Hardlink],
        Method::Copy => &[Method::Copy],
    };
    for (i, m) in order.iter().enumerate() {
        let res = match m {
            Method::Reflink => try_reflink(src, dst).map(|_| "reflink"),
            Method::Hardlink => fs::hard_link(src, dst).map(|_| "lien physique"),
            _ => fs::copy(src, dst).map(|_| "copie"),
        };
        match res {
            Ok(name) => {
                // Les copies gardent la date d'origine (Resilio la conserve aussi).
                if name != "lien physique" {
                    if let Ok(mt) = fs::metadata(src).and_then(|m| m.modified()) {
                        if let Ok(f) = fs::OpenOptions::new().write(true).open(dst) {
                            let _ = f.set_modified(mt);
                        }
                    }
                }
                return Ok(name);
            }
            Err(e) => {
                // On passe à la méthode suivante seulement pour « non supporté / autre volume / droits ».
                let recoverable = matches!(e.raw_os_error(), Some(18) | Some(95) | Some(22) | Some(25) | Some(1) | Some(31) | Some(38));
                let _ = fs::remove_file(dst);
                if i + 1 == order.len() || !recoverable {
                    return Err(io_err(&format!("copie de {}", src.display()), e));
                }
            }
        }
    }
    Err(ExecError::Io("aucune méthode de copie disponible".into()))
}

fn first_missing_ancestors(p: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut cur = Some(p);
    while let Some(c) = cur {
        if c.exists() {
            break;
        }
        v.push(c.to_path_buf());
        cur = c.parent();
    }
    v.reverse();
    v
}

pub struct Request<'a> {
    pub nom: &'a str,
    pub titre: &'a str,
    pub categorie: Option<Category>,
    pub source: &'a Path,
    pub vers: &'a Path,
    pub renommages: &'a [Renommage],
    pub action: Action,
    pub method: Method,
    pub settle_minutes: u64,
}

/// Range un élément. En cas d'échec, rien n'est laissé à moitié fait et l'original n'est jamais touché.
pub fn execute(r: &Request) -> Result<Entry, ExecError> {
    if !r.source.exists() {
        return Err(ExecError::SourceAbsente(r.source.to_path_buf()));
    }
    // Nouvelle vérification juste avant de copier : l'élément ne doit plus bouger.
    match check_settled(r.source, r.settle_minutes) {
        Settle::Pret => {}
        Settle::Transfert { fichier } => return Err(ExecError::PasPret(format!("transfert en cours ({fichier})"))),
        Settle::Attente { minutes } => return Err(ExecError::PasPret(format!("modifié récemment, encore {minutes} min"))),
    }
    if r.vers.exists() {
        return Err(ExecError::Existe(r.vers.to_path_buf()));
    }
    if r.vers.starts_with(r.source) || r.source.starts_with(r.vers) {
        return Err(ExecError::Io("destination et origine se chevauchent".into()));
    }
    let id = unique_id();
    let parent = r.vers.parent().ok_or_else(|| ExecError::Io("destination sans dossier parent".into()))?;
    let created = first_missing_ancestors(parent);
    fs::create_dir_all(parent).map_err(|e| io_err(&format!("création de {}", parent.display()), e))?;
    let staging_root = parent.join(format!(".kesal-en-cours-{id}"));
    let final_name = r.vers.file_name().ok_or_else(|| ExecError::Io("nom de destination vide".into()))?;
    let staging = staging_root.join(final_name);

    let cleanup = |staging_root: &Path, created: &[PathBuf]| {
        let _ = fs::remove_dir_all(staging_root); // uniquement notre dossier provisoire
        for d in created.iter().rev() {
            let _ = fs::remove_dir(d); // seulement s'il est vide
        }
    };

    let rename_map: HashMap<&str, &str> = r.renommages.iter().map(|x| (x.ancien.as_str(), x.nouveau.as_str())).collect();
    let mut fichiers: Vec<Fichier> = Vec::new();
    let result: Result<(), ExecError> = (|| {
        fs::create_dir_all(&staging_root).map_err(|e| io_err("dossier provisoire", e))?;
        if r.source.is_file() {
            let dst = &staging;
            let m = place_file(r.source, dst, r.method)?;
            fichiers.push(Fichier { src: r.source.to_path_buf(), dst: r.vers.to_path_buf(), taille: fs::metadata(r.source).map(|m| m.len()).unwrap_or(0), methode: m.into() });
        } else {
            for e in WalkDir::new(r.source).min_depth(1).into_iter().filter_map(|e| e.ok()) {
                let name = e.file_name().to_string_lossy();
                if crate::library::is_junk_name(&name) {
                    continue;
                }
                let rel = e.path().strip_prefix(r.source).unwrap().to_string_lossy().to_string();
                let rel_new = rename_map.get(rel.as_str()).map(|s| s.to_string()).unwrap_or(rel.clone());
                let dst = staging.join(&rel_new);
                if e.file_type().is_dir() {
                    fs::create_dir_all(&dst).map_err(|e| io_err("création de dossier", e))?;
                } else if e.file_type().is_file() {
                    if let Some(p) = dst.parent() {
                        fs::create_dir_all(p).map_err(|e| io_err("création de dossier", e))?;
                    }
                    let m = place_file(e.path(), &dst, r.method)?;
                    fichiers.push(Fichier { src: e.path().to_path_buf(), dst: r.vers.join(&rel_new), taille: e.metadata().map(|m| m.len()).unwrap_or(0), methode: m.into() });
                }
            }
            if !staging.exists() {
                fs::create_dir_all(&staging).map_err(|e| io_err("création de dossier", e))?;
            }
        }
        // Vérification : chaque fichier est là, avec la bonne taille.
        for f in &fichiers {
            let staged = if r.source.is_file() { staging.clone() } else { staging.join(f.dst.strip_prefix(r.vers).unwrap()) };
            let got = fs::metadata(&staged).map(|m| m.len()).map_err(|e| ExecError::Verification(format!("{} : {e}", staged.display())))?;
            if got != f.taille {
                return Err(ExecError::Verification(format!("{} : {got} octets au lieu de {}", staged.display(), f.taille)));
            }
        }
        // Le fichier d'origine a-t-il bougé pendant la copie ?
        for f in &fichiers {
            let now_size = fs::metadata(&f.src).map(|m| m.len()).unwrap_or(0);
            if now_size != f.taille {
                return Err(ExecError::Verification(format!("{} a changé pendant la copie", f.src.display())));
            }
        }
        if r.vers.exists() {
            return Err(ExecError::Existe(r.vers.to_path_buf()));
        }
        fs::rename(&staging, r.vers).map_err(|e| io_err("mise en place finale", e))?;
        Ok(())
    })();
    if let Err(e) = result {
        cleanup(&staging_root, &created);
        return Err(e);
    }
    let _ = fs::remove_dir(&staging_root);

    // Déplacer : l'original n'est retiré qu'après la mise en place vérifiée.
    if r.action == Action::Move {
        for f in &fichiers {
            let _ = fs::remove_file(&f.src);
        }
        if r.source.is_dir() {
            // dossiers vides restants, du plus profond au plus haut
            let mut dirs: Vec<PathBuf> = WalkDir::new(r.source).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_dir()).map(|e| e.into_path()).collect();
            dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
            for d in dirs {
                let _ = fs::remove_dir(&d);
            }
        }
    }
    let total = fichiers.iter().map(|f| f.taille).sum();
    Ok(Entry {
        id,
        t: now(),
        nom: r.nom.to_string(),
        titre: r.titre.to_string(),
        categorie: r.categorie,
        action: r.action,
        vers: r.vers.to_path_buf(),
        fichiers,
        dossiers_crees: created,
        taille_totale: total,
        statut: "fait".into(),
        motif: String::new(),
    })
}

/// Annule un rangement : retire ce qui a été créé (et remet l'original pour un déplacement).
/// Ne supprime que les fichiers listés dans l'historique, et seulement s'ils n'ont pas été modifiés depuis.
pub fn undo(e: &Entry) -> Result<(), ExecError> {
    if e.statut != "fait" {
        return Err(ExecError::Io("ce rangement n'est pas actif".into()));
    }
    // Contrôle avant toute suppression.
    for f in &e.fichiers {
        match fs::metadata(&f.dst) {
            Ok(m) if m.len() != f.taille => return Err(ExecError::Verification(format!("{} a été modifié depuis le rangement : annulation refusée", f.dst.display()))),
            _ => {}
        }
    }
    for f in &e.fichiers {
        if !f.dst.exists() {
            continue;
        }
        if e.action == Action::Move {
            if let Some(p) = f.src.parent() {
                fs::create_dir_all(p).map_err(|er| io_err("recréation du dossier d'origine", er))?;
            }
            if f.src.exists() {
                return Err(ExecError::Existe(f.src.clone()));
            }
            place_file(&f.dst, &f.src, Method::Auto)?;
            if fs::metadata(&f.src).map(|m| m.len()).unwrap_or(0) != f.taille {
                return Err(ExecError::Verification(format!("remise en place de {} incomplète", f.src.display())));
            }
        }
        fs::remove_file(&f.dst).map_err(|er| io_err(&format!("suppression de {}", f.dst.display()), er))?;
    }
    // dossiers vides, du plus profond au plus haut ; puis ceux qu'on avait créés
    if e.vers.is_dir() {
        let mut dirs: Vec<PathBuf> = WalkDir::new(&e.vers).into_iter().filter_map(|x| x.ok()).filter(|x| x.file_type().is_dir()).map(|x| x.into_path()).collect();
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for d in dirs {
            let _ = fs::remove_dir(&d);
        }
    }
    for d in e.dossiers_crees.iter().rev() {
        let _ = fs::remove_dir(d);
    }
    Ok(())
}

/// Historique des rangements, enregistré dans `history.json`.
pub struct History {
    path: PathBuf,
    entries: Mutex<Vec<Entry>>,
}

impl History {
    pub fn load(data_dir: &Path) -> History {
        let path = data_dir.join("history.json");
        let entries = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        History { path, entries: Mutex::new(entries) }
    }
    fn save(&self, v: &[Entry]) {
        if let Some(d) = self.path.parent() {
            let _ = fs::create_dir_all(d);
        }
        if let Ok(t) = serde_json::to_string_pretty(v) {
            let tmp = self.path.with_extension("json.tmp");
            if fs::write(&tmp, t).is_ok() {
                let _ = fs::rename(&tmp, &self.path);
            }
        }
    }
    pub fn add(&self, e: Entry) {
        let mut v = self.entries.lock().unwrap();
        v.push(e);
        self.save(&v);
    }
    pub fn all(&self) -> Vec<Entry> {
        self.entries.lock().unwrap().clone()
    }
    pub fn get(&self, id: &str) -> Option<Entry> {
        self.entries.lock().unwrap().iter().find(|e| e.id == id).cloned()
    }
    pub fn set_status(&self, id: &str, statut: &str, motif: &str) {
        let mut v = self.entries.lock().unwrap();
        if let Some(e) = v.iter_mut().find(|e| e.id == id) {
            e.statut = statut.into();
            e.motif = motif.into();
        }
        self.save(&v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    /// Écrit un fichier et le vieillit de 2 h (pour qu'il soit « calme »).
    fn mk(p: &Path, content: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
        let f = fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(7200)).unwrap();
    }
    fn age_dir(p: &Path) {
        for e in WalkDir::new(p).into_iter().filter_map(|e| e.ok()) {
            if e.file_type().is_dir() {
                let f = fs::File::open(e.path()).unwrap();
                f.set_modified(SystemTime::now() - Duration::from_secs(7200)).unwrap();
            }
        }
    }
    fn req<'a>(src: &'a Path, vers: &'a Path, ren: &'a [Renommage], action: Action, method: Method) -> Request<'a> {
        Request { nom: "x", titre: "X", categorie: Some(Category::Serie), source: src, vers, renommages: ren, action, method, settle_minutes: 30 }
    }

    #[test]
    fn film_lien_physique_original_intact() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/Film.2026.mkv");
        mk(&src, "contenu du film");
        let vers = t.path().join("films/Film.2026.mkv");
        let e = execute(&req(&src, &vers, &[], Action::Copy, Method::Auto)).unwrap();
        assert_eq!(fs::read_to_string(&vers).unwrap(), "contenu du film");
        assert!(src.exists(), "l'original doit rester");
        // sur le même volume ext4 : pas de reflink, donc lien physique
        assert_eq!(e.fichiers[0].methode, "lien physique");
        assert_eq!(fs::metadata(&src).unwrap().ino(), fs::metadata(&vers).unwrap().ino());
        assert!(!t.path().join("films").read_dir().unwrap().any(|x| x.unwrap().file_name().to_string_lossy().starts_with(".kesal")));
    }

    #[test]
    fn copie_forcee_donne_un_autre_fichier() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/a.mkv");
        mk(&src, "abc");
        let vers = t.path().join("out/a.mkv");
        let e = execute(&req(&src, &vers, &[], Action::Copy, Method::Copy)).unwrap();
        assert_eq!(e.fichiers[0].methode, "copie");
        assert_ne!(fs::metadata(&src).unwrap().ino(), fs::metadata(&vers).unwrap().ino());
        // la date d'origine est gardée
        let d = fs::metadata(&src).unwrap().modified().unwrap().duration_since(fs::metadata(&vers).unwrap().modified().unwrap().min(fs::metadata(&src).unwrap().modified().unwrap())).unwrap_or_default();
        assert!(d < Duration::from_secs(2));
    }

    #[test]
    fn reflink_force_echoue_proprement_sur_ext4() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/a.mkv");
        mk(&src, "abc");
        let vers = t.path().join("out/a.mkv");
        let r = execute(&req(&src, &vers, &[], Action::Copy, Method::Reflink));
        // ext4 n'a pas de reflink : erreur claire, rien de laissé derrière
        assert!(r.is_err());
        assert!(!vers.exists());
        assert!(src.exists());
        assert!(!t.path().join("out").exists(), "le dossier créé doit être retiré");
    }

    #[test]
    fn dossier_serie_avec_renumerotation() {
        let t = tempfile::tempdir().unwrap();
        let rel = "Show.S02.FRENCH-X";
        let f1 = format!("{rel}/Show.S02E01.mkv");
        let f2 = format!("{rel}/Show.S02E02.mkv");
        let src = t.path().join("in").join(rel);
        mk(&t.path().join("in").join(&f1), "ep1");
        mk(&t.path().join("in").join(&f2), "episode deux");
        mk(&src.join("Show.S02.nfo"), "info");
        age_dir(&t.path().join("in"));
        let ren = vec![
            Renommage { ancien: "Show.S02E01.mkv".into(), nouveau: "Show.S01E01.mkv".into() },
            Renommage { ancien: "Show.S02E02.mkv".into(), nouveau: "Show.S01E02.mkv".into() },
            Renommage { ancien: "Show.S02.nfo".into(), nouveau: "Show.S01.nfo".into() },
        ];
        fs::create_dir_all(t.path().join("series")).unwrap(); // la racine de la bibliothèque existe déjà
        let vers = t.path().join("series/Show (2019)/Show.S01.FRENCH-X");
        let e = execute(&req(&src, &vers, &ren, Action::Copy, Method::Auto)).unwrap();
        assert!(vers.join("Show.S01E01.mkv").exists() && vers.join("Show.S01E02.mkv").exists() && vers.join("Show.S01.nfo").exists());
        assert!(!vers.join("Show.S02E01.mkv").exists());
        assert_eq!(e.fichiers.len(), 3);
        assert_eq!(e.dossiers_crees.len(), 1, "le dossier « Show (2019) » a été créé");
        assert!(src.join("Show.S02E01.mkv").exists(), "l'original garde ses noms");
    }

    #[test]
    fn destination_existante_refusee_sans_rien_changer() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/a.mkv");
        mk(&src, "nouveau");
        let vers = t.path().join("out/a.mkv");
        mk(&vers, "ancien");
        let r = execute(&req(&src, &vers, &[], Action::Copy, Method::Auto));
        assert!(matches!(r, Err(ExecError::Existe(_))));
        assert_eq!(fs::read_to_string(&vers).unwrap(), "ancien");
    }

    #[test]
    fn original_disparu() {
        let t = tempfile::tempdir().unwrap();
        let r = execute(&req(&t.path().join("in/rien.mkv"), &t.path().join("out/rien.mkv"), &[], Action::Copy, Method::Auto));
        assert!(matches!(r, Err(ExecError::SourceAbsente(_))));
        assert!(!t.path().join("out").exists());
    }

    #[test]
    fn element_pas_encore_calme_refuse() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/a.mkv");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, "tout juste arrivé").unwrap();
        let r = execute(&req(&src, &t.path().join("out/a.mkv"), &[], Action::Copy, Method::Auto));
        assert!(matches!(r, Err(ExecError::PasPret(_))), "{r:?}");
        assert!(!t.path().join("out").exists());
    }

    #[test]
    fn transfert_resilio_en_cours_refuse() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("in/Show.S01");
        mk(&dir.join("Show.S01E01.mkv"), "fini");
        mk(&dir.join("Show.S01E02.mkv.!sync"), "en cours");
        age_dir(&t.path().join("in"));
        let r = execute(&req(&dir, &t.path().join("out/Show.S01"), &[], Action::Copy, Method::Auto));
        assert!(matches!(r, Err(ExecError::PasPret(ref m)) if m.contains("!sync")), "{r:?}");
        assert!(!t.path().join("out").exists());
    }

    #[test]
    fn destination_en_lecture_seule_ne_laisse_rien() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/a.mkv");
        mk(&src, "abc");
        let out = t.path().join("out");
        fs::create_dir_all(&out).unwrap();
        fs::set_permissions(&out, fs::Permissions::from_mode(0o555)).unwrap();
        let vers = out.join("a.mkv");
        let r = execute(&req(&src, &vers, &[], Action::Copy, Method::Auto));
        fs::set_permissions(&out, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err());
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0, "aucun reste dans la destination");
        assert!(src.exists());
    }

    #[test]
    fn annulation_retire_la_copie_et_laisse_l_original() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/a.mkv");
        mk(&src, "abc");
        fs::create_dir_all(t.path().join("series")).unwrap(); // la racine de la bibliothèque existe déjà
        let vers = t.path().join("series/Nouvelle Série (2024)/a.mkv");
        let e = execute(&req(&src, &vers, &[], Action::Copy, Method::Auto)).unwrap();
        assert!(vers.exists());
        undo(&e).unwrap();
        assert!(!vers.exists());
        assert!(!t.path().join("series/Nouvelle Série (2024)").exists(), "dossier créé retiré");
        assert!(t.path().join("series").exists(), "dossier déjà existant conservé");
        assert_eq!(fs::read_to_string(&src).unwrap(), "abc", "l'original est intact (lien physique)");
        undo(&e).unwrap(); // idempotent
    }

    #[test]
    fn annulation_refusee_si_fichier_modifie() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/a.mkv");
        mk(&src, "abc");
        let vers = t.path().join("out/a.mkv");
        let e = execute(&req(&src, &vers, &[], Action::Copy, Method::Copy)).unwrap();
        fs::write(&vers, "modifié par quelqu'un d'autre, plus long").unwrap();
        assert!(undo(&e).is_err());
        assert!(vers.exists(), "rien n'est supprimé");
    }

    #[test]
    fn deplacer_puis_annuler_remet_l_original() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("in/Show.S01");
        mk(&dir.join("e1.mkv"), "un");
        mk(&dir.join("e2.mkv"), "deux");
        age_dir(&t.path().join("in"));
        let vers = t.path().join("out/Show.S01");
        let e = execute(&req(&dir, &vers, &[], Action::Move, Method::Auto)).unwrap();
        assert!(!dir.exists(), "l'original est parti après vérification");
        assert_eq!(fs::read_to_string(vers.join("e2.mkv")).unwrap(), "deux");
        undo(&e).unwrap();
        assert_eq!(fs::read_to_string(dir.join("e1.mkv")).unwrap(), "un");
        assert_eq!(fs::read_to_string(dir.join("e2.mkv")).unwrap(), "deux");
        assert!(!vers.exists());
    }

    #[test]
    fn identifiants_uniques_meme_dans_la_meme_seconde() {
        let t = tempfile::tempdir().unwrap();
        let mut ids = std::collections::HashSet::new();
        for n in 0..50 {
            let src = t.path().join(format!("in/f{n}.mkv"));
            mk(&src, "x");
            let e = execute(&req(&src, &t.path().join(format!("out/f{n}.mkv")), &[], Action::Copy, Method::Auto)).unwrap();
            assert!(ids.insert(e.id), "identifiant en double");
        }
        assert_eq!(ids.len(), 50);
        // et l'annulation vise bien le bon rangement
        let h = History::load(&t.path().join("data"));
        let a = execute(&req(&t.path().join("in/f1.mkv"), &t.path().join("out2/a.mkv"), &[], Action::Copy, Method::Copy)).unwrap();
        let b = execute(&req(&t.path().join("in/f2.mkv"), &t.path().join("out2/b.mkv"), &[], Action::Copy, Method::Copy)).unwrap();
        let (ida, idb) = (a.id.clone(), b.id.clone());
        h.add(a);
        h.add(b);
        assert_eq!(h.get(&idb).unwrap().vers, t.path().join("out2/b.mkv"));
        assert_eq!(h.get(&ida).unwrap().vers, t.path().join("out2/a.mkv"));
    }

    #[test]
    fn historique_enregistre_et_recharge() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/a.mkv");
        mk(&src, "abc");
        let e = execute(&req(&src, &t.path().join("out/a.mkv"), &[], Action::Copy, Method::Auto)).unwrap();
        let id = e.id.clone();
        let h = History::load(&t.path().join("data"));
        h.add(e);
        h.set_status(&id, "annule", "test");
        let h2 = History::load(&t.path().join("data"));
        assert_eq!(h2.get(&id).unwrap().statut, "annule");
    }
}
