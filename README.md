# kesal-rangeur

🇫🇷 Français · [🇬🇧 English](README.en.md) · [🇪🇸 Español](README.es.md)

**Range automatiquement vos téléchargements** (films, séries, dessins animés, émissions, documentaires, spectacles) au bon endroit de votre bibliothèque, avec **les titres dans votre langue**.

Pourquoi ? Sonarr et Radarr s'appuient sur TheTVDB, qui donne souvent des titres anglais (« GIGN » devient « Elite Force », « Tapie » devient « Class Act »). `kesal-rangeur` utilise **TMDB**, qui a les titres en français, espagnol, etc.

## Ce que ça fait

- **Simulation complète** : un onglet montre tout ce qui se passerait (fichiers à transférer, volume, nouveaux dossiers, renommages, doublons, à trier), rapport téléchargeable. Ensuite seulement, choix du mode : simulation / manuel / automatique.
- Surveille un ou plusieurs **dossiers d'arrivée** (Resilio Sync, client torrent…).
- **Attend la fin du transfert** : aucun fichier temporaire (`.!sync`, `.part`…) et rien de modifié depuis 30 min (réglable). Vérifié une seconde fois juste avant de copier.
- **Reconnaît** film ou série, saison, pack complet, qualité, langue (MULTi, VFF, VOSTFR…) à partir du nom de la release.
- **Identifie avec TMDB** dans la langue choisie et déduit la catégorie (genres Animation, Documentaire, Reality, Talk…). Règles personnelles possibles.
- **Retrouve le dossier de série existant** (même sous un nom différent) et y ajoute la saison ; sinon crée « Titre (année) ». Les remakes de même titre sont distingués par l'année.
- Gère les séries qui ont **une fiche TMDB par saison** (ex. « Meurtres à Nordholm 1, 2, 3, 4 ») et renumérote les épisodes pour que Plex et Jellyfin les retrouvent.
- **Nommage à la FileBot (facultatif)** : mêmes variables (`{n} {y} {s00e00} {t}`…), préréglages FileBot et Plex, titres d'épisodes TMDB, aperçu avant → après. Désactivé par défaut (noms d'origine conservés).
- **Détecte les doublons** (film : titre + année ; épisode : série + SxxEyy).
- **Copie sans doubler l'espace** : reflink (Btrfs/XFS), sinon lien physique, sinon copie. L'original reste en place (idéal avec Resilio, où supprimer = supprimer sur la seedbox).
- **En cas de doute → « À trier »** : fiches TMDB avec affiche, vous choisissez, l'appli retient.
- **Historique avec bouton Annuler**, réessais automatiques, rien n'est jamais laissé à moitié fait.
- Prévient **Plex** et **Jellyfin** (et Home Assistant) après un rangement.
- Interface web utilisable sur téléphone, avec une **aide intégrée** (bouton `?`).

## Sécurité : le mode « essai à blanc »

Au premier lancement, l'appli est en **essai à blanc** : elle analyse et propose, mais **n'écrit rien**. Vous vérifiez le résultat (onglets *Simulation* et *Détail*), puis vous désactivez l'essai dans les Réglages quand vous êtes prêt. Chaque rangement réel reste annulable.

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
      - /volume1/MEDIAS:/medias      # UN SEUL montage pour l'arrivée et les destinations
```

> **Un seul montage pour tout.** Pour des copies instantanées, l'arrivée et les destinations doivent être sur le même volume Btrfs/XFS *et* dans un même montage Docker. Vérifiez avec :
> `docker exec kesal-rangeur kesal-rangeur selftest /medias/arrivee /medias/films`

Ouvrez ensuite `http://adresse-du-serveur:3050` et suivez l'aide (`?`).

### Binaires

Windows (`.exe`), Linux et macOS (x86 et ARM) : voir les *Releases*. Lancez `kesal-rangeur serve`. Les copies par reflink ne marchent que sous Linux ; ailleurs, lien physique ou copie.

### Compiler

```sh
cargo build --release     # Rust récent ; l'interface est intégrée au binaire
```

## Réglages

Tout se fait dans l'interface. Les fichiers sont dans le dossier de configuration : `config.toml` (réglages), `secrets.json` (clés, droits 600), `choices.json` (vos choix), `history.json`, `tmdb-cache.json`.

**Clé TMDB** : gratuite sur [themoviedb.org](https://www.themoviedb.org/settings/api). Chaque utilisateur saisit la sienne ; aucune n'est fournie. Le guide pas à pas est dans l'aide de l'appli.

## Assistant (MCP)

Un assistant IA (Claude Code, Claude Desktop…) peut vous aider à configurer et utiliser l'appli : `kesal-rangeur mcp` est un serveur [MCP](https://modelcontextprotocol.io) (stdio) qui pilote une instance **déjà lancée** via son API HTTP. Il garde l'essai à blanc actif tant que vous n'avez pas explicitement accepté les vraies copies.

```sh
claude mcp add kesal-rangeur -- kesal-rangeur mcp
# avec Docker :
claude mcp add kesal-rangeur -- docker exec -i kesal-rangeur kesal-rangeur mcp
# application ailleurs que sur http://127.0.0.1:3050 :
claude mcp add kesal-rangeur -e KESAL_URL=http://nas:3050 -- kesal-rangeur mcp
```

Demandez ensuite à l'assistant d'utiliser le prompt `setup_kesal_rangeur` (première configuration guidée). Détails : [docs/mcp.md](docs/mcp.md).

## Sécurité réseau

- **Mot de passe (conseillé)** : définissez la variable `KESAL_PASSWORD` ; toute l'interface demande alors ce mot de passe (n'importe quel identifiant).
- **Noms d'hôte** : l'accès par adresse IP, `localhost`, un nom court (service Docker) ou `.local` est accepté. Pour un nom de domaine (tunnel, reverse proxy), ajoutez-le à `KESAL_ALLOWED_HOSTS` (liste séparée par des virgules) : cela protège du « DNS rebinding ».
- Les requêtes qui modifient quoi que ce soit sont refusées si elles viennent d'un autre site web (contrôle de l'en-tête `Origin`).
- Un jeton (Plex, Jellyfin, Home Assistant) est **lié à l'adresse saisie avec lui** : si l'adresse change sans nouveau jeton, l'ancien est effacé.
- Gardez malgré tout l'interface sur votre réseau local ou derrière un VPN ; ne l'exposez pas directement sur Internet.

## À venir

Recherche sur les trackers (Prowlarr / Jackett, Torznab), choix de release selon vos préférences, envoi au client torrent, veille des nouveaux épisodes.

## Licence

GPL v3 — voir [LICENSE](LICENSE). Cette appli doit rester **gratuite** (l'API TMDB est gratuite pour un usage non commercial).

## TMDB

> This product uses the TMDB API but is not endorsed or certified by TMDB.
> Ce produit utilise l'API TMDB mais n'est ni approuvé ni certifié par TMDB.

---

<img src="web/tmdb-logo.svg" alt="TMDB" width="200">

This application uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by TMDB.
