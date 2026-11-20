mod library;
mod norm;
mod parser;

use std::path::Path;

fn main() -> anyhow::Result<()> {
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
        _ => eprintln!("usage : kesal-rangeur parse|index <dossier>"),
    }
    Ok(())
}
