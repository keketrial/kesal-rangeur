# kesal-rangeur

[🇫🇷 Français](README.md) · [🇬🇧 English](README.en.md) · 🇪🇸 Español

**Ordena automáticamente tus descargas** (películas, series, dibujos animados, programas, documentales, espectáculos…) en el sitio correcto de tu biblioteca, con **los títulos en tu idioma**.

¿Por qué? Sonarr y Radarr se apoyan en TheTVDB, que suele dar títulos en inglés. `kesal-rangeur` usa **TMDB**, que tiene los títulos en español, francés, etc.

## Qué hace

- Vigila una o varias **carpetas de entrada** (Resilio Sync, cliente torrent…) y espera a que terminen las transferencias.
- Reconoce cada película o serie con **TMDB**, elige la carpeta correcta (serie existente, nueva temporada, categoría) y detecta los **duplicados**.
- **Tres modos**, que se cambian arriba en la interfaz: Simulación (no se escribe nada), Manual (validas cada elemento) o Auto (ordena solo lo seguro).
- **Copiar** o **Mover** según la carpeta de entrada. Copia instantánea en Btrfs/XFS (reflink).
- Si hay duda, el elemento va a **Por clasificar**: tú eliges y la app lo recuerda.
- Se conservan los nombres originales por defecto, o nombres a tu gusto (año, resolución, códec H.265/AV1, audio…).
- **Historial con botón Deshacer** para cada ordenación.
- Avisa a **Plex**, **Jellyfin** y Home Assistant después de ordenar.

En el primer arranque la app está en **Simulación**: muestra todo lo que haría (archivos, tamaño, renombrados, duplicados) sin escribir nada.

## Instalación

### Docker (recomendado, por ejemplo en un NAS)

```yaml
services:
  rangeur:
    image: ghcr.io/TU-CUENTA/kesal-rangeur:latest
    ports: ["3050:3050"]
    user: "1026:100"                 # PUID:PGID del propietario de tus medios
    volumes:
      - ./config:/config
      - /volume1/MEDIOS:/medios      # un solo montaje para la entrada y los destinos
```

Mantén **un solo montaje** para la entrada y los destinos: así las copias y los movimientos son instantáneos. Luego abre `http://direccion-del-servidor:3050`.

### Binarios

Windows, Linux y macOS: ver *Releases*. Ejecuta `kesal-rangeur serve`.

### Compilar

```sh
cargo build --release
```

## Ajustes

Todo se configura en la interfaz (botón `?` para la ayuda). Necesitas una **clave TMDB** gratuita, que se crea en [themoviedb.org](https://www.themoviedb.org/settings/api): cada uno usa la suya.

Para proteger la interfaz, define `KESAL_PASSWORD`. Para acceder con un nombre de dominio, añádelo a `KESAL_ALLOWED_HOSTS`. Mantenla en tu red local o detrás de una VPN.

## Asistente IA (opcional)

`kesal-rangeur mcp` permite que un asistente (Claude Code, Claude Desktop…) maneje una instancia ya en marcha. Mantiene la Simulación activa hasta que aceptes las copias reales. Detalles: [docs/mcp.md](docs/mcp.md).

## Licencia

GPL v3, ver [LICENSE](LICENSE). App gratuita (la API de TMDB lo es para uso no comercial).

## TMDB

> This product uses the TMDB API but is not endorsed or certified by TMDB.
> Este producto utiliza la API de TMDB pero no está respaldado ni certificado por TMDB.

---

<img src="web/tmdb-logo.svg" alt="TMDB" width="200">

This application uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by TMDB.
