# Notes de version / Changelog

## 0.1.4 — 26/11/2026

**Nouveauté**
- La **version installée** est maintenant affichée en haut de l'interface, à côté du nom (un clic ouvre la page des versions sur GitHub), pour savoir en un coup d'œil quelle version tourne.

## 0.1.3 — 26/11/2026

**Nouveauté : une saison ou une série entière peut arriver par-dessus des épisodes déjà rangés**
- Quand une saison (ou une intégrale) arrive alors que la série est déjà en partie dans la bibliothèque, l'appli compare **épisode par épisode** : ceux qui sont déjà là ne sont pas recopiés, **seuls les épisodes manquants sont rangés**, dans le dossier de leur saison s'il existe.
- Si tout est déjà là : « tous les épisodes existent déjà » (doublon). Les fichiers en double restent dans le dossier d'arrivée, rien n'est jamais supprimé ni écrasé.
- Une saison reçue dont les épisodes suivent ceux de la bibliothèque (ex. 4 à 6 après 1 à 3) n'est plus bloquée par « saison déjà présente ».
- Par prudence, si certains noms de la bibliothèque ne permettent pas de lire le numéro d'épisode, l'appli garde l'ancien comportement plutôt que de risquer un doublon. Cette comparaison s'applique avec les noms d'origine gardés (pas encore avec le nommage personnalisé).

## 0.1.2 — 26/11/2026

**Correction**
- Une destination qui n'est qu'une **coquille vide** (dossiers sans aucun fichier, restes d'un ancien rangement raté) ne bloque plus le rangement : l'appli la reprend au lieu de répondre « la destination existe déjà ». Un dossier qui contient le moindre fichier reste protégé, rien n'est jamais écrasé.

**Publication**
- L'image Docker est construite automatiquement à partir des programmes déjà compilés (bien plus rapide).

## 0.1.1 — 26/11/2026

**Corrections importantes**
- Un dossier **sans aucune vidéo** n'est plus jamais rangé. Avant, un dossier créé par Resilio avant l'arrivée de son contenu pouvait être jugé « prêt » et déplacé vide : la série devenait introuvable dans Plex. Il reste maintenant « en attente » (*aucune vidéo pour l'instant*), et le rangement manuel est refusé aussi.
- Un dossier vide n'est plus compté comme « **déjà présent** » : une saison n'existe que si une vraie vidéo (plus de 0 octet) s'y trouve ; un dossier de film sans vidéo n'entre plus dans l'index des doublons.

**Nouveautés**
- **Windows** : le programme se lance par un simple double-clic (sans argument, il démarre l'interface). Ouvrez ensuite <http://localhost:3050>.
- L'**analyse automatique** se fait toutes les **2 minutes** par défaut (0 = seulement à la demande).
- Mot de passe : blocage après 5 essais ratés (15 minutes, par adresse), en-têtes de sécurité.
- README en français, anglais et espagnol avec lien de téléchargement ; exemple Docker `ghcr.io/keketrial/kesal-rangeur`.

*English: folders without any video are never filed nor counted as "already present" (fixes series going missing when Resilio created the folder before its content); the Windows build starts with a double-click; automatic scan every 2 minutes by default; login lockout after 5 failed attempts.*

## 0.1.0 — 26/11/2026

Première version publique : reconnaissance des films et séries avec TMDB (titres dans votre langue), doublons, copie/déplacement (reflink, lien physique, copie), modes Simulation / Manuel / Auto, « À trier », historique avec annulation, Plex / Jellyfin / Home Assistant, interface web, Docker, serveur MCP.

> Défaut connu, corrigé en 0.1.1 : un dossier vide pouvait être rangé comme s'il était complet. Préférez la 0.1.1.
