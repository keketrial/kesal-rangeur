"use strict";
const $ = (s, r = document) => r.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const CATS = ["film", "serie", "dessin_anime", "telerealite", "documentaire", "spectacle"];
const catName = (c) => t("cat_" + c);
let S = { status: null, items: [], settings: null, view: "accueil", filter: "ranger" };

async function api(path, opts) {
  const o = Object.assign({ headers: { "Content-Type": "application/json" } }, opts || {});
  if (o.body && typeof o.body !== "string") o.body = JSON.stringify(o.body);
  const r = await fetch(path, o);
  if (!r.ok) throw new Error((await r.text()) || r.statusText);
  return r.json();
}
let toastT;
function toast(msg) {
  const el = $("#toast"); el.textContent = msg; el.classList.add("show");
  clearTimeout(toastT); toastT = setTimeout(() => el.classList.remove("show"), 3200);
}
const poster = (p, w = 154) => p ? `<img class="poster" loading="lazy" alt="" src="https://image.tmdb.org/t/p/w${w}${esc(p)}">` : `<div class="poster"></div>`;
const TIPS = {
  tmdb: "Clé gratuite TMDB : donne les titres dans votre langue, les affiches et les genres.",
  langue: "Langue des titres (fr-FR, es-ES…). TMDB renvoie l'original s'il n'a pas de traduction.",
  arrivee: "Dossier où arrivent vos téléchargements (Resilio, torrent…). Vous pouvez en ajouter plusieurs.",
  modes: "Automatique : l'appli devine la catégorie. « Tout va dans… » : tout ce dossier part dans une destination fixe.",
  action: "Copier : l'original reste (conseillé avec Resilio). Déplacer : l'original disparaît après vérification de la copie.",
  calme: "Un élément n'est rangé que si aucun fichier temporaire n'est présent et si rien de nouveau n'est arrivé depuis ce nombre de minutes.",
  destinations: "Un dossier par catégorie. Une série existante reçoit la nouvelle saison ; sinon un dossier est créé.",
  regles: "Forcer la catégorie d'un titre (ex. « Pékin Express » → télé). Prioritaire sur TMDB.",
  doublons: "Ce qui est déjà dans la bibliothèque n'est jamais recopié.",
  nommage: "Par défaut, noms d'origine gardés. Vous pouvez cocher les éléments à garder dans le nom (année, résolution, codec…).",
  methode: "Reflink (instantané, sans place en plus), sinon lien physique, sinon copie normale.",
  fonctionnement: "Mode (simulation / manuel / automatique), méthode de copie et analyse automatique.",
  essai: "Simulation : l'appli analyse et propose, mais ne copie, ne déplace et ne supprime rien.",
  simulation: "Tout ce qui serait fait : fichiers, volume, dossiers créés, renommages, doublons. Rien n'est écrit.",
  auto: "Analyse seule toutes les N minutes et range ce qui est sûr. Doublons et doutes ne sont jamais rangés seuls.",
  services: "Après un rangement, Plex / Jellyfin rafraîchissent seulement le dossier concerné. Facultatif.",
  attente: "Chaque élément arrivé et ce que l'appli en ferait. Rien n'est exécuté en simulation.",
  trier: "Quand l'appli hésite, elle ne range pas : vous choisissez la bonne fiche, elle s'en souvient.",
  historique: "Les rangements effectués. Chacun peut être annulé : seuls les fichiers créés sont retirés.",
  bibliotheque: "Ce que l'appli a compris de vos dossiers existants. Sert à retrouver le bon dossier et repérer les doublons.",
  docker: "Un seul montage pour tout, afin que la copie soit instantanée.",
};
const helpBtn = (a) => `<button class="help-btn" type="button" data-tip-for="${a}" aria-label="Aide : ${esc(TIPS[a] || a)}">?</button>`;
const short = (p, n = 70) => { p = String(p); return p.length > n ? "…" + p.slice(-n) : p; };

// ---------- navigation ----------
const TABS = [
  ["accueil", "tab_home"], ["simulation", "tab_sim"], ["plan", "tab_plan"], ["trier", "tab_sort"], ["historique", "tab_history"],
  ["bibliotheque", "tab_library"], ["reglages", "tab_settings"], ["journal", "tab_log"], ["apropos", "tab_about"],
];
const VIEWS = {}; // rempli plus bas : id → fonction qui dessine l'onglet
const dirty = new Set(); // onglets à redessiner la prochaine fois qu'on les ouvre
const scrollOf = {};
function invalidate(...ids) { (ids.length ? ids : Object.keys(VIEWS)).forEach((i) => dirty.add(i)); if (!ids.length || ids.includes("plan")) dirty.add("simulation"); }
function route() {
  scrollOf[S.view] = window.scrollY;
  const v = (location.hash || "#/accueil").replace("#/", "").split("?")[0];
  S.view = TABS.some((x) => x[0] === v) ? v : "accueil";
  render();
}
// ---------- sélecteur de mode (en haut) ----------
const MODES = [["simulation", "🛡️", "Simulation", "Rien n'est écrit : l'appli montre ce qu'elle ferait."],
  ["manuel", "✋", "Manuel", "Vous validez et rangez chaque élément vous-même."],
  ["auto", "⚡", "Auto", "L'appli surveille le dossier d'arrivée et range seule ce qui est sûr."]];
function renderModeSwitch(st) {
  const cur = st.essai ? "simulation" : st.auto_range ? "auto" : "manuel";
  $("#modeSw").innerHTML = MODES.map(([k, ico, n, d]) => `<button type="button" class="m-${k} ${k === cur ? "on" : ""}" data-mode="${k}" title="${esc(d)}" aria-pressed="${k === cur}"><span class="dot-s"></span>${ico} ${n}</button>`).join("");
}
async function setMode(m) {
  const st = S.status || {}; const cur = st.essai ? "simulation" : st.auto_range ? "auto" : "manuel";
  if (m === cur) return;
  if (m !== "simulation") {
    if (!st.dernier_scan) { toast("Lancez d'abord une analyse et regardez la Simulation."); return; }
    const msg = m === "auto"
      ? `Passer en mode AUTO ?\n\nL'appli analysera le dossier d'arrivée toutes les ${st.intervalle_min || 2} min et rangera seule ce qui est sûr (action choisie pour chaque dossier : copier ou déplacer). Doublons et doutes ne sont jamais rangés seuls. Chaque rangement reste annulable.`
      : `Passer en mode MANUEL ?\n\nRien ne se fera tout seul : vous validez chaque élément avec « Valider et ranger ». Chaque rangement reste annulable.`;
    if (!confirm(msg)) return;
  }
  try { await api("/api/mode", { method: "POST", body: { mode: m, confirm: true } }); toast(m === "simulation" ? "Retour en simulation" : m === "auto" ? "Mode Auto activé" : "Mode Manuel activé"); invalidate(); await refreshStatus(); render(); }
  catch (e) { toast(e.message); }
}
function counts() { return (S.status && S.status.compteurs) || {}; }
function renderTabs() {
  const c = counts();
  const badge = { plan: (c.ranger || 0) + (c.doublon || 0) + (c.attendre || 0), trier: c.a_trier || 0 };
  $("#tabs").innerHTML = TABS.map(([id, k]) =>
    `<a href="#/${id}" class="${S.view === id ? "on" : ""}">${t(k)}${badge[id] ? `<span class="n">${badge[id]}</span>` : ""}</a>`).join("");
}
async function refreshStatus() {
  let fresh; try { fresh = await api("/api/status"); } catch (e) { return; }
  const sig = (x) => x && JSON.stringify([x.compteurs, x.essai, x.auto_range, x.tmdb_ok, x.entrees, x.destinations, x.plex, x.jellyfin, x.dernier_scan, x.erreur, x.films_connus, x.choix_retenus]);
  const changed = sig(fresh) !== sig(S.status);
  S.status = fresh;
  if (changed) { invalidate("accueil", "plan", "trier"); if (S.status && document.getElementById("v-" + S.view) && ["accueil", "plan", "trier"].includes(S.view) && !S._busyRender) { S._busyRender = true; render().finally(() => { S._busyRender = false; }); } }
  const st = S.status;
  if (st.version) { const v = $("#ver"); v.textContent = "v" + st.version; v.hidden = false; }
  $("#pill").textContent = st.scanning ? t("pill_busy") : (st.en_cours ? "Rangement…" : "");
  $("#pill").hidden = !(st.scanning || st.en_cours); $("#pill").className = "pill busy";
  renderModeSwitch(st);
  $("#btnScan").disabled = st.scanning;
  $("#btnScan").innerHTML = st.scanning ? `<span class="spin"></span>${t("scanning")}` : t("scan");
  renderTabs();
}
let wasScanning = false;
async function tick() {
  await refreshStatus();
  const sc = S.status && S.status.scanning;
  if (wasScanning && !sc) { await loadPlan(); invalidate("accueil", "plan", "trier", "bibliotheque", "historique"); toast(S.status.erreur ? "Analyse impossible : " + S.status.erreur : "Analyse terminée"); render(); }
  wasScanning = !!sc;
}
async function loadPlan() {
  try { S.items = (await api("/api/plan")).items; } catch (e) { S.items = []; }
  try { const d = (await api("/api/settings")).destinations || {}; S.dests = Object.keys(d).filter((k) => d[k]); } catch (e) { S.dests = S.dests || []; }
}

function pane(id) {
  let el = document.getElementById("v-" + id);
  if (!el) { el = document.createElement("section"); el.id = "v-" + id; el.hidden = true; $("#view").appendChild(el); }
  return el;
}
/** Affiche l'onglet courant. Les autres restent en mémoire (cachés) : pas de reconstruction, pas de flash des affiches. */
async function render() {
  renderTabs();
  const id = S.view, el = pane(id);
  if (dirty.has(id) || !el.dataset.done) {
    dirty.delete(id);
    el.dataset.done = "1";
    await VIEWS[id](el);
  }
  document.querySelectorAll("#view > section").forEach((x) => { x.hidden = x !== el; });
  window.scrollTo(0, scrollOf[id] || 0);
  if (id === "journal") vLog(el); // le journal se rafraîchit à chaque visite
}
/** Prépare les autres onglets en arrière-plan pour que le changement d'onglet soit instantané. */
async function prewarm() {
  for (const id of Object.keys(VIEWS)) {
    if (id === S.view || id === "reglages") continue;
    const el = pane(id);
    if (!el.dataset.done) { el.dataset.done = "1"; dirty.delete(id); try { await VIEWS[id](el); } catch (e) {} }
    await new Promise((r) => setTimeout(r, 60));
  }
}

// ---------- Accueil ----------
function transfers() {
  const l = S.items.filter((i) => decisionOf(i) === "attendre");
  if (!l.length) return "";
  return `<div class="card transfers"><b><span class="spin"></span>Transferts en cours (${l.length})</b>
    <p class="muted" style="margin:4px 0 8px">Ces éléments ne sont pas rangés tant qu'ils ne sont pas complets. L'appli surveille et les rangera ensuite.</p>
    ${l.map((i) => `<div class="tr-row"><div class="mono" style="font-size:12px">${esc(short(i.nom, 70))}</div><div class="muted" style="font-size:13px">${esc(i.decision.raison)}</div></div>`).join("")}</div>`;
}
function vHome(v) {
  const st = S.status || {}; const c = counts();
  const ready = st.tmdb_ok && st.entrees > 0 && st.destinations > 0;
  const step = (ok, txt, anchor, link) => `<li><span class="dot ${ok ? "ok" : ""}">${ok ? "✓" : "·"}</span><span style="flex:1">${txt}</span>${helpBtn(anchor)}${ok ? "" : `<a class="btn small" href="${link}">Régler</a>`}</li>`;
  const stat = (n, label, f) => `<a class="stat" href="#/plan" data-filter="${f}"><b>${n || 0}</b><span>${label}</span></a>`;
  v.innerHTML = `
    <h2>Accueil</h2><p class="lead">Ce que l'appli a compris des dossiers d'arrivée.</p>
    ${st.essai ? `<div class="banner"><span>🛡️</span><span>${t("dry_banner")} ${helpBtn("essai")}</span></div>` : `<div class="banner bad"><span>⚡</span><span><b>Mode réel :</b> les éléments « À ranger » peuvent être copiés dans votre bibliothèque (bouton « Ranger »). Chaque rangement est annulable depuis l'Historique. ${helpBtn("essai")}</span></div>`}
    ${st.erreur ? `<div class="banner bad"><span>⚠️</span><span>${esc(st.erreur)}</span></div>` : ""}
    ${ready ? "" : `<div class="card"><b>Pour démarrer</b><ul class="steps">
      ${step(st.tmdb_ok, "Clé TMDB (pour les titres en français)", "tmdb", "#/reglages")}
      ${step(st.entrees > 0, "Un dossier d'arrivée (là où Resilio dépose)", "arrivee", "#/reglages")}
      ${step(st.destinations > 0, "Les dossiers de destination (films, séries…)", "destinations", "#/reglages")}
    </ul></div>`}
    <div class="grid">
      ${stat(c.ranger, t("d_ranger"), "ranger")}${stat(c.doublon, t("d_doublon"), "doublon")}${stat(c.a_trier, t("d_a_trier"), "a_trier")}
      ${stat(c.attendre, t("d_attendre"), "attendre")}${stat(c.ignorer, t("d_ignorer"), "ignorer")}${c.fait ? stat(c.fait, "Rangés", "fait") : ""}
    </div>
    ${transfers()}
    <p class="muted" style="margin-top:14px">${st.dernier_scan ? "Dernière analyse : " + new Date(st.dernier_scan * 1000).toLocaleString() : "Aucune analyse pour l'instant — cliquez sur « " + t("scan") + " »."}
    ${st.films_connus != null ? ` · ${st.films_connus} films connus · ${st.choix_retenus || 0} choix retenu(s)` : ""}</p>
    <h3>Services</h3>
    <div class="chips"><span class="tag ${st.tmdb_ok ? "ok" : "warn"}">TMDB ${st.tmdb_ok ? "connecté" : "non configuré"}</span>
      <span class="tag ${st.plex ? "ok" : ""}">Plex ${st.plex ? "configuré" : "non configuré"}</span>
      <span class="tag ${st.jellyfin ? "ok" : ""}">Jellyfin ${st.jellyfin ? "configuré" : "non configuré"}</span></div>`;
  v.querySelectorAll("[data-filter]").forEach((a) => a.addEventListener("click", () => { if (S.filter !== a.dataset.filter) { S.filter = a.dataset.filter; invalidate("plan"); } }));
}

// ---------- Simulation complète + choix du mode ----------
const GO = (n) => n >= 1e9 ? (n / 1e9).toFixed(1) + " Go" : n >= 1e6 ? Math.round(n / 1e6) + " Mo" : n + " o";
function rapportTexte(r) {
  const T = r.totaux, c = T.compte || {}, L = [];
  L.push(`SIMULATION kesal-rangeur — ${new Date().toLocaleString()}`, `Rien n'a été copié, déplacé ni supprimé.`, "");
  L.push(`À ranger : ${c.ranger || 0} élément(s) · ${T.fichiers} fichier(s) · ${GO(T.octets)} · ${T.nouveaux_dossiers} nouveau(x) dossier(s) · ${T.renommes} fichier(s) renommé(s)`);
  L.push(`Doublons : ${c.doublon || 0} · À trier : ${c.a_trier || 0} · En attente : ${c.attendre || 0} · Ignorés : ${c.ignorer || 0}`, "");
  const sec = (k, titre, f) => { const l = r.lignes.filter((x) => x.decision === k); if (!l.length) return; L.push(`== ${titre} (${l.length}) ==`); l.forEach((x) => L.push(...f(x))); L.push(""); };
  sec("ranger", "À RANGER", (x) => [`• ${x.nom}`, `    → ${x.vers}  [${x.raison}, ${x.fichiers} fichier(s), ${GO(x.octets)}]`, ...x.changements.map((m) => `      ${m.de}  →  ${m.vers}`)]);
  sec("doublon", "DOUBLONS (déjà présents, rien ne sera copié)", (x) => [`• ${x.nom}`, `    déjà : ${x.vers} — ${x.raison}`]);
  sec("a_trier", "À TRIER (vous devrez choisir)", (x) => [`• ${x.nom} — ${x.raison}`]);
  sec("attendre", "EN ATTENTE", (x) => [`• ${x.nom} — ${x.raison}`]);
  sec("ignorer", "IGNORÉS", (x) => [`• ${x.nom} — ${x.raison}`]);
  return L.join("\n");
}
async function vSim(v) {
  const r = await api("/api/report"); S.report = r;
  const T = r.totaux, c = T.compte || {}; const mode = r.essai ? "simulation" : r.auto_range ? "auto" : "manuel";
  const none = !r.dernier_scan;
  const stat = (n, l) => `<div class="stat"><b>${n}</b><span>${l}</span></div>`;
  const lst = (k, titre, f, open) => { const l = r.lignes.filter((x) => x.decision === k); if (!l.length) return "";
    return `<details class="card" ${open ? "open" : ""}><summary><b>${titre}</b> <span class="muted">(${l.length})</span></summary>${l.slice(0, 80).map(f).join("")}${l.length > 80 ? `<p class="muted">… et ${l.length - 80} autres : voir le rapport texte.</p>` : ""}</details>`; };
  const row = (x, extra) => `<div style="margin:8px 0"><div class="mono" style="font-size:12px;color:var(--muted)">${esc(x.nom)}</div>${x.titre ? `<b>${esc(x.titre)}</b> ` : ""}${extra}</div>`;
  v.innerHTML = `<h2>Simulation ${helpBtn("simulation")}</h2>
    <p class="lead">Tout ce que l'appli ferait si vous la laissiez travailler. <b>Rien n'est écrit</b> tant que vous ne changez pas de mode.</p>
    ${none ? `<div class="empty"><b>Pas encore d'analyse</b>Cliquez sur « ${t("scan")} » en haut à droite.</div>` : `
    <div class="grid">${stat(c.ranger || 0, "à ranger")}${stat(T.fichiers, "fichiers à transférer")}${stat(GO(T.octets), "volume concerné")}${stat(T.nouveaux_dossiers, "nouveaux dossiers")}
      ${stat(T.renommes, "fichiers renommés")}${stat(c.doublon || 0, "doublons (ignorés)")}${stat(c.a_trier || 0, "à trier")}</div>
    <p class="muted" style="margin-top:8px">Méthode de copie : <b>${esc(r.methode)}</b>. Avec reflink ou lien physique, le volume concerné n'occupe pas de place en plus.</p>
    <div class="chips" style="margin:10px 0"><button class="btn small" id="simDl">Télécharger le rapport (.txt)</button><button class="btn small" id="simCopy">Copier</button></div>
    ${lst("ranger", "À ranger", (x) => row(x, `<span class="tag ${x.raison === "nouveau dossier" ? "ok" : "info"}">${esc(x.raison)}</span><div class="path mono" style="font-size:12px">→ ${esc(x.vers)}</div><div class="muted" style="font-size:12px">${x.fichiers} fichier(s) · ${GO(x.octets)}</div>${r.essai ? `<button class="btn small" data-test="${esc(x.nom)}" style="margin-top:4px">Essayer sur cet élément</button>` : ""}${x.changements.length ? `<details class="ren"><summary>${x.changements.length} renommage(s)</summary>${x.changements.slice(0, 10).map((m) => `<div class="mono">${esc(m.de)} → ${esc(m.vers)}</div>`).join("")}</details>` : ""}`), true)}
    ${lst("doublon", "Doublons : déjà dans la bibliothèque, rien ne sera copié", (x) => row(x, `<span class="tag dup">Doublon</span><div class="path mono" style="font-size:12px">déjà : ${esc(x.vers)}</div>`), false)}
    ${lst("a_trier", "À trier : l'appli hésite, vous choisirez", (x) => row(x, `<span class="tag warn">${esc(x.raison)}</span>`), false)}
    ${lst("attendre", "Transfert en cours (pas fini)", (x) => row(x, `<span class="tag warn">${esc(x.raison)}</span>`), false)}`}
    ${none ? "" : `<div class="banner info" style="margin-top:14px"><span>👆</span><span>Le résultat vous convient ? Choisissez le mode en haut : <b>Manuel</b> (vous validez chaque élément) ou <b>Auto</b> (l'appli surveille et range seule). Vous pouvez aussi <b>essayer sur un seul élément</b> ci-dessus, sans toucher au reste de la bibliothèque.</span></div>`}`;
  const dl = $("#simDl"); if (dl) {
    dl.onclick = () => { const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob([rapportTexte(r)], { type: "text/plain;charset=utf-8" })); a.download = "simulation-kesal-rangeur.txt"; a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 2000); };
    $("#simCopy").onclick = async () => { try { await navigator.clipboard.writeText(rapportTexte(r)); toast("Rapport copié"); } catch (e) { toast("Copie impossible ici : utilisez « Télécharger »"); } };
  }
  bindTest(v);
}

// ---------- En attente (plan) ----------
function decisionOf(it) { return it.decision.decision; }
function itemCard(it, opts = {}) {
  const d = it.decision, r = it.reconnu;
  const title = r ? `${esc(r.titre)}${r.annee ? ` <span class="muted">(${r.annee})</span>` : ""}` : `<span class="muted">${esc(it.release.title || "—")}</span>`;
  const tags = [];
  if (r) tags.push(`<span class="tag info">${catName(r.categorie)}</span>`);
  if (r && r.titre_original && r.titre_original !== r.titre) tags.push(`<span class="tag">${esc(r.titre_original)}</span>`);
  if (it.release.resolution) tags.push(`<span class="tag">${esc(it.release.resolution)}</span>`);
  let detail = "";
  switch (d.decision) {
    case "ranger":
      tags.unshift(r && r.media === "movie" ? `<span class="tag ok">Sera rangé</span>` : `<span class="tag ok">${d.nouveau_dossier ? "Nouveau dossier" : "Dossier de la série existant"}</span>`);
      detail = `<span class="path mono" title="${esc(d.vers)}">→ ${esc(short(d.vers, 90))}</span>`;
      if (d.renommages.length) {
        const renum = d.renumerotation ? `Renumérotation S${String(d.renumerotation[0]).padStart(2, "0")} → S${String(d.renumerotation[1]).padStart(2, "0")} · ` : "";
        const named = d.fichiers && d.fichiers.length;
        detail += `<details class="ren"><summary>${renum}${d.renommages.length} fichier(s) ${named ? "renommé(s)" : "modifié(s)"} — voir l'aperçu</summary>${d.renommages.map((x) => `<div class="mono">${esc(named ? x.ancien : x.ancien.split("/").pop())}<br>→ ${esc(named ? x.nouveau : x.nouveau.split("/").pop())}</div>`).join("")}</details>`;
      }
      break;
    case "doublon": tags.unshift(`<span class="tag dup">Doublon</span>`); detail = `<span class="path">${esc(d.raison)} — <span class="mono">${esc(short(d.existant, 80))}</span></span>`; break;
    case "a_trier": tags.unshift(`<span class="tag warn">À trier</span>`); detail = `<span class="path">${esc(d.raison)}</span>`; break;
    case "attendre": tags.unshift(`<span class="tag warn">En attente</span>`); detail = `<span class="path">${esc(d.raison)}</span>`; break;
    case "ignorer": tags.unshift(`<span class="tag">Ignoré</span>`); detail = `<span class="path">${esc(d.raison)}</span>`; break;
    case "fait": tags.unshift(`<span class="tag ok">✓ Rangé</span>`); detail = `<span class="path mono">→ ${esc(short(d.vers, 90))}</span>`; break;
  }
  const real = S.status && !S.status.essai;
  const canRun = d.decision === "ranger";
  const dest = canRun && r && (S.dests || []).length > 1 ? `<label class="dest-pick">Ranger dans :
      <select data-dest="${esc(it.nom)}" data-id="${r.tmdb_id}" data-media="${r.media}" data-cur="${r.categorie}">${S.dests.map((c) => `<option value="${c}" ${c === r.categorie ? "selected" : ""}>${catName(c)}</option>`).join("")}</select></label>` : "";
  const actions = opts.noActions || d.decision === "fait" ? "" : `<div class="actions">
      <button class="btn small" data-fix="${esc(it.nom)}">Corriger la fiche…</button>${dest}
      ${canRun ? (real ? `<button class="btn small primary" data-run="${esc(it.nom)}">Valider et ranger</button>` : `<button class="btn small" data-test="${esc(it.nom)}" title="Copie ce seul élément pour voir le résultat. L'original reste, annulable depuis l'Historique.">Essayer sur cet élément</button>`) : ""}</div>`;
  return `<div class="item">${poster(r && r.affiche)}<div class="body">
    <div class="orig mono">${esc(it.nom)}</div><div class="title">${title}</div>
    <div class="row">${tags.join("")}</div>${detail}${actions}</div></div>`;
}
// Après un clic « Ranger » / « Essayer » : la carte disparaît tout de suite (la suivante vient sous la souris, plus de curseur bloqué),
// puis on attend la fin réelle du travail avant de relire la liste.
S.pending = new Set();
function launched(b, nom, msg, extra) {
  S.pending.add(nom);
  const card = b.closest(".card"); if (card && S.view === "plan") card.remove();
  toast(msg);
  const finish = async () => { S.pending.delete(nom); await loadPlan(); invalidate("accueil", "plan", "trier", "historique", ...extra); await refreshStatus(); render(); };
  let n = 0;
  const poll = async () => {
    n++;
    try { await refreshStatus(); } catch (e) { /* on réessaie */ }
    if ((S.status && S.status.en_cours === 0 && n >= 2) || n > 300) return finish();
    setTimeout(poll, 2000);
  };
  setTimeout(poll, 1500);
}
function bindRun(root) {
  root.querySelectorAll("[data-run]").forEach((b) => b.addEventListener("click", async () => {
    b.disabled = true;
    try { await api("/api/run", { method: "POST", body: { nom: b.dataset.run } }); launched(b, b.dataset.run, "Rangement lancé…", []); }
    catch (e) { toast(e.message); b.disabled = false; }
  }));
}
function bindTest(root) {
  root.querySelectorAll("[data-test]").forEach((b) => b.addEventListener("click", async () => {
    const it = S.items.find((i) => i.nom === b.dataset.test); if (!it || !it.decision.vers) return;
    if (!confirm(`ESSAI sur UN seul élément (le reste de la bibliothèque n'est pas touché)\n\n${it.nom}\n→ ${it.decision.vers}\n\nL'appli va COPIER ce seul élément (l'original reste en place), même si le dossier d'arrivée est réglé sur « Déplacer ». Vous pourrez l'annuler depuis l'Historique.\n\nContinuer ?`)) return;
    b.disabled = true;
    try { await api("/api/run", { method: "POST", body: { nom: b.dataset.test, test: true } }); launched(b, b.dataset.test, "Essai lancé : voir l'Historique", ["simulation"]); }
    catch (e) { toast(e.message); b.disabled = false; }
  }));
}
function bindDest(root) {
  root.querySelectorAll("[data-dest]").forEach((sel) => sel.addEventListener("change", async () => {
    if (sel.value === sel.dataset.cur) return;
    try { await api("/api/choice", { method: "POST", body: { nom: sel.dataset.dest, tmdb_id: +sel.dataset.id, media: sel.dataset.media, category: sel.value } });
      toast("Destination changée : l'appli s'en souviendra"); await loadPlan(); invalidate("accueil", "plan", "trier", "simulation"); await refreshStatus(); render(); }
    catch (e) { toast(e.message); sel.value = sel.dataset.cur; }
  }));
}
function bindFix(root) { root.querySelectorAll("[data-fix]").forEach((b) => b.addEventListener("click", () => openSearch(b.dataset.fix))); }

async function vPlan(v) {
  if (!S.items.length) await loadPlan();
  const c = counts(); const order = ["ranger", "doublon", "a_trier", "attendre", "ignorer", ...(c.fait ? ["fait"] : [])];
  const real = S.status && !S.status.essai;
  const list = S.items.filter((i) => decisionOf(i) === S.filter && !S.pending.has(i.nom));
  v.innerHTML = `<h2>Détail ${helpBtn("attente")}</h2><p class="lead">Ce que l'appli ferait de chaque élément arrivé. Rien n'est exécuté.</p>
    <div class="chips">${order.map((k) => `<button class="chip ${S.filter === k ? "on" : ""}" data-f="${k}">${k === "fait" ? "Rangés" : t("d_" + k)} (${c[k] || 0})</button>`).join("")}
      ${real && S.filter === "ranger" && list.length ? `<button class="btn small primary" id="runAll" style="margin-left:auto">Tout ranger (${list.length})</button>` : ""}</div>
    ${S.items.length === 0 ? `<div class="empty"><b>Rien à afficher</b>Lancez une analyse avec le bouton en haut à droite.</div>` :
      list.length === 0 ? `<div class="empty"><b>Aucun élément dans cette catégorie</b></div>` : list.map((i) => itemCard(i)).join("")}`;
  v.querySelectorAll("[data-f]").forEach((b) => b.addEventListener("click", () => { S.filter = b.dataset.f; vPlan(v); }));
  bindFix(v); bindRun(v); bindTest(v); bindDest(v);
  const ra = $("#runAll");
  if (ra) ra.onclick = async () => {
    if (!confirm(`Ranger les ${list.length} élément(s) « À ranger » ? Chaque rangement pourra être annulé depuis l'Historique.`)) return;
    ra.disabled = true;
    try { await api("/api/run", { method: "POST", body: {} }); toast("Rangement lancé…"); setTimeout(async () => { await loadPlan(); invalidate("accueil", "plan", "trier", "historique"); await refreshStatus(); render(); }, 3000); } catch (e) { toast(e.message); ra.disabled = false; }
  };
}

// ---------- À trier ----------
async function vSort(v) {
  if (!S.items.length) await loadPlan();
  const list = S.items.filter((i) => decisionOf(i) === "a_trier");
  v.innerHTML = `<h2>À trier ${helpBtn("trier")}</h2><p class="lead">Quand l'appli hésite, elle ne range pas : vous choisissez, elle retient.</p>
    ${list.length === 0 ? `<div class="empty"><b>Rien à trier 🎉</b>Tous les éléments reconnus sont sûrs.</div>` : ""}
    ${list.map((it) => `<div class="card"><div class="orig mono">${esc(it.nom)}</div><p style="margin:6px 0"><span class="tag warn">${esc(it.decision.raison)}</span></p>
      <div class="cands" style="padding:0">${it.candidats.slice(0, 4).map((c) => candCard(c, it.nom)).join("") || `<span class="muted">Aucune fiche proposée.</span>`}</div>
      <div class="actions"><button class="btn small" data-fix="${esc(it.nom)}">Chercher autre chose…</button></div></div>`).join("")}`;
  bindFix(v); bindPick(v);
}
function candCard(c, nom) {
  const cat = c.genres.includes(10764) || c.genres.includes(10767) ? "telerealite" : c.genres.includes(99) ? "documentaire" : c.genres.includes(16) ? "dessin_anime" : c.media === "tv" ? "serie" : "film";
  return `<div class="cand">${poster(c.poster, 154)}<div style="min-width:0;flex:1"><b>${esc(c.title)}</b> <span class="muted">(${c.year || "?"})</span>
    <div><span class="tag">${c.media === "tv" ? "Série" : "Film"}</span> <span class="tag info">${catName(cat)}</span>${c.original_title && c.original_title !== c.title ? ` <span class="tag">${esc(c.original_title)}</span>` : ""}</div>
    <p>${esc(c.overview || "Pas de résumé.")}</p>
    <button class="btn small primary" data-pick="${esc(nom)}" data-id="${c.id}" data-media="${c.media}" data-cat="${cat}">C'est celui-ci</button></div></div>`;
}
function bindPick(root) {
  root.querySelectorAll("[data-pick]").forEach((b) => b.addEventListener("click", async () => {
    b.disabled = true;
    try {
      await api("/api/choice", { method: "POST", body: { nom: b.dataset.pick, tmdb_id: +b.dataset.id, media: b.dataset.media, category: b.dataset.cat } });
      toast("Choix retenu : l'appli s'en souviendra"); $("#dlgSearch").close();
      await loadPlan(); invalidate("accueil", "plan", "trier"); await refreshStatus(); render();
    } catch (e) { toast("Erreur : " + e.message); b.disabled = false; }
  }));
}
function openSearch(nom) {
  const it = S.items.find((i) => i.nom === nom); if (!it) return;
  $("#searchTitle").textContent = "Corriger la fiche";
  $("#searchQ").value = it.release.title;
  $("#searchKind").innerHTML = `<option value="all">Films et séries</option><option value="tv">Séries</option><option value="movie">Films</option>`;
  $("#searchKind").value = it.release.seasons.length ? "tv" : "all";
  $("#searchForm button").textContent = "Chercher";
  const run = async () => {
    $("#searchRes").innerHTML = `<p class="muted"><span class="spin"></span>Recherche…</p>`;
    try {
      const res = await api(`/api/tmdb/search?q=${encodeURIComponent($("#searchQ").value)}&kind=${$("#searchKind").value}`);
      $("#searchRes").innerHTML = res.length ? res.map((c) => candCard(c, nom)).join("") : `<p class="muted">Aucun résultat.</p>`;
      bindPick($("#searchRes"));
    } catch (e) { $("#searchRes").innerHTML = `<p class="muted">Erreur : ${esc(e.message)}</p>`; }
  };
  $("#searchForm").onsubmit = (e) => { e.preventDefault(); run(); };
  $("#dlgSearch").showModal(); run();
}

// ---------- Historique ----------
const fmtSize = (n) => n > 1e9 ? (n / 1e9).toFixed(2) + " Go" : n > 1e6 ? (n / 1e6).toFixed(1) + " Mo" : n + " o";
async function vHistory(v) {
  const h = (await api("/api/history")).entrees;
  const real = S.status && !S.status.essai;
  const okNoms = new Set(h.filter((e) => e.statut === "fait").map((e) => e.nom));
  const resolved = (e) => e.statut === "erreur" && okNoms.has(e.nom);
  v.innerHTML = `<h2>Historique ${helpBtn("historique")}</h2><p class="lead">Chaque rangement : avant → après, avec un bouton pour l'annuler.</p>
  ${h.length === 0 ? `<div class="empty"><b>Aucun rangement pour l'instant</b>En mode essai à blanc, rien n'est exécuté, donc rien n'est écrit ici.</div>` :
    h.map((e) => `<div class="card"><div class="row" style="display:flex;gap:6px;align-items:center;flex-wrap:wrap">
      <span class="tag ${e.statut === "fait" || resolved(e) ? "ok" : e.statut === "erreur" ? "bad" : ""}">${e.statut === "fait" ? "Rangé" : resolved(e) ? "Erreur résolue" : e.statut === "erreur" ? "Erreur" : "Annulé"}</span>
      <b>${esc(e.titre)}</b>${e.categorie ? `<span class="tag info">${catName(e.categorie)}</span>` : ""}
      <span class="muted">${new Date(e.t * 1000).toLocaleString()}</span>${e.taille_totale ? `<span class="muted">· ${fmtSize(e.taille_totale)}</span>` : ""}
      ${e.fichiers[0] ? `<span class="tag">${esc(e.fichiers[0].methode)}</span>` : ""}</div>
      <div class="mono muted" style="margin-top:6px">${esc(e.nom)}</div><div class="mono" style="margin-top:2px">→ ${esc(e.vers)}</div>
      ${e.motif ? `<div class="path">${esc(e.motif)}</div>` : ""}
      <div class="actions">${e.statut === "fait" ? `<button class="btn small danger" data-undo="${esc(e.id)}">Annuler ce rangement</button>` : ""}
      ${e.statut === "erreur" && real && !resolved(e) ? `<button class="btn small" data-run="${esc(e.nom)}">Réessayer</button>` : ""}</div></div>`).join("")}`;
  v.querySelectorAll("[data-undo]").forEach((b) => b.addEventListener("click", async () => {
    if (!confirm("Annuler ce rangement ? Les fichiers rangés par l'appli seront retirés de la bibliothèque. L'original dans le dossier d'arrivée n'est pas touché.")) return;
    try { await api("/api/undo", { method: "POST", body: { id: b.dataset.undo } }); toast("Rangement annulé"); invalidate("accueil", "plan"); vHistory(v); } catch (e) { toast("Annulation refusée : " + e.message); }
  }));
  bindRun(v);
}

// ---------- Bibliothèque ----------
async function vLibrary(v, cached) {
  let lib; if (cached && S.lib) lib = S.lib; else { try { lib = await api("/api/library"); } catch (e) { lib = { pret: false }; } S.lib = lib; }
  if (!lib.pret) { v.innerHTML = `<h2>Bibliothèque</h2><div class="empty"><b>Pas encore lue</b>La bibliothèque est lue pendant l'analyse. Lancez-en une.</div>`; return; }
  const q = (S.libq || "").toLowerCase();
  let html = `<h2>Bibliothèque ${helpBtn("bibliotheque")}</h2><p class="lead">Ce que l'appli a compris de vos dossiers existants (${lib.films} films connus pour les doublons).</p>
    <label class="field"><input id="libq" placeholder="Filtrer par titre…" value="${esc(S.libq || "")}"></label>`;
  const films = (lib.liste_films || []).filter((f) => !q || (f.nom + f.titre).toLowerCase().includes(q));
  const MAXF = 300;
  html += `<h3>Films <span class="muted">(${q ? films.length + " sur " + lib.films : lib.films})</span></h3>` + (lib.films === 0
    ? `<p class="muted">Aucun film trouvé (dossiers de films non montés sur cette machine ?).</p>`
    : `<div class="card" style="padding:0;overflow:auto"><table><thead><tr><th>Fichier ou dossier</th><th>Titre lu</th><th>Année</th><th>Dans</th></tr></thead><tbody>
      ${films.slice(0, MAXF).map((f) => `<tr><td class="mono">${esc(f.nom)}</td><td>${esc(f.titre)}</td><td>${f.annee || `<span class="muted">?</span>`}</td><td class="muted">${esc(f.dossier)}</td></tr>`).join("")}</tbody></table></div>
      ${films.length > MAXF ? `<p class="muted">… ${films.length - MAXF} autres. Tapez dans « Filtrer par titre » pour les trouver.</p>` : ""}`);
  for (const [cat, rows] of Object.entries(lib.categories)) {
    const f = rows.filter((r) => !q || (r.nom + r.titre).toLowerCase().includes(q));
    if (!rows.length) { html += `<h3>${esc(cat)} <span class="muted">— vide ou non montée sur cette machine</span></h3>`; continue; }
    const sus = f.filter((r) => r.suspect && !r.integrale).length;
    html += `<h3>${esc(cat)} <span class="muted">(${f.length})</span> ${sus ? `<span class="tag warn">${sus} sans saison détectée</span>` : ""}</h3>
      <div class="card" style="padding:0;overflow:auto"><table><thead><tr><th>Dossier</th><th>Titre lu</th><th>Saisons</th><th>Vidéos</th></tr></thead><tbody>
      ${f.map((r) => `<tr><td class="mono">${esc(r.nom)}</td><td>${esc(r.titre)}${r.annee ? ` <span class="muted">(${r.annee})</span>` : ""}</td>
      <td>${r.saisons.length ? r.saisons.join(", ") : (r.integrale ? "intégrale" : `<span class="tag warn">?</span>`)}</td><td>${r.videos}</td></tr>`).join("")}</tbody></table></div>`;
  }
  v.innerHTML = html;
  const box = $("#libq");
  if (S.libFocus) { box.focus(); box.setSelectionRange(box.value.length, box.value.length); }
  box.addEventListener("input", (e) => { S.libq = e.target.value; S.libFocus = true; clearTimeout(S.lt); S.lt = setTimeout(() => vLibrary(v, true), 150); });
  box.addEventListener("blur", () => { S.libFocus = false; });
}

// ---------- Réglages ----------
async function vSettings(v) {
  const s = S.settings = await api("/api/settings");
  const secTag = (x) => x.ok ? `<span class="tag ok">enregistrée</span>` : `<span class="tag warn">non renseignée</span>`;
  v.innerHTML = `<h2>Réglages</h2><p class="lead">Tout se règle ici. Le bouton <b>?</b> à côté de chaque réglage explique à quoi il sert.</p>
  ${s.dossiers_introuvables.length ? `<div class="banner"><span>⚠️</span><span>Dossiers introuvables sur cette machine : ${s.dossiers_introuvables.map((p) => `<span class="mono">${esc(p)}</span>`).join(", ")}</span></div>` : ""}
  <div class="card"><h3 style="margin-top:0">TMDB — titres et affiches ${helpBtn("tmdb")}</h3>
    <label class="field"><span>Clé API ou jeton d'accès ${secTag(s.secrets.tmdb)}</span>
      <div class="inline"><input id="s_tmdb" type="password" autocomplete="off" placeholder="Coller ici pour la remplacer (laisser vide = inchangée)"><button class="btn" id="b_tmdbtest" type="button">${t("test")}</button></div></label>
    <label class="field"><span>Langue des titres ${helpBtn("langue")}</span>
      <select id="s_lang">${[["fr-FR", "Français"], ["es-ES", "Español"], ["en-US", "English"], ["de-DE", "Deutsch"], ["it-IT", "Italiano"], ["pt-PT", "Português"]].map(([c, n]) => `<option value="${c}" ${s.language === c ? "selected" : ""}>${n} (${c})</option>`).join("")}</select></label></div>

  <div class="card"><h3 style="margin-top:0">Dossiers d'arrivée ${helpBtn("arrivee")}</h3><div id="inputs"></div>
    <button class="btn small" id="addInput" type="button">+ Ajouter un dossier d'arrivée</button></div>

  <div class="card"><h3 style="margin-top:0">Dossiers de destination ${helpBtn("destinations")}</h3>
    ${CATS.map((c) => `<div class="dest-row"><span>${catName(c)}</span><div class="inline"><input data-dest="${c}" value="${esc(s.destinations[c] || "")}" placeholder="(non utilisé)"><button class="btn small" data-pickdest="${c}" type="button">${t("browse")}</button></div></div>`).join("")}
    <h3>Films déjà présents ailleurs (pour repérer les doublons) ${helpBtn("doublons")}</h3><div id="filmlibs"></div>
    <button class="btn small" id="addFilmLib" type="button">+ Ajouter un dossier de films</button></div>

  <div class="card"><h3 style="margin-top:0">Règles personnelles ${helpBtn("regles")}</h3>
    <p class="muted" style="margin-top:0">Exemple : « Pékin Express » → télé / téléréalité. Une règle l'emporte sur la devinette de TMDB.</p><div id="rules"></div>
    <button class="btn small" id="addRule" type="button">+ Ajouter une règle</button></div>

  <div class="card"><h3 style="margin-top:0">Noms des fichiers ${helpBtn("nommage")}</h3>
    <label class="field"><span>Que mettre dans le nom des fichiers rangés ?</span><select id="n_preset">
      ${[["keep", "Garder les noms d'origine (par défaut)"], ["custom", "Je choisis : année, codec (H.265, AV1…), résolution… (cases à cocher)"], ["simple", "Simple : Titre (année) · Série - S01E01 - Titre de l'épisode"], ["plex", "Style Plex : un dossier par film, dossiers de saison"]].map(([k, n]) => `<option value="${k}" ${s.naming.preset === k ? "selected" : ""}>${n}</option>`).join("")}</select></label>
    <p class="muted" id="n_hint" style="margin:4px 0 8px" ${s.naming.preset === "custom" ? "hidden" : ""}>Pour ajouter le codec (H.265, AV1…) ou la résolution au nom, choisissez « Je choisis » : des cases à cocher apparaissent.</p>
    <div id="n_custom" ${s.naming.preset === "custom" ? "" : "hidden"}>
      <div class="card" style="background:var(--bg)"><b>Cochez ce que vous voulez dans le nom</b>
        <div class="checks">${[["y", "Année"], ["vf", "Résolution (1080p)"], ["source", "Source (WEB, BluRay)"], ["vc", "Codec vidéo (H.265)"], ["audio", "Audio (EAC3, DTS)"], ["hdr", "HDR"], ["lang", "Langue"], ["group", "Groupe"], ["t", "Titre de l'épisode"], ["folder", "Un dossier par film"], ["season", "Dossiers de saison"]]
          .map(([k, n]) => `<label class="chk"><input type="checkbox" data-nb="${k}"> ${n}</label>`).join("")}</div>
        <p class="muted" style="margin:6px 0 0">Ce qui n'est pas trouvé dans le nom d'origine est simplement omis.</p></div>
      <details><summary class="muted" style="cursor:pointer;margin:8px 0">Formats avancés</summary>
      <div class="two"><label class="field"><span>Film</span><input id="n_movie" class="mono" value="${esc(s.naming.movie)}"></label>
        <label class="field"><span>Épisode</span><input id="n_episode" class="mono" value="${esc(s.naming.episode)}"></label></div>
      <div class="two"><label class="field"><span>Dossier de saison (« auto » = style déjà présent ; vide = aucun)</span><input id="n_season" class="mono" value="${esc(s.naming.season_folder)}"></label>
        <label class="field"><span>Nouveau dossier de série</span><input id="n_series" class="mono" value="${esc(s.naming.series_folder)}"></label></div>
      <p class="muted" style="margin-top:0">Variables : <code>{n}</code> titre, <code>{y}</code> année, <code>{ny}</code> titre (année), <code>{s}</code> <code>{s00}</code> saison, <code>{e}</code> <code>{e00}</code> épisode, <code>{s00e00}</code> S01E01, <code>{sxe}</code> 1x01, <code>{t}</code> titre de l'épisode, <code>{vf}</code> 1080p, <code>{source}</code>, <code>{vc}</code> codec vidéo, <code>{audio}</code>, <code>{hdr}</code>, <code>{lang}</code>, <code>{group}</code>. Un « / » crée un dossier.</p></details></div>
    <div id="n_prev" class="card" style="background:var(--bg);margin:0"></div></div>

  <div class="card"><h3 style="margin-top:0">Fonctionnement ${helpBtn("fonctionnement")}</h3>
    <label class="field" style="display:flex;gap:10px;align-items:flex-start"><input id="s_essai" type="checkbox" style="width:auto;min-height:0;margin-top:4px" ${s.essai ? "checked" : ""}>
      <span style="display:block"><b>Mode essai à blanc</b> ${helpBtn("essai")}<br><span class="muted" style="font-weight:400">Coché : l'appli analyse et propose, mais n'écrit rien. Décochez seulement quand vous avez validé le résultat.</span></span></label>
    <div class="two"><label class="field"><span>Méthode de copie ${helpBtn("methode")}</span><select id="s_method">
      ${[["auto", "Automatique (reflink, puis lien physique, puis copie)"], ["reflink", "Reflink seulement"], ["hardlink", "Lien physique seulement"], ["copy", "Copie normale seulement"]].map(([k, n]) => `<option value="${k}" ${s.copy_method === k ? "selected" : ""}>${n}</option>`).join("")}</select></label>
      <label class="field"><span>Analyse automatique toutes les (min) ${helpBtn("auto")}</span><input id="s_interval" type="number" min="0" value="${s.scan_interval_min}"></label></div>
    <label class="field" style="display:flex;gap:10px;align-items:center"><input id="s_auto" type="checkbox" style="width:auto;min-height:0" ${s.auto_range ? "checked" : ""}><span>Ranger seul ce qui est sûr après chaque analyse (hors essai à blanc)</span></label></div>

  <div class="card"><h3 style="margin-top:0">Plex, Jellyfin, Home Assistant ${helpBtn("services")}</h3>
    <div class="banner info"><span>ℹ️</span><span>Après un rangement réel, Plex et Jellyfin sont prévenus pour rafraîchir leur bibliothèque. « Tester » ne fait que lire, rien n'est modifié chez eux.</span></div>
    <div class="two"><label class="field"><span>Adresse de Plex</span><input id="s_plexurl" value="${esc(s.plex_url)}" placeholder="http://192.168.1.10:32400"></label>
      <label class="field"><span>Jeton Plex ${secTag(s.secrets.plex)}</span><input id="s_plextok" type="password" autocomplete="off" placeholder="vide = inchangé"></label></div>
    <div class="two"><label class="field"><span>Adresse de Jellyfin</span><input id="s_jfurl" value="${esc(s.jellyfin_url)}" placeholder="http://192.168.1.10:8096"></label>
      <label class="field"><span>Clé API Jellyfin ${secTag(s.secrets.jellyfin)}</span><input id="s_jftok" type="password" autocomplete="off" placeholder="vide = inchangée"></label></div>
    <div class="two"><label class="field"><span>Adresse de Home Assistant</span><input id="s_haurl" value="${esc(s.ha_url)}" placeholder="http://192.168.1.20:8123"></label>
      <label class="field"><span>Jeton Home Assistant ${secTag(s.secrets.ha)}</span><input id="s_hatok" type="password" autocomplete="off" placeholder="vide = inchangé"></label></div>
    <label class="field"><span>Service de notification Home Assistant (facultatif)</span><input id="s_haserv" value="${esc(s.ha_service)}" placeholder="notify.mobile_app_mon_iphone"></label>
    <button class="btn" id="b_svctest" type="button">Tester les services</button> <span id="svcres" class="muted"></span></div>
  <div class="save-bar"><span class="muted" id="saveInfo"></span><button class="btn primary" id="saveBtn" type="button">${t("save")}</button></div>`;
  drawInputs(); drawFilmLibs(); drawRules();
  // cases à cocher ⇄ formats (les formats restent modifiables à la main dans « Formats avancés »)
  const nbOn = (k) => { const e = document.querySelector(`[data-nb="${k}"]`); return !!(e && e.checked); };
  const nbInit = () => {
    const mv = $("#n_movie").value, ep = $("#n_episode").value, all = mv + " " + ep;
    const set = (k, v) => { const e = document.querySelector(`[data-nb="${k}"]`); if (e) e.checked = v; };
    ["y", "vf", "source", "vc", "audio", "hdr", "lang", "group"].forEach((k) => set(k, all.includes("{" + k + "}") || (k === "y" && all.includes("{ny}"))));
    set("t", ep.includes("{t}")); set("folder", mv.includes("/")); set("season", $("#n_season").value.trim() !== "");
  };
  const nbBuild = () => {
    const tech = ["vf", "source", "vc", "audio", "hdr", "lang", "group"].filter(nbOn).map((k) => `{${k}}`).join(" ");
    const base = nbOn("y") ? "{n} ({y})" : "{n}";
    const film = base + (tech ? " - " + tech : "");
    $("#n_movie").value = nbOn("folder") ? `${base}/${film}` : film;
    $("#n_episode").value = "{n} - {s00e00}" + (nbOn("t") ? " - {t}" : "") + (tech ? " - " + tech : "");
    $("#n_season").value = nbOn("season") ? "auto" : "";
    $("#n_series").value = base;
    ["n_movie", "n_episode", "n_season", "n_series"].forEach((id) => $("#" + id).dispatchEvent(new Event("input")));
  };
  nbInit();
  document.querySelectorAll("[data-nb]").forEach((e) => e.addEventListener("change", nbBuild));
  const namingNow = () => ({ preset: $("#n_preset").value, movie: $("#n_movie").value, episode: $("#n_episode").value, season_folder: $("#n_season").value, series_folder: $("#n_series").value });
  let npT; const nPrev = async () => {
    $("#n_custom").hidden = $("#n_preset").value !== "custom"; $("#n_hint").hidden = $("#n_preset").value === "custom";
    try {
      const r = await api("/api/naming/preview", { method: "POST", body: { naming: namingNow() } });
      $("#n_prev").innerHTML = r.garde ? `<span class="muted">Les fichiers gardent le nom de la release.</span>`
        : r.ok ? `<b>Aperçu</b><div class="mono" style="margin-top:6px">🎬 ${esc(r.film)}</div><div class="mono" style="margin-top:4px">📺 ${esc(r.serie)}</div>`
        : `<span style="color:var(--bad)">⚠️ ${esc(r.erreur)}</span>`;
    } catch (e) { $("#n_prev").textContent = e.message; }
  };
  ["n_preset", "n_movie", "n_episode", "n_season", "n_series"].forEach((id) => $("#" + id).addEventListener("input", () => { clearTimeout(npT); npT = setTimeout(nPrev, 200); }));
  nPrev();
  $("#addInput").onclick = () => { collect(); s.inputs.push({ path: "", mode: "auto", action: "copy", settle_minutes: 30 }); drawInputs(); };
  $("#addFilmLib").onclick = () => { collect(); s.film_libraries.push(""); drawFilmLibs(); };
  $("#addRule").onclick = () => { collect(); s.rules.push({ title: "", category: "telerealite" }); drawRules(); };
  v.querySelectorAll("[data-pickdest]").forEach((b) => b.onclick = async () => {
    const inp = v.querySelector(`[data-dest="${b.dataset.pickdest}"]`); const p = await pickFolder(inp.value, catName(b.dataset.pickdest)); if (p) inp.value = p;
  });
  $("#b_tmdbtest").onclick = async () => {
    const key = $("#s_tmdb").value.trim();
    if (key) { await save(true); }
    const r = await api("/api/tmdb/test", { method: "POST" }); toast(r.message);
  };
  $("#saveBtn").onclick = () => save(false);
  $("#b_svctest").onclick = async () => {
    await save(true); $("#svcres").textContent = "Test en cours…";
    const r = await api("/api/services/test", { method: "POST" });
    const f = (n, x) => `${n} : ${x.ok === null ? "non configuré" : (x.ok ? "✓ " : "✗ ") + x.message}`;
    $("#svcres").innerHTML = [f("Plex", r.plex), f("Jellyfin", r.jellyfin), f("Home Assistant", r.ha)].map(esc).join("<br>");
  };
}
function drawInputs() {
  const s = S.settings;
  $("#inputs").innerHTML = s.inputs.length ? s.inputs.map((i, n) => `<div class="input-block" data-n="${n}">
    <label class="field"><span>Dossier</span><div class="inline"><input data-k="path" value="${esc(i.path)}"><button class="btn small" data-b="path" type="button">${t("browse")}</button><button class="btn small danger" data-b="del" type="button">${t("remove")}</button></div></label>
    <div class="three"><label class="field"><span>Mode ${helpBtn("modes")}</span><select data-k="mode"><option value="auto">${t("mode_auto")}</option>${CATS.map((c) => `<option value="${c}" ${i.mode === c ? "selected" : ""}>Tout va dans : ${catName(c)}</option>`).join("")}</select></label>
      <label class="field"><span>Action ${helpBtn("action")}</span><select data-k="action"><option value="copy" ${i.action === "copy" ? "selected" : ""}>${t("act_copy")}</option><option value="move" ${i.action === "move" ? "selected" : ""}>${t("act_move")}</option></select></label>
      <label class="field"><span>Attendre avant de ranger (min) ${helpBtn("calme")}</span><input data-k="settle_minutes" type="number" min="0" value="${i.settle_minutes}"></label></div></div>`).join("") : `<p class="muted">Aucun dossier d'arrivée.</p>`;
  $("#inputs").querySelectorAll(".input-block").forEach((blk) => {
    const n = +blk.dataset.n;
    blk.querySelector('[data-b="path"]').onclick = async () => { const el = blk.querySelector('[data-k="path"]'); const p = await pickFolder(el.value, "Dossier d'arrivée"); if (p) el.value = p; };
    blk.querySelector('[data-b="del"]').onclick = () => { collect(); s.inputs.splice(n, 1); drawInputs(); };
  });
}
function drawFilmLibs() {
  const s = S.settings;
  $("#filmlibs").innerHTML = s.film_libraries.map((p, n) => `<div class="inline" style="margin-bottom:6px" data-n="${n}"><input data-k="film" value="${esc(p)}"><button class="btn small" data-b="pick" type="button">${t("browse")}</button><button class="btn small danger" data-b="del" type="button">${t("remove")}</button></div>`).join("");
  $("#filmlibs").querySelectorAll("[data-n]").forEach((row) => {
    const n = +row.dataset.n;
    row.querySelector('[data-b="pick"]').onclick = async () => { const el = row.querySelector("input"); const p = await pickFolder(el.value, "Dossier de films"); if (p) el.value = p; };
    row.querySelector('[data-b="del"]').onclick = () => { collect(); s.film_libraries.splice(n, 1); drawFilmLibs(); };
  });
}
function drawRules() {
  const s = S.settings;
  $("#rules").innerHTML = s.rules.map((r, n) => `<div class="inline" style="margin-bottom:6px" data-n="${n}"><input data-k="rt" value="${esc(r.title)}" placeholder="Titre (ex. Pékin Express)"><select data-k="rc" style="max-width:200px">${CATS.map((c) => `<option value="${c}" ${r.category === c ? "selected" : ""}>${catName(c)}</option>`).join("")}</select><button class="btn small danger" data-b="del" type="button">${t("remove")}</button></div>`).join("");
  $("#rules").querySelectorAll("[data-n]").forEach((row) => { row.querySelector('[data-b="del"]').onclick = () => { collect(); s.rules.splice(+row.dataset.n, 1); drawRules(); }; });
}
function collect() {
  const s = S.settings;
  s.language = $("#s_lang").value;
  s.inputs = [...document.querySelectorAll("#inputs .input-block")].map((b) => ({
    path: b.querySelector('[data-k="path"]').value.trim(), mode: b.querySelector('[data-k="mode"]').value,
    action: b.querySelector('[data-k="action"]').value, settle_minutes: Math.max(0, +b.querySelector('[data-k="settle_minutes"]').value || 0) }));
  s.destinations = {};
  document.querySelectorAll("[data-dest]").forEach((i) => { if (i.value.trim()) s.destinations[i.dataset.dest] = i.value.trim(); });
  s.film_libraries = [...document.querySelectorAll('#filmlibs [data-k="film"]')].map((i) => i.value.trim()).filter(Boolean);
  s.rules = [...document.querySelectorAll("#rules [data-n]")].map((r) => ({ title: r.querySelector('[data-k="rt"]').value.trim(), category: r.querySelector('[data-k="rc"]').value })).filter((r) => r.title);
}
async function save(quiet) {
  collect(); const s = S.settings;
  const body = { language: s.language, inputs: s.inputs.filter((i) => i.path), destinations: s.destinations, film_libraries: s.film_libraries, rules: s.rules,
    plex_url: $("#s_plexurl").value, jellyfin_url: $("#s_jfurl").value, ha_url: $("#s_haurl").value, ha_service: $("#s_haserv").value,
    naming: { preset: $("#n_preset").value, movie: $("#n_movie").value, episode: $("#n_episode").value, season_folder: $("#n_season").value, series_folder: $("#n_series").value },
    essai: $("#s_essai").checked, copy_method: $("#s_method").value, scan_interval_min: Math.max(0, +$("#s_interval").value || 0), auto_range: $("#s_auto").checked,
    tmdb_key: $("#s_tmdb").value, plex_token: $("#s_plextok").value, jellyfin_key: $("#s_jftok").value, ha_token: $("#s_hatok").value };
  if (S.status && S.status.essai && !body.essai &&
      !confirm("Vous allez DÉSACTIVER l'essai à blanc.\n\nÀ partir de maintenant, « Ranger » copiera réellement dans votre bibliothèque (chaque rangement reste annulable depuis l'Historique).\n\nContinuer ?")) { $("#s_essai").checked = true; return; }
  try { const res = await api("/api/settings", { method: "POST", body }); if (res.jetons_effaces && res.jetons_effaces.length) toast("Adresse modifiée : saisissez à nouveau le jeton de " + res.jetons_effaces.join(", "));  if (!quiet) toast(t("saved")); invalidate("accueil", "plan", "trier", "historique", "bibliotheque"); await refreshStatus(); if (!quiet) vSettings(pane("reglages")); }
  catch (e) { toast("Erreur : " + e.message); }
}

// ---------- Journal / À propos ----------
async function vLog(v) {
  const l = (await api("/api/log")).lignes;
  v.innerHTML = `<h2>Journal</h2><p class="lead">Ce que fait l'appli, en détail (pour comprendre un souci).</p><div class="card log">${l.length ? l.map((x) => `<div class="${x.niveau}">${new Date(x.t * 1000).toLocaleTimeString()} · ${esc(x.msg)}</div>`).join("") : "<span class='muted'>Vide.</span>"}</div>`;
}
function vAbout(v) {
  v.innerHTML = `<h2>À propos</h2><div class="card"><b>kesal-rangeur</b> ${S.status ? "v" + esc(S.status.version) : ""}<p>Range automatiquement ce que vous téléchargez (films, séries, dessins animés, émissions, documentaires, spectacles) au bon endroit de votre bibliothèque, avec les titres de TMDB dans votre langue. Logiciel libre.</p></div>
  <div class="card"><a href="https://www.themoviedb.org" target="_blank" rel="noopener"><img class="tmdb-about" src="/tmdb-logo.svg" alt="TMDB"></a><p>This application uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by TMDB.<br><i>Cette application utilise TMDB et les API TMDB, mais n'est ni approuvée, ni certifiée, ni validée par TMDB.</i></p>
  <p class="muted">Les titres, résumés et affiches viennent de <a href="https://www.themoviedb.org" target="_blank" rel="noopener">themoviedb.org</a>. L'API TMDB est gratuite pour un usage non commercial : cette appli doit rester gratuite et ne pas servir à faire de l'argent.</p></div>`;
}

// ---------- explorateur de dossiers ----------
function pickFolder(start, title) {
  return new Promise((resolve) => {
    const dlg = $("#dlgFolder"); let cur = start || "/"; let done = false;
    $("#folderTitle").textContent = title || "Choisir un dossier"; $("#folderOk").textContent = "Choisir ce dossier";
    const finish = (v) => { if (!done) { done = true; resolve(v); } };
    async function go(p) {
      try {
        const r = await api("/api/fs?path=" + encodeURIComponent(p)); cur = r.path;
        $("#folderInput").value = cur; $("#folderInfo").textContent = `${r.dirs.length} dossier(s), ${r.videos} vidéo(s) ici`;
        $("#folderList").innerHTML = (r.parent ? `<li data-p="${esc(r.parent)}">⬆️ ..</li>` : "") + r.dirs.map((d) => `<li data-p="${esc(cur.replace(/\/$/, "") + "/" + d)}">📁 ${esc(d)}</li>`).join("");
        $("#folderList").querySelectorAll("li").forEach((li) => li.onclick = () => go(li.dataset.p));
      } catch (e) { $("#folderInfo").textContent = "Dossier illisible"; }
    }
    $("#folderGo").onclick = () => go($("#folderInput").value);
    $("#folderInput").onkeydown = (e) => { if (e.key === "Enter") go($("#folderInput").value); };
    $("#folderOk").onclick = () => { finish($("#folderInput").value); dlg.close(); };
    dlg.addEventListener("close", () => finish(null), { once: true });
    dlg.showModal(); go(cur);
  });
}

// ---------- aide ----------
let helpLoaded = false;
async function openHelp(anchor) {
  $("#helpTitle").textContent = t("help_title");
  if (!helpLoaded) {
    const html = await (await fetch("/api/help?lang=" + LANG)).text();
    $("#helpBody").innerHTML = html;
    const toc = [...$("#helpBody").querySelectorAll("h3[id]")];
    $("#helpToc").innerHTML = toc.map((h) => `<a href="#" data-a="${h.id}">${esc(h.textContent)}</a>`).join("");
    $("#helpToc").querySelectorAll("a").forEach((a) => a.onclick = (e) => { e.preventDefault(); scrollHelp(a.dataset.a); });
    helpLoaded = true;
  }
  if (!$("#dlgHelp").open) $("#dlgHelp").showModal();
  scrollHelp(anchor || "debut");
}
function scrollHelp(id) { const el = $("#helpBody").querySelector("#" + CSS.escape(id)); if (el) $("#helpBody").scrollTo({ top: el.offsetTop - 8 }); }

Object.assign(VIEWS, { accueil: vHome, simulation: vSim, plan: vPlan, trier: vSort, historique: vHistory, bibliotheque: vLibrary, reglages: vSettings, journal: vLog, apropos: vAbout });

// ---------- démarrage ----------
// Bulle d'aide des « ? » : au survol, au clavier ou au toucher ; aucun changement de page.
const tipEl = document.createElement("div"); tipEl.id = "tip"; tipEl.setAttribute("role", "tooltip"); tipEl.hidden = true; document.body.appendChild(tipEl);
let tipPin = null;
function showTip(btn, pinned) {
  const a = btn.dataset.tipFor; tipEl.innerHTML = esc(TIPS[a] || "Pas d'aide pour ce réglage.") + `<a href="#" data-help="${esc(a)}" class="tip-more">En savoir plus</a>`;
  tipEl.hidden = false; const r = btn.getBoundingClientRect(), tw = tipEl.offsetWidth, th = tipEl.offsetHeight;
  let x = Math.min(Math.max(8, r.left + r.width / 2 - tw / 2), window.innerWidth - tw - 8), y = r.bottom + 8;
  if (y + th > window.innerHeight - 8) y = Math.max(8, r.top - th - 8);
  tipEl.style.left = x + "px"; tipEl.style.top = y + "px"; tipPin = pinned ? btn : null;
}
function hideTip() { tipEl.hidden = true; tipPin = null; }
document.addEventListener("mouseover", (e) => { const b = e.target.closest && e.target.closest("[data-tip-for]"); if (b && !tipPin) showTip(b, false); else if (!b && !tipPin && !e.target.closest("#tip")) hideTip(); });
document.addEventListener("focusin", (e) => { const b = e.target.closest && e.target.closest("[data-tip-for]"); if (b) showTip(b, false); });
document.addEventListener("focusout", () => { if (!tipPin) hideTip(); });
document.addEventListener("keydown", (e) => { if (e.key === "Escape") hideTip(); });
document.addEventListener("scroll", () => { if (!tipEl.hidden && !tipPin) hideTip(); }, true);
document.addEventListener("click", (e) => {
  const tb = e.target.closest("[data-tip-for]");
  if (tb) { e.preventDefault(); if (tipPin === tb) hideTip(); else showTip(tb, true); return; }
  if (tipPin && !e.target.closest("#tip")) hideTip();
  const more = e.target.closest(".tip-more"); if (more) { hideTip(); }
});
document.addEventListener("click", (e) => {
  const h = e.target.closest("[data-help]"); if (h) { e.preventDefault(); openHelp(h.dataset.help); }
  const c = e.target.closest("[data-close]"); if (c) c.closest("dialog").close();
});
$("#btnHelp").onclick = () => openHelp("debut");
$("#modeSw").addEventListener("click", (e) => { const b = e.target.closest("[data-mode]"); if (b) setMode(b.dataset.mode); });
$("#btnScan").onclick = async () => {
  try { await api("/api/scan", { method: "POST" }); wasScanning = true; await refreshStatus(); } catch (e) { toast("Erreur : " + e.message); }
};
window.addEventListener("hashchange", route);
(async () => { await refreshStatus(); await loadPlan(); route(); setInterval(tick, 2000); setTimeout(prewarm, 400); })();
