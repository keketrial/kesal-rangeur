# kesal-rangeur

🇫🇷 Français · [🇬🇧 English](README.en.md) · [🇪🇸 Español](README.es.md)

**Range automatiquement vos téléchargements** (films, séries, dessins animés, émissions, documentaires, spectacles) au bon endroit de votre bibliothèque, avec **les titres dans votre langue**.

Pourquoi ? Sonarr et Radarr s'appuient sur TheTVDB, qui donne souvent des titres anglais (« GIGN » devient « Elite Force », « Tapie » devient « Class Act »). `kesal-rangeur` utilise **TMDB**, qui a les titres en français, espagnol, etc.

## Ce que ça fait

- Surveille un ou plusieurs **dossiers d'arrivée** (Resilio Sync, client torrent…) et attend que les transferts soient finis.
- Reconnaît chaque film ou série avec **TMDB**, choisit le bon dossier (série existante, nouvelle saison, catégorie) et détecte les **doublons**.
- **Trois modes**, au choix en haut de l'interface : Simulation (rien n'est écrit), Manuel (vous validez chaque élément) ou Auto (range seul ce qui est sûr).
- **Copier** ou **Déplacer** selon le dossier d'arrivée. Copie instantanée sur Btrfs/XFS (reflink).
- En cas de doute, l'élément va dans **À trier** : vous choisissez, l'appli s'en souvient.
- Noms d'origine gardés par défaut, ou noms au choix (année, résolution, codec H.265/AV1, audio…).
- **Historique avec bouton Annuler** pour chaque rangement.
- Prévient **Plex**, **Jellyfin** et Home Assistant après un rangement.

Au premier lancement, l'appli est en **Simulation** : elle montre tout ce qu'elle ferait (fichiers, volume, renommages, doublons) sans rien écrire.

## Installation

### Docker (recommandé, par exemple sur un NAS)

```yaml
services:
  rangeur:
    image: ghcr.io/VOTRE-COMPTE/kesal-rangeur:latest
    ports: ["3050:3050"]
    user: "1026:100"                 # PUID:PGID du propriétaire de vos médias
    volumes:
      - ./config:/config
      - /volume1/MEDIAS:/medias      # un seul montage pour l'arrivée et les destinations
```

Gardez **un seul montage** pour l'arrivée et les destinations : c'est ce qui rend les copies et les déplacements instantanés. Ouvrez ensuite `http://adresse-du-serveur:3050`.

### Binaires

Windows, Linux et macOS : voir les *Releases*. Lancez `kesal-rangeur serve`.

### Compiler

```sh
cargo build --release
```

## Réglages

Tout se règle dans l'interface (bouton `?` pour l'aide). Il faut une **clé TMDB** gratuite, à créer sur [themoviedb.org](https://www.themoviedb.org/settings/api) : chacun utilise la sienne.

Pour protéger l'interface, définissez `KESAL_PASSWORD`. Pour y accéder par un nom de domaine, ajoutez-le à `KESAL_ALLOWED_HOSTS`. Gardez-la sur votre réseau local ou derrière un VPN.

## Assistant IA (facultatif)

`kesal-rangeur mcp` permet à un assistant (Claude Code, Claude Desktop…) de piloter une instance déjà lancée. Il laisse la simulation active tant que vous n'avez pas accepté les vraies copies. Détails : [docs/mcp.md](docs/mcp.md).

## Licence

GPL v3, voir [LICENSE](LICENSE). Appli gratuite (l'API TMDB l'est pour un usage non commercial).

## TMDB

> This product uses the TMDB API but is not endorsed or certified by TMDB.
> Ce produit utilise l'API TMDB mais n'est ni approuvé ni certifié par TMDB.

---

<img src="web/tmdb-logo.svg" alt="TMDB" width="200">

This application uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by TMDB.
