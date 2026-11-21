// Textes de l'interface. Pour ajouter une langue : copier le bloc « fr » sous un autre code (en, es…) et traduire.
const I18N = {
  fr: {
    app: "kesal-rangeur",
    scan: "Analyser maintenant", scanning: "Analyse en cours…",
    tab_home: "Accueil", tab_sim: "Simulation", tab_plan: "Détail", tab_sort: "À trier", tab_history: "Historique",
    tab_library: "Bibliothèque", tab_settings: "Réglages", tab_log: "Journal", tab_about: "À propos",
    pill_dry: "Essai à blanc", pill_busy: "Analyse…",
    dry_banner: "Simulation : l'appli analyse et propose, mais ne copie, ne déplace et ne supprime rien. Pour ranger vraiment, choisissez Manuel (vous validez chaque élément) ou Auto, en haut.",
    cat_film: "Film", cat_serie: "Série", cat_dessin_anime: "Dessin animé", cat_telerealite: "Télé / téléréalité",
    cat_documentaire: "Documentaire", cat_spectacle: "Spectacle",
    d_ranger: "À ranger", d_doublon: "Doublons", d_a_trier: "À trier", d_attendre: "Transfert en cours", d_ignorer: "Ignorés",
    mode_auto: "Automatique (l'appli devine)", act_copy: "Copier (l'original reste)", act_move: "Déplacer",
    save: "Enregistrer", saved: "Réglages enregistrés", cancel: "Annuler", choose: "Choisir", browse: "Parcourir…",
    ok: "OK", test: "Tester", add: "Ajouter", remove: "Retirer",
    help: "Aide", help_title: "Aide et mode d'emploi",
  },
};
let LANG = (localStorage.getItem("lang") || "fr");
if (!I18N[LANG]) LANG = "fr";
function t(k) { return (I18N[LANG] && I18N[LANG][k]) || I18N.fr[k] || k; }
