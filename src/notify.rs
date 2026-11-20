//! Prévenir Plex, Jellyfin et Home Assistant après un rangement (jamais en mode essai à blanc).

use crate::config::{Category, Config, Secrets};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap()
}

fn base(u: &str) -> String {
    u.trim().trim_end_matches('/').to_string()
}

fn plain(e: reqwest::Error) -> String {
    e.without_url().to_string()
}

/// Sections Plex : (numéro, titre, dossiers).
async fn plex_sections(cfg: &Config, sec: &Secrets) -> Result<Vec<(String, String, Vec<String>)>, String> {
    let url = format!("{}/library/sections", base(&cfg.integrations.plex_url));
    let r = client().get(&url).header("Accept", "application/json").header("X-Plex-Token", &sec.plex).send().await.map_err(plain)?;
    if !r.status().is_success() {
        return Err(format!("Plex a répondu {}", r.status()));
    }
    let v: Value = r.json().await.map_err(plain)?;
    Ok(v["MediaContainer"]["Directory"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            (
                d["key"].as_str().unwrap_or("").to_string(),
                d["title"].as_str().unwrap_or("").to_string(),
                d["Location"].as_array().into_iter().flatten().filter_map(|l| l["path"].as_str()).map(String::from).collect(),
            )
        })
        .collect())
}

fn last_component(p: &str) -> String {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_lowercase()
}

/// Rafraîchit ce qui doit l'être pour la catégorie. Renvoie des phrases pour le journal.
pub async fn refresh(cfg: &Config, sec: &Secrets, cat: Category, dest_root: &Path) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    if !cfg.integrations.plex_url.is_empty() && !sec.plex.is_empty() {
        match plex_refresh(cfg, sec, cat, dest_root).await {
            Ok(m) => out.push((true, m)),
            Err(e) => out.push((false, format!("Plex : {e}"))),
        }
    }
    if !cfg.integrations.jellyfin_url.is_empty() && !sec.jellyfin.is_empty() {
        let url = format!("{}/Library/Refresh", base(&cfg.integrations.jellyfin_url));
        match client().post(&url).header("X-Emby-Token", &sec.jellyfin).send().await {
            Ok(r) if r.status().is_success() => out.push((true, "Jellyfin : rafraîchissement demandé".into())),
            Ok(r) => out.push((false, format!("Jellyfin a répondu {}", r.status()))),
            Err(e) => out.push((false, format!("Jellyfin : {}", plain(e)))),
        }
    }
    out
}

async fn plex_refresh(cfg: &Config, sec: &Secrets, cat: Category, dest_root: &Path) -> Result<String, String> {
    let sections = plex_sections(cfg, sec).await?;
    let key = if let Some(k) = cfg.integrations.plex_sections.get(&cat) {
        k.clone()
    } else {
        let want = last_component(&dest_root.to_string_lossy());
        let found: Vec<_> = sections.iter().filter(|(_, _, locs)| locs.iter().any(|l| last_component(l) == want)).collect();
        match found.as_slice() {
            [one] => one.0.clone(),
            [] => return Err(format!("aucune bibliothèque Plex ne pointe vers un dossier « {want} »")),
            _ => return Err(format!("plusieurs bibliothèques Plex pour « {want} » : précisez-la dans plex_sections")),
        }
    };
    let title = sections.iter().find(|s| s.0 == key).map(|s| s.1.clone()).unwrap_or_default();
    let url = format!("{}/library/sections/{key}/refresh", base(&cfg.integrations.plex_url));
    let r = client().get(&url).header("X-Plex-Token", &sec.plex).send().await.map_err(plain)?;
    if r.status().is_success() {
        Ok(format!("Plex : bibliothèque « {title} » en cours de rafraîchissement"))
    } else {
        Err(format!("Plex a répondu {}", r.status()))
    }
}

pub async fn home_assistant(cfg: &Config, sec: &Secrets, titre: &str, message: &str) -> Result<(), String> {
    if cfg.integrations.ha_url.is_empty() || sec.ha.is_empty() || cfg.integrations.ha_service.is_empty() {
        return Ok(());
    }
    let (domain, service) = cfg.integrations.ha_service.split_once('.').ok_or("service de notification invalide (attendu : notify.xxx)")?;
    let url = format!("{}/api/services/{domain}/{service}", base(&cfg.integrations.ha_url));
    let r = client().post(&url).bearer_auth(&sec.ha).json(&serde_json::json!({"title": titre, "message": message})).send().await.map_err(plain)?;
    if r.status().is_success() { Ok(()) } else { Err(format!("Home Assistant a répondu {}", r.status())) }
}

/// Test de connexion en lecture seule (rien n'est modifié chez les services).
pub async fn test_all(cfg: &Config, sec: &Secrets) -> Value {
    let mut res = serde_json::Map::new();
    let c = client();
    // Plex
    let plex = if cfg.integrations.plex_url.is_empty() || sec.plex.is_empty() {
        serde_json::json!({"ok": null, "message": "non configuré"})
    } else {
        match plex_sections(cfg, sec).await {
            Ok(s) => {
                let det: Vec<String> = s.iter().map(|x| format!("{} → {}", x.1, x.2.join(", "))).collect();
                serde_json::json!({"ok": true, "message": format!("{} bibliothèque(s) : {}", s.len(), det.join(" ; "))})
            }
            Err(e) => serde_json::json!({"ok": false, "message": e}),
        }
    };
    res.insert("plex".into(), plex);
    // Jellyfin
    let jf = if cfg.integrations.jellyfin_url.is_empty() || sec.jellyfin.is_empty() {
        serde_json::json!({"ok": null, "message": "non configuré"})
    } else {
        match c.get(format!("{}/System/Info", base(&cfg.integrations.jellyfin_url))).header("X-Emby-Token", &sec.jellyfin).send().await {
            Ok(r) if r.status().is_success() => {
                let v: Value = r.json().await.unwrap_or_default();
                serde_json::json!({"ok": true, "message": format!("Jellyfin {} « {} »", v["Version"].as_str().unwrap_or("?"), v["ServerName"].as_str().unwrap_or("?"))})
            }
            Ok(r) => serde_json::json!({"ok": false, "message": format!("Jellyfin a répondu {}", r.status())}),
            Err(e) => serde_json::json!({"ok": false, "message": plain(e)}),
        }
    };
    res.insert("jellyfin".into(), jf);
    // Home Assistant
    let ha = if cfg.integrations.ha_url.is_empty() || sec.ha.is_empty() {
        serde_json::json!({"ok": null, "message": "non configuré"})
    } else {
        match c.get(format!("{}/api/", base(&cfg.integrations.ha_url))).bearer_auth(&sec.ha).send().await {
            Ok(r) if r.status().is_success() => serde_json::json!({"ok": true, "message": "Home Assistant répond"}),
            Ok(r) => serde_json::json!({"ok": false, "message": format!("Home Assistant a répondu {}", r.status())}),
            Err(e) => serde_json::json!({"ok": false, "message": plain(e)}),
        }
    };
    res.insert("ha".into(), ha);
    Value::Object(res)
}
