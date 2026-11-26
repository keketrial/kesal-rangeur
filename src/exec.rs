//! Exécution réelle d'un rangement : copie sûre (reflink → lien physique → copie), vérification,
//! historique et annulation. Ce module n'est appelé que si le « mode essai à blanc » est désactivé.

use crate::config::Category;
use crate::plan::{check_settled, Renommage, Settle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
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
    /// Rangement avec noms/dossiers calculés (nommage activé) : à l'annulation, on ne retire que les dossiers créés.
    #[serde(default)]
    pub plan: bool,
    /// « fait », « annule » ou « erreur »
    pub statut: String,
    pub motif: String,
}

#[derive(Debug)]
pub enum ExecError {
    Existe(PathBuf),
    SourceAbsente(PathBuf),
    PasPret(String),
    /// Panne possiblement passagère (réseau, accès) : on réessaie.
    Io(String),
    /// Panne durable (nom trop long, disque plein, lecture seule…) : inutile de réessayer.
    Fatal(String),
    Verification(String),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Existe(p) => write!(f, "la destination existe déjà : {}", p.display()),
            ExecError::SourceAbsente(p) => write!(f, "l'original a disparu : {}", p.display()),
            ExecError::PasPret(s) => write!(f, "pas prêt : {s}"),
            ExecError::Io(s) | ExecError::Fatal(s) => write!(f, "{s}"),
            ExecError::Verification(s) => write!(f, "vérification échouée : {s}"),
        }
    }
}

fn io_err(ctx: &str, e: io::Error) -> ExecError {
    let (msg, fatal) = match e.raw_os_error() {
        Some(28) => ("plus de place sur le disque de destination".to_string(), true),
        Some(13) | Some(1) => ("accès refusé".to_string(), false),
        Some(30) => ("destination en lecture seule".to_string(), true),
        Some(36) => ("nom de fichier trop long pour la destination".to_string(), true),
        _ => (e.to_string(), false),
    };
    if fatal { ExecError::Fatal(format!("{ctx} : {msg}")) } else { ExecError::Io(format!("{ctx} : {msg}")) }
}

/// Refuse un lien symbolique : suivre un lien hors du dossier d'arrivée permettrait de copier (ou, en « déplacer », de supprimer) autre chose.
fn refuse_symlink(p: &Path) -> Result<(), ExecError> {
    if fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        return Err(ExecError::Fatal(format!("lien symbolique refusé : {}", p.display())));
    }
    Ok(())
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

#[cfg(target_os = "linux")]
const FICLONE: libc::Ioctl = 0x4004_9409 as libc::Ioctl;

/// Reflink (copie instantanée sans place en plus) : Linux sur Btrfs/XFS. Ailleurs : non supporté, on passe à la méthode suivante.
#[cfg(not(target_os = "linux"))]
fn try_reflink(_src: &Path, _dst: &Path) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(95))
}

#[cfg(target_os = "linux")]
fn try_reflink(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
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
pub fn place_file(src: &Path, dst: &Path, pref: Method) -> Result<&'static str, ExecError> {
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
    /// Correspondance exacte fichier d'origine → fichier final (nommage activé). Vide : on garde les noms d'origine.
    pub plan: &'a [(PathBuf, PathBuf)],
}

/// Range un élément. En cas d'échec, rien n'est laissé à moitié fait et l'original n'est jamais touché.
pub fn execute(r: &Request) -> Result<Entry, ExecError> {
    if !r.plan.is_empty() {
        return execute_plan(r);
    }
    if !r.source.exists() {
        return Err(ExecError::SourceAbsente(r.source.to_path_buf()));
    }
    refuse_symlink(r.source)?;
    // Nouvelle vérification juste avant de copier : l'élément ne doit plus bouger.
    match check_settled(r.source, r.settle_minutes) {
        Settle::Pret => {}
        Settle::Transfert { fichier } => return Err(ExecError::PasPret(format!("transfert en cours ({fichier})"))),
        Settle::Attente { minutes } => return Err(ExecError::PasPret(format!("modifié récemment, encore {minutes} min"))),
        Settle::Vide => return Err(ExecError::PasPret("aucune vidéo pour l'instant (fichiers pas encore arrivés)".into())),
    }
    // Une destination qui n'est qu'une coquille vide (dossiers sans aucun fichier, restes d'un ancien rangement raté)
    // ne contient rien à protéger : on la retire pour reprendre sa place.
    if r.vers.exists() && !clear_empty_shell(r.vers) {
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
            for e in WalkDir::new(r.source).min_depth(1) {
                let e = e.map_err(|e| ExecError::Fatal(format!("dossier illisible dans l'élément : {e}")))?;
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
        plan: false,
        statut: "fait".into(),
        motif: String::new(),
    })
}

fn nearest_existing(p: &Path) -> PathBuf {
    let mut cur = p;
    while !cur.exists() {
        match cur.parent() {
            Some(x) => cur = x,
            None => break,
        }
    }
    cur.to_path_buf()
}

/// Rangement avec chemins calculés : chaque fichier est copié dans un dossier provisoire, vérifié, puis mis en place.
/// En cas d'échec à n'importe quel moment, tout ce qui a été placé est retiré.
/// Si `p` est un dossier qui ne contient AUCUN fichier (seulement des dossiers vides, à n'importe quelle profondeur),
/// le supprime (bas en haut, avec `remove_dir` qui refuse tout dossier non vide) et renvoie vrai. Sinon ne touche à rien.
fn clear_empty_shell(p: &Path) -> bool {
    let Ok(md) = fs::symlink_metadata(p) else { return false };
    if !md.is_dir() {
        return false;
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    for e in WalkDir::new(p).follow_links(false) {
        let Ok(e) = e else { return false };
        if !e.file_type().is_dir() {
            return false; // un fichier, un lien… : ce n'est pas vide
        }
        dirs.push(e.path().to_path_buf());
    }
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.iter().all(|d| fs::remove_dir(d).is_ok())
}

fn execute_plan(r: &Request) -> Result<Entry, ExecError> {
    refuse_symlink(r.source)?;
    for (src, _) in r.plan {
        if !src.exists() {
            return Err(ExecError::SourceAbsente(src.clone()));
        }
        refuse_symlink(src)?;
    }
    match check_settled(r.source, r.settle_minutes) {
        Settle::Pret => {}
        Settle::Transfert { fichier } => return Err(ExecError::PasPret(format!("transfert en cours ({fichier})"))),
        Settle::Attente { minutes } => return Err(ExecError::PasPret(format!("modifié récemment, encore {minutes} min"))),
        Settle::Vide => return Err(ExecError::PasPret("aucune vidéo pour l'instant (fichiers pas encore arrivés)".into())),
    }
    let mut seen = std::collections::HashSet::new();
    for (_, dst) in r.plan {
        if dst.exists() {
            return Err(ExecError::Existe(dst.clone()));
        }
        if !seen.insert(dst.clone()) {
            return Err(ExecError::Io(format!("deux fichiers vont au même endroit : {}", dst.display())));
        }
        if dst.starts_with(r.source) && r.source.is_dir() {
            return Err(ExecError::Io("destination et origine se chevauchent".into()));
        }
    }
    let id = unique_id();
    let anchor = nearest_existing(r.vers);
    let anchor = if anchor.is_dir() { anchor } else { anchor.parent().map(|p| p.to_path_buf()).unwrap_or(anchor) };
    let staging_root = anchor.join(format!(".kesal-en-cours-{id}"));
    let mut fichiers: Vec<Fichier> = Vec::new();
    let mut staged: Vec<PathBuf> = Vec::new();
    let mut created: Vec<PathBuf> = Vec::new();
    let mut placed: Vec<PathBuf> = Vec::new();

    let result: Result<(), ExecError> = (|| {
        fs::create_dir_all(&staging_root).map_err(|e| io_err("dossier provisoire", e))?;
        for (i, (src, dst)) in r.plan.iter().enumerate() {
            let tmp = staging_root.join(i.to_string());
            let m = place_file(src, &tmp, r.method)?;
            let taille = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
            fichiers.push(Fichier { src: src.clone(), dst: dst.clone(), taille, methode: m.into() });
            staged.push(tmp);
        }
        for (f, tmp) in fichiers.iter().zip(&staged) {
            let got = fs::metadata(tmp).map(|m| m.len()).map_err(|e| ExecError::Verification(format!("{} : {e}", f.dst.display())))?;
            if got != f.taille {
                return Err(ExecError::Verification(format!("{} : {got} octets au lieu de {}", f.dst.display(), f.taille)));
            }
            if fs::metadata(&f.src).map(|m| m.len()).unwrap_or(0) != f.taille {
                return Err(ExecError::Verification(format!("{} a changé pendant la copie", f.src.display())));
            }
        }
        for (f, tmp) in fichiers.iter().zip(&staged) {
            let parent = f.dst.parent().ok_or_else(|| ExecError::Io("destination sans dossier".into()))?;
            created.extend(first_missing_ancestors(parent));
            fs::create_dir_all(parent).map_err(|e| io_err(&format!("création de {}", parent.display()), e))?;
            if f.dst.exists() {
                return Err(ExecError::Existe(f.dst.clone()));
            }
            fs::rename(tmp, &f.dst).map_err(|e| io_err("mise en place finale", e))?;
            placed.push(f.dst.clone());
        }
        Ok(())
    })();
    if let Err(e) = result {
        for p in &placed {
            let _ = fs::remove_file(p);
        }
        let _ = fs::remove_dir_all(&staging_root); // uniquement notre dossier provisoire
        for d in created.iter().rev() {
            let _ = fs::remove_dir(d); // seulement s'il est vide
        }
        return Err(e);
    }
    let _ = fs::remove_dir(&staging_root);
    if r.action == Action::Move {
        for f in &fichiers {
            let _ = fs::remove_file(&f.src);
        }
        if r.source.is_dir() {
            let mut dirs: Vec<PathBuf> = WalkDir::new(r.source).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_dir()).map(|e| e.into_path()).collect();
            dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
            for d in dirs {
                let _ = fs::remove_dir(&d);
            }
        }
    }
    let total = fichiers.iter().map(|f| f.taille).sum();
    Ok(Entry { id, t: now(), nom: r.nom.to_string(), titre: r.titre.to_string(), categorie: r.categorie, action: r.action, vers: r.vers.to_path_buf(), fichiers, dossiers_crees: created, taille_totale: total, plan: true, statut: "fait".into(), motif: String::new() })
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
    if !e.plan && e.vers.is_dir() {
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
        let entries = match fs::read_to_string(&path) {
            Ok(t) => match serde_json::from_str(&t) {
                Ok(v) => v,
                Err(e) => {
                    // fichier abîmé : on le garde de côté (jamais écrasé) pour ne pas perdre les informations d'annulation
                    let bak = path.with_extension(format!("json.corrompu-{}", now()));
                    let _ = fs::rename(&path, &bak);
                    eprintln!("[erreur] history.json illisible ({e}) : conservé dans {}", bak.display());
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        History { path, entries: Mutex::new(entries) }
    }
    fn save(&self, v: &[Entry]) {
        if let Some(d) = self.path.parent() {
            let _ = fs::create_dir_all(d);
        }
        if let Ok(t) = serde_json::to_string_pretty(v) {
            let tmp = self.path.with_extension("json.tmp");
            if let Err(e) = fs::write(&tmp, t).and_then(|_| fs::rename(&tmp, &self.path)) {
                eprintln!("[erreur] history.json non enregistré : {e}");
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
    /// Il y a combien de secondes la dernière erreur pour cet élément (None s'il n'y en a pas, ou s'il a été rangé depuis).
    pub fn last_error_age(&self, nom: &str) -> Option<u64> {
        let v = self.entries.lock().unwrap();
        let last = v.iter().rev().find(|e| e.nom == nom)?;
        (last.statut == "erreur").then(|| now().saturating_sub(last.t))
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

#[cfg(all(test, unix))]
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
        Request { nom: "x", titre: "X", categorie: Some(Category::Serie), source: src, vers, renommages: ren, action, method, settle_minutes: 30, plan: &[] }
    }

    #[test]
    fn dossier_vide_refuse_et_rien_n_est_deplace() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("arrivee/Serie.S01.1080p");
        fs::create_dir_all(&src).unwrap();
        age_dir(&src);
        let vers = t.path().join("series/Serie (2026)/Serie.S01.1080p");
        let r = execute(&Request { settle_minutes: 0, ..req(&src, &vers, &[], Action::Move, Method::Copy) });
        assert!(matches!(r, Err(ExecError::PasPret(_))), "{r:?}");
        assert!(src.exists(), "le dossier d'origine doit rester");
        assert!(!t.path().join("series").exists(), "rien ne doit être créé");
    }

    #[test]
    fn destination_coquille_vide_est_reprise() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("arrivee/Serie.S01.1080p");
        mk(&src.join("Serie.S01E01.mkv"), "episode");
        age_dir(&src);
        // reste d'un ancien rangement raté : dossiers vides à la destination
        let vers = t.path().join("series/Serie (2026)/Serie.S01.1080p");
        fs::create_dir_all(vers.join("vide")).unwrap();
        let e = execute(&Request { settle_minutes: 0, ..req(&src, &vers, &[], Action::Move, Method::Copy) }).unwrap();
        assert_eq!(fs::read_to_string(vers.join("Serie.S01E01.mkv")).unwrap(), "episode");
        assert!(!e.fichiers.is_empty());
    }

    #[test]
    fn destination_avec_un_fichier_reste_protegee() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("arrivee/Serie.S01.1080p");
        mk(&src.join("Serie.S01E01.mkv"), "nouveau");
        age_dir(&src);
        let vers = t.path().join("series/Serie (2026)/Serie.S01.1080p");
        mk(&vers.join("deja-la.mkv"), "ancien");
        let r = execute(&Request { settle_minutes: 0, ..req(&src, &vers, &[], Action::Move, Method::Copy) });
        assert!(matches!(r, Err(ExecError::Existe(_))), "{r:?}");
        assert_eq!(fs::read_to_string(vers.join("deja-la.mkv")).unwrap(), "ancien");
        assert!(src.join("Serie.S01E01.mkv").exists());
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

    fn plan_req<'a>(src: &'a Path, base: &'a Path, plan: &'a [(PathBuf, PathBuf)], action: Action) -> Request<'a> {
        Request { nom: "x", titre: "X", categorie: Some(Category::Serie), source: src, vers: base, renommages: &[], action, method: Method::Auto, settle_minutes: 30, plan }
    }

    #[test]
    fn plan_dossiers_de_saison_et_annulation_prudente() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/Show.S01.FRENCH-X");
        mk(&src.join("Show.S01E01.mkv"), "ep un");
        mk(&src.join("Show.S01E02.mkv"), "ep deux");
        age_dir(&t.path().join("in"));
        // la série existe déjà, avec un dossier vide à l'utilisateur
        let base = t.path().join("series/Show (2020)");
        fs::create_dir_all(base.join("Saison_00 vide")).unwrap();
        let plan = vec![
            (src.join("Show.S01E01.mkv"), base.join("Season 01/Show - S01E01 - Un.mkv")),
            (src.join("Show.S01E02.mkv"), base.join("Season 01/Show - S01E02 - Deux.mkv")),
        ];
        let e = execute(&plan_req(&src, &base, &plan, Action::Copy)).unwrap();
        assert_eq!(fs::read_to_string(base.join("Season 01/Show - S01E02 - Deux.mkv")).unwrap(), "ep deux");
        assert!(src.join("Show.S01E01.mkv").exists(), "originaux intacts");
        assert!(e.plan);
        assert!(!fs::read_dir(&base).unwrap().any(|x| x.unwrap().file_name().to_string_lossy().starts_with(".kesal")), "pas de reste provisoire");
        undo(&e).unwrap();
        assert!(!base.join("Season 01").exists(), "dossier de saison créé par l'appli : retiré");
        assert!(base.join("Saison_00 vide").exists(), "dossier vide de l'utilisateur : conservé");
        assert!(base.exists(), "dossier de série existant : conservé");
        assert_eq!(fs::read_to_string(src.join("Show.S01E01.mkv")).unwrap(), "ep un");
    }

    #[test]
    fn plan_panne_a_mi_chemin_retire_tout() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/Show.S01");
        mk(&src.join("a.mkv"), "a");
        mk(&src.join("b.mkv"), "b");
        age_dir(&t.path().join("in"));
        let base = t.path().join("series/Show");
        let ro = base.join("Season 02");
        fs::create_dir_all(&ro).unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
        let plan = vec![(src.join("a.mkv"), base.join("Season 01/a.mkv")), (src.join("b.mkv"), ro.join("b.mkv"))];
        let r = execute(&plan_req(&src, &base, &plan, Action::Copy));
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err());
        assert!(!base.join("Season 01").exists(), "le 1er fichier, déjà placé, a été retiré avec son dossier");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 1, "il ne reste que le dossier à l'utilisateur : {:?}", fs::read_dir(&base).unwrap().map(|x| x.unwrap().file_name()).collect::<Vec<_>>());
        assert!(src.join("a.mkv").exists() && src.join("b.mkv").exists());
    }

    #[test]
    fn plan_destination_existante_refusee() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/f.mkv");
        mk(&src, "nouveau");
        let dst = t.path().join("films/F (2020)/F (2020).mkv");
        mk(&dst, "ancien");
        let r = execute(&plan_req(&src, &t.path().join("films/F (2020)"), &[(src.clone(), dst.clone())], Action::Copy));
        assert!(matches!(r, Err(ExecError::Existe(_))));
        assert_eq!(fs::read_to_string(&dst).unwrap(), "ancien");
    }

    #[test]
    fn plan_film_dossier_cree_puis_deplacer_et_annuler() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("in/Film.2020.1080p-X.mkv");
        mk(&src, "film");
        fs::create_dir_all(t.path().join("films")).unwrap();
        let dst = t.path().join("films/Film (2020)/Film (2020).mkv");
        let base = t.path().join("films/Film (2020)");
        let e = execute(&plan_req(&src, &base, &[(src.clone(), dst.clone())], Action::Move)).unwrap();
        assert!(dst.exists() && !src.exists(), "déplacé");
        undo(&e).unwrap();
        assert!(src.exists() && !dst.exists() && !base.exists(), "remis à l'origine, dossier créé retiré");
    }

    #[test]
    fn lien_symbolique_refuse_et_rien_n_est_touche() {
        let t = tempfile::tempdir().unwrap();
        let dehors = t.path().join("dehors/Show.S01");
        mk(&dehors.join("e1.mkv"), "secret");
        fs::create_dir_all(t.path().join("in")).unwrap();
        let lien = t.path().join("in/Show.S01");
        std::os::unix::fs::symlink(&dehors, &lien).unwrap();
        age_dir(&t.path().join("dehors"));
        for action in [Action::Copy, Action::Move] {
            let r = execute(&req(&lien, &t.path().join("out/Show.S01"), &[], action, Method::Auto));
            assert!(matches!(r, Err(ExecError::Fatal(_))), "{r:?}");
        }
        assert_eq!(fs::read_to_string(dehors.join("e1.mkv")).unwrap(), "secret", "le dossier pointé n'est pas touché");
        assert!(!t.path().join("out").exists());
        // un fichier-lien
        let f = t.path().join("in/Film.mkv");
        mk(&t.path().join("dehors/x.txt"), "mot de passe");
        std::os::unix::fs::symlink(t.path().join("dehors/x.txt"), &f).unwrap();
        let r = execute(&plan_req(&f, &t.path().join("films"), &[(f.clone(), t.path().join("films/Film.mkv"))], Action::Copy));
        assert!(matches!(r, Err(ExecError::Fatal(_))));
    }

    #[test]
    fn historique_corrompu_est_conserve() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("data")).unwrap();
        fs::write(t.path().join("data/history.json"), "[{\"id\": \"a\", tronqué").unwrap();
        let h = History::load(&t.path().join("data"));
        assert!(h.all().is_empty());
        let sauvegardes: Vec<_> = fs::read_dir(t.path().join("data")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).filter(|n| n.contains("corrompu")).collect();
        assert_eq!(sauvegardes.len(), 1, "le fichier abîmé est gardé de côté");
    }

    #[test]
    fn nettoyage_des_dossiers_provisoires_abandonnes() {
        let t = tempfile::tempdir().unwrap();
        let vieux = t.path().join("series/Show/.kesal-en-cours-abc");
        mk(&vieux.join("0"), "reste");
        let neuf = t.path().join("series/.kesal-en-cours-xyz");
        fs::create_dir_all(&neuf).unwrap();
        let autre = t.path().join("series/Show/.kesal-autre");
        fs::create_dir_all(&autre).unwrap();
        // le plus vieux est âgé de 2 h
        let f = fs::File::open(&vieux).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(7200)).unwrap();
        let n = sweep_staging(&[t.path().join("series")], 1800);
        assert_eq!(n, 1);
        assert!(!vieux.exists() && neuf.exists() && autre.exists(), "seul le vieux dossier provisoire est retiré");
    }

    #[test]
    fn erreurs_durables_ne_sont_pas_reessayees() {
        assert!(matches!(io_err("x", io::Error::from_raw_os_error(36)), ExecError::Fatal(_)));
        assert!(matches!(io_err("x", io::Error::from_raw_os_error(28)), ExecError::Fatal(_)));
        assert!(matches!(io_err("x", io::Error::from_raw_os_error(30)), ExecError::Fatal(_)));
        assert!(matches!(io_err("x", io::Error::from_raw_os_error(5)), ExecError::Io(_)));
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

/// Vérifie quelles méthodes de copie marchent entre deux dossiers (ne laisse rien derrière).
pub fn selftest(from_dir: &Path, to_dir: &Path) -> Vec<(String, Result<String, String>)> {
    let tag = unique_id();
    let a = from_dir.join(format!(".kesal-selftest-{tag}"));
    let b = to_dir.join(format!(".kesal-selftest-{tag}"));
    let mut out = Vec::new();
    let setup = fs::create_dir_all(&a).and_then(|_| fs::create_dir_all(&b)).and_then(|_| fs::write(a.join("source.bin"), vec![7u8; 1 << 20]));
    if let Err(e) = setup {
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
        return vec![("préparation".into(), Err(format!("impossible d'écrire dans les dossiers : {e}")))];
    }
    for (name, m) in [("reflink (instantané, sans place en plus)", Method::Reflink), ("lien physique (sans place en plus)", Method::Hardlink), ("copie normale", Method::Copy)] {
        let dst = b.join(format!("{m:?}.bin"));
        let r = place_file(&a.join("source.bin"), &dst, m).map(|x| x.to_string()).map_err(|e| e.to_string());
        let size_ok = fs::metadata(&dst).map(|x| x.len() == 1 << 20).unwrap_or(false);
        out.push((name.to_string(), match r { Ok(x) if size_ok => Ok(x), Ok(_) => Err("fichier incomplet".into()), Err(e) => Err(e) }));
    }
    let _ = fs::remove_dir_all(&a); // uniquement les dossiers de test créés ci-dessus
    let _ = fs::remove_dir_all(&b);
    out
}

/// Supprime les dossiers provisoires `.kesal-en-cours-*` abandonnés (arrêt brutal pendant une copie).
/// Ne touche qu'à des dossiers portant ce nom, plus vieux que `min_age_secs`, jusqu'à 2 niveaux sous chaque racine.
pub fn sweep_staging(roots: &[PathBuf], min_age_secs: u64) -> usize {
    let mut removed = 0;
    for root in roots {
        for e in WalkDir::new(root).min_depth(1).max_depth(2).into_iter().filter_map(|e| e.ok()) {
            if !e.file_type().is_dir() || !e.file_name().to_string_lossy().starts_with(".kesal-en-cours-") {
                continue;
            }
            let age = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| SystemTime::now().duration_since(t).ok()).map(|d| d.as_secs()).unwrap_or(0);
            if age >= min_age_secs && fs::remove_dir_all(e.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}
