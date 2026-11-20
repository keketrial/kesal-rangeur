# kesal-rangeur

[🇫🇷 Français](README.md) · [🇬🇧 English](README.en.md) · 🇪🇸 Español

**Ordena automáticamente tus descargas** (películas, series, dibujos animados, programas, documentales, espectáculos…) en el sitio correcto de tu biblioteca, con **los títulos en tu idioma**.

¿Por qué? Sonarr y Radarr se apoyan en TheTVDB, que suele dar títulos en inglés. `kesal-rangeur` usa **TMDB**, que tiene los títulos en español, francés, etc.

## Qué hace

- Vigila una o varias **carpetas de entrada** (Resilio Sync, cliente torrent…).
- **Espera a que termine la transferencia**: sin archivos temporales (`.!sync`, `.part`…) y sin cambios durante 30 min (configurable). Se comprueba de nuevo justo antes de copiar.
- **Reconoce** película o serie, temporada, pack completo, calidad e idioma a partir del nombre de la release.
- **Identifica con TMDB** en tu idioma y deduce la categoría por los géneros (Animación, Documental, Reality, Talk…). Admite reglas personales.
- **Encuentra la carpeta de serie existente** (aunque tenga otro nombre) y añade la temporada; si no, crea « Título (año) ». Los remakes con el mismo título se distinguen por el año.
- Maneja series con **una ficha de TMDB por temporada** y renumera los episodios para que Plex y Jellyfin los reconozcan.
- **Nombres al estilo FileBot (opcional)**: mismas variables (`{n} {y} {s00e00} {t}`…), preajustes FileBot y Plex, títulos de episodios de TMDB, vista previa antes → después. Desactivado por defecto (se conservan los nombres originales).
- **Detecta duplicados** (película: título + año; episodio: serie + SxxEyy).
- **Sin duplicar espacio**: reflink (Btrfs/XFS), si no enlace físico, si no copia normal. El original se queda donde está.
- **En caso de duda → « Por clasificar »**: fichas de TMDB con póster, tú eliges y la app lo recuerda.
- **Historial con botón Deshacer**, reintentos automáticos, nunca deja nada a medias.
- Avisa a **Plex** y **Jellyfin** (y Home Assistant) tras ordenar.
- Interfaz web apta para móvil con **ayuda integrada** (botón `?`).

## Seguridad: modo de prueba

Al primer arranque la app está en **modo de prueba**: analiza y propone, pero **no escribe nada**. Revisa el resultado y desactiva el modo de prueba en Ajustes cuando estés listo. Cada ordenación real se puede deshacer.

## Instalación

### Docker (recomendado, p. ej. en un NAS)

```yaml
services:
  rangeur:
    image: ghcr.io/TU-CUENTA/kesal-rangeur:latest
    ports: ["3050:3050"]
    user: "1026:100"                 # PUID:PGID propietario de tus medios
    volumes:
      - ./config:/config
      - /volume1/MEDIOS:/medios      # UN SOLO montaje para entrada y destinos
```

> **Un solo montaje para todo.** Para copias instantáneas, entrada y destinos deben estar en el mismo volumen Btrfs/XFS *y* dentro de un único montaje de Docker. Compruébalo con:
> `docker exec kesal-rangeur kesal-rangeur selftest /medios/entrada /medios/peliculas`

Después abre `http://direccion-del-servidor:3050` y sigue la ayuda integrada (`?`).

### Binarios

Windows (`.exe`), Linux y macOS (x86 y ARM): ver *Releases*. Ejecuta `kesal-rangeur serve`. Las copias reflink solo funcionan en Linux; en otros sistemas, enlace físico o copia normal.

### Compilar

```sh
cargo build --release     # Rust reciente; la interfaz va integrada en el binario
```

## Ajustes

Todo se configura en la interfaz. Los archivos están en la carpeta de configuración: `config.toml`, `secrets.json` (claves, permisos 600), `choices.json`, `history.json`, `tmdb-cache.json`.

**Clave TMDB**: gratuita en [themoviedb.org](https://www.themoviedb.org/settings/api). Cada usuario introduce la suya; no se incluye ninguna. La ayuda de la app tiene una guía paso a paso.

## Asistente (MCP)

Un asistente de IA (Claude Code, Claude Desktop…) puede ayudarle a configurar y usar la app: `kesal-rangeur mcp` es un servidor [MCP](https://modelcontextprotocol.io) (stdio) que controla una instancia **ya en marcha** mediante su API HTTP. Mantiene activo el modo de prueba hasta que usted acepte explícitamente las copias reales.

```sh
claude mcp add kesal-rangeur -- kesal-rangeur mcp
# con Docker:
claude mcp add kesal-rangeur -- docker exec -i kesal-rangeur kesal-rangeur mcp
# app que no está en http://127.0.0.1:3050:
claude mcp add kesal-rangeur -e KESAL_URL=http://nas:3050 -- kesal-rangeur mcp
```

Después pida al asistente que use el prompt `setup_kesal_rangeur` (primera configuración guiada). Detalles: [docs/mcp.md](docs/mcp.md).

## Seguridad de red

- **Contraseña (recomendada)**: define la variable `KESAL_PASSWORD`; toda la interfaz la pedirá (cualquier nombre de usuario).
- **Nombres de host**: se acepta el acceso por dirección IP, `localhost`, un nombre corto (servicio Docker) o `.local`. Para un nombre de dominio (túnel, proxy inverso), añádelo a `KESAL_ALLOWED_HOSTS` (separado por comas); protege contra el « DNS rebinding ».
- Las peticiones que modifican algo se rechazan si vienen de otro sitio web (control de la cabecera `Origin`).
- Un token (Plex, Jellyfin, Home Assistant) está **ligado a la dirección con la que se introdujo**: si la dirección cambia sin un token nuevo, el anterior se borra.
- Aun así, mantén la interfaz en tu red local o detrás de una VPN; no la expongas directamente a Internet.

## Próximamente

Búsqueda en trackers (Prowlarr / Jackett, Torznab), elección de release según tus preferencias, envío al cliente torrent, vigilancia de nuevos episodios.

## Licencia

GPL v3 — ver [LICENSE](LICENSE). Esta app debe seguir siendo **gratuita** (la API de TMDB es gratuita solo para uso no comercial).

## TMDB

> This product uses the TMDB API but is not endorsed or certified by TMDB.
> Este producto utiliza la API de TMDB pero no está respaldado ni certificado por TMDB.
