mod config;
mod library;
mod norm;
mod parser;
mod plan;
mod tmdb;

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
        Some("plan") => {
            // kesal-rangeur plan <config.toml> : essai à blanc, ne touche à rien
            let cfg = config::Config::load(Path::new(args.get(2).map(|s| s.as_str()).unwrap_or("data/config.toml")))?;
            let tmdb = tmdb::Tmdb::new(cfg.tmdb_secret()?, cfg.language.clone(), &cfg.data_dir)?;
            let planner = plan::Planner::new(&cfg, &tmdb);
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
                    }
                }
            }
            tmdb.save_cache();
            eprintln!("{} requêtes TMDB", tmdb.requests.lock().unwrap());
        }
        _ => eprintln!("usage : kesal-rangeur parse|index <dossier> | plan <config.toml>"),
    }
    Ok(())
}
