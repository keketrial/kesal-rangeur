mod config;
mod exec;
mod library;
mod notify;
mod norm;
mod parser;
mod plan;
mod tmdb;
mod web;

use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("parse") => {
            // kesal-rangeur parse <dossier>  : analyse le nom de chaque entrée du dossier
            let dir = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            let mut names: Vec<String> = std::fs::read_dir(Path::new(dir))?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            names.sort();
            for n in names {
                let r = parser::parse(&n);
                println!(
                    "{:<60} | {:<28} | {:<4} | S{:?} E{:?} {} | {} {}",
                    n.chars().take(60).collect::<String>(),
                    r.title,
                    r.year.map(|y| y.to_string()).unwrap_or_default(),
                    r.seasons,
                    r.episodes,
                    if r.complete { "COMPLET" } else { "" },
                    r.resolution.clone().unwrap_or_default(),
                    r.group.clone().unwrap_or_default()
                );
            }
        }
        Some("index") => {
            // kesal-rangeur index <dossier de séries> : ce que l'appli comprend de la bibliothèque existante
            let dir = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            let t = std::time::Instant::now();
            let lib = library::scan_series(Path::new(dir))?;
            for e in &lib {
                let seasons: Vec<String> = e.seasons.iter().map(|s| s.to_string()).collect();
                println!(
                    "{:<55} | {:<30} | {:<4} | saisons [{}] {} | {} vidéos",
                    e.name.chars().take(55).collect::<String>(),
                    e.title,
                    e.year.map(|y| y.to_string()).unwrap_or_default(),
                    seasons.join(","),
                    if e.complete { "intégrale" } else { "" },
                    e.videos
                );
            }
            eprintln!("{} dossiers en {:.1}s", lib.len(), t.elapsed().as_secs_f32());
        }
        Some("serve") => {
            // kesal-rangeur serve [config.toml] : interface web (port 3050 par défaut, KESAL_BIND pour changer)
            let cfg_path = std::path::PathBuf::from(args.get(2).cloned().or_else(|| std::env::var("KESAL_CONFIG").ok()).unwrap_or("data/config.toml".into()));
            let bind = std::env::var("KESAL_BIND").unwrap_or("0.0.0.0:3050".into());
            web::serve(cfg_path, bind).await?;
        }
        Some("selftest") => {
            // kesal-rangeur selftest <dossier d'arrivée> [dossier de destination] : quelles copies marchent entre les deux ?
            let a = Path::new(args.get(2).map(|s| s.as_str()).unwrap_or("."));
            let b = Path::new(args.get(3).map(|s| s.as_str()).unwrap_or(args.get(2).map(|s| s.as_str()).unwrap_or(".")));
            println!("De  : {}\nVers: {}\n", a.display(), b.display());
            let res = exec::selftest(a, b);
            for (name, r) in &res {
                match r {
                    Ok(_) => println!("  ✓ {name}"),
                    Err(e) => println!("  ✗ {name} — {e}"),
                }
            }
            let fast = res.iter().any(|(n, r)| r.is_ok() && !n.starts_with("copie"));
            println!("\n{}", if fast { "Les copies instantanées sont possibles : le mode automatique les utilisera." } else { "Aucune copie instantanée possible entre ces deux dossiers : chaque rangement sera une vraie copie (lente, et elle double l'espace utilisé). Vérifiez que les deux sont sur le même volume Btrfs et dans un seul montage Docker." });
        }
        Some("matrix") => matrix(args.get(2).map(|s| s.as_str()).unwrap_or("data/config.toml")).await?,
        Some("plan") => {
            // kesal-rangeur plan <config.toml> : essai à blanc, ne touche à rien
            let cfg = std::sync::Arc::new(config::Config::load(Path::new(args.get(2).map(|s| s.as_str()).unwrap_or("data/config.toml")))?);
            let tmdb = std::sync::Arc::new(tmdb::Tmdb::new(cfg.tmdb_secret()?, cfg.language.clone(), &cfg.data_dir)?);
            let planner = plan::Planner::new(cfg.clone(), tmdb.clone());
            for input in &cfg.inputs {
                println!("=== {} (mode {}, {}) ===", input.path.display(), input.mode, input.action);
                let items = planner.plan_input(input).await?;
                for it in &items {
                    let rec = it.reconnu.as_ref().map(|r| format!("{} ({}) → {}", r.titre, r.annee.map(|y| y.to_string()).unwrap_or("?".into()), r.categorie.label())).unwrap_or_default();
                    println!("• {}", it.nom);
                    if !rec.is_empty() {
                        println!("    reconnu : {rec}");
                    }
                    match &it.decision {
                        plan::Decision::Ranger { vers, nouveau_dossier, renumerotation, renommages } => {
                            println!("    RANGER{} → {}", if *nouveau_dossier { " (nouveau dossier)" } else { "" }, vers.display());
                            if let Some((a, b)) = renumerotation {
                                println!("    RENUMÉROTATION S{a:02} → S{b:02} (la fiche TMDB n'a que la saison {b})");
                                for r in renommages { println!("       {}  →  {}", r.ancien, r.nouveau); }
                            }
                        }
                        plan::Decision::Doublon { existant, raison } => println!("    DOUBLON : {raison} → {}", existant.display()),
                        plan::Decision::ATrier { raison, propositions } => {
                            println!("    À TRIER : {raison}");
                            for p in propositions { println!("       ? {p}"); }
                        }
                        plan::Decision::Attendre { raison } => println!("    ATTENDRE : {raison}"),
                        plan::Decision::Ignorer { raison } => println!("    ignoré : {raison}"),
                        plan::Decision::Fait { vers } => println!("    FAIT → {}", vers.display()),
                    }
                }
            }
            tmdb.save_cache();
            eprintln!("{} requêtes TMDB", tmdb.requests.lock().unwrap());
        }
        _ => eprintln!("usage : kesal-rangeur parse|index <dossier> | plan <config.toml> | serve [config.toml]"),
    }
    Ok(())
}

/// Essai à blanc de nombreuses configurations sur les vraies données (lecture seule : rien n'est écrit).
async fn matrix(cfg_path: &str) -> anyhow::Result<()> {
    use config::{Category, Rule};
    use std::sync::Arc;
    let base = config::Config::load(Path::new(cfg_path))?;
    let secret = base.tmdb_secret()?;
    let mk_tmdb = |lang: &str| tmdb::Tmdb::new(secret.clone(), lang.to_string(), &base.data_dir).map(Arc::new);
    let tmdb0 = mk_tmdb(&base.language)?;
    let t = std::time::Instant::now();
    let base_arc = Arc::new(base.clone());
    let p0 = plan::Planner::new(base_arc.clone(), tmdb0.clone());
    eprintln!("bibliothèques lues en {:.0}s", t.elapsed().as_secs_f32());

    type Mut = Box<dyn Fn(&mut config::Config)>;
    let mut cases: Vec<(String, Mut, &str, bool)> = Vec::new(); // (nom, modif, langue, relire films)
    let l0 = base.language.clone();
    cases.push(("0 référence (auto, copier, calme 30 min)".into(), Box::new(|_| {}), "", false));
    cases.push(("1 calme = 0 min (ne jamais attendre)".into(), Box::new(|c| c.inputs.iter_mut().for_each(|i| i.settle_minutes = 0)), "", false));
    cases.push(("2 calme = 10 ans (tout doit attendre)".into(), Box::new(|c| c.inputs.iter_mut().for_each(|i| i.settle_minutes = 5_000_000)), "", false));
    cases.push(("3 action = déplacer".into(), Box::new(|c| c.inputs.iter_mut().for_each(|i| i.action = "move".into())), "", false));
    for (cat, name) in [("film", "film"), ("serie", "série"), ("dessin_anime", "dessin animé"), ("telerealite", "télé/téléréalité"), ("documentaire", "documentaire"), ("spectacle", "spectacle")] {
        let cat = cat.to_string();
        cases.push((format!("4 type imposé : tout → {name}"), Box::new(move |c| c.inputs.iter_mut().for_each(|i| i.mode = cat.clone())), "", false));
    }
    cases.push(("5 règle : « Friends Sex and Games » → téléréalité, « Allen » → spectacle".into(), Box::new(|c| {
        c.rules.push(Rule { title: "Friends Sex and Games".into(), category: Category::Telerealite });
        c.rules.push(Rule { title: "Allen".into(), category: Category::Spectacle });
    }), "", false));
    cases.push(("6 destination « série » non renseignée".into(), Box::new(|c| { c.destinations.remove(&Category::Serie); }), "", false));
    cases.push(("7 destination « film » non renseignée".into(), Box::new(|c| { c.destinations.remove(&Category::Film); }), "", true));
    cases.push(("8 aucun dossier de films supplémentaire (doublons : film_recent seulement)".into(), Box::new(|c| c.film_libraries.clear()), "", true));
    cases.push(("9 langue espagnol (es-ES)".into(), Box::new(|c| c.language = "es-ES".into()), "es-ES", false));
    cases.push(("10 langue anglais (en-US)".into(), Box::new(|c| c.language = "en-US".into()), "en-US", false));
    cases.push(("11 dossier d'arrivée inexistant + dossier valide".into(), Box::new(|c| {
        let mut bad = c.inputs[0].clone();
        bad.path = "/mnt/nas/telechargement/n_existe_pas".into();
        c.inputs.insert(0, bad);
    }), "", false));
    let _ = l0;

    // Résultat de référence
    let mut reference: std::collections::BTreeMap<String, String> = Default::default();
    println!("{:<86} {:>6} {:>7} {:>7} {:>7} {:>6}", "CONFIGURATION", "ranger", "doublon", "à trier", "attente", "ignoré");
    for (label, modify, lang, rescan) in cases {
        let mut c = base.clone();
        modify(&mut c);
        let tm = if lang.is_empty() { tmdb0.clone() } else { mk_tmdb(lang)? };
        let planner = p0.variant(Arc::new(c.clone()), tm, rescan);
        let mut items = Vec::new();
        let mut errors = Vec::new();
        for input in &c.inputs {
            match planner.plan_input(input).await {
                Ok(mut v) => items.append(&mut v),
                Err(e) => errors.push(format!("{} : {e}", input.path.display())),
            }
        }
        let (mut r, mut d, mut a, mut w, mut ig) = (0, 0, 0, 0, 0);
        let mut now_map = std::collections::BTreeMap::new();
        for it in &items {
            let desc = match &it.decision {
                plan::Decision::Ranger { vers, .. } => { r += 1; format!("RANGER → {}", vers.display().to_string().replace("/mnt/nas/", "")) }
                plan::Decision::Doublon { raison, .. } => { d += 1; format!("DOUBLON ({raison})") }
                plan::Decision::ATrier { raison, .. } => { a += 1; format!("À TRIER ({raison})") }
                plan::Decision::Attendre { .. } => { w += 1; "ATTENDRE".to_string() }
                plan::Decision::Ignorer { .. } => { ig += 1; "ignoré".to_string() }
                plan::Decision::Fait { .. } => String::new(),
            };
            now_map.insert(it.nom.chars().take(34).collect::<String>(), desc);
        }
        println!("{:<86} {:>6} {:>7} {:>7} {:>7} {:>6}", label.chars().take(86).collect::<String>(), r, d, a, w, ig);
        for e in &errors { println!("      (dossier illisible géré proprement : {e})"); }
        if reference.is_empty() {
            reference = now_map;
        } else {
            let mut shown = 0;
            for (k, v) in &now_map {
                if reference.get(k) != Some(v) && shown < 18 {
                    println!("      {k:<34} → {v}");
                    shown += 1;
                }
            }
        }
    }
    tmdb0.save_cache();
    eprintln!("{} requêtes TMDB en tout", tmdb0.requests.lock().unwrap());
    Ok(())
}
