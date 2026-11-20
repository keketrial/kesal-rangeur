//! Normalisation des titres pour les comparer (accents, casse, articles, ponctuation).

const ARTICLES: &[&str] = &[
    "le", "la", "les", "l", "un", "une", "the", "a", "an", "el", "los", "las", "il", "der", "die", "das",
];

/// Titre normalisé : sans accents, en minuscules, sans ponctuation ni article initial.
pub fn key(s: &str) -> String {
    let ascii = deunicode::deunicode(s).to_lowercase();
    let mut words: Vec<String> = ascii
        .replace(['\'', '’', '`'], " ")
        .replace('&', " and ")
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect();
    if words.len() > 1 && ARTICLES.contains(&words[0].as_str()) {
        words.remove(0);
    }
    words.join(" ")
}

/// Même chose sans espaces : « workingirls » = « working girls ».
pub fn compact(s: &str) -> String {
    key(s).replace(' ', "")
}

fn bigrams(s: &str) -> Vec<(char, char)> {
    let c: Vec<char> = s.chars().collect();
    c.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Ressemblance entre deux titres, de 0 (rien à voir) à 1 (identiques après normalisation).
pub fn similarity(a: &str, b: &str) -> f32 {
    let (ca, cb) = (compact(a), compact(b));
    if ca.is_empty() || cb.is_empty() {
        return 0.0;
    }
    if ca == cb {
        return 1.0;
    }
    let (ba, mut bb) = (bigrams(&ca), bigrams(&cb));
    if ba.is_empty() || bb.is_empty() {
        return 0.0;
    }
    let total = (ba.len() + bb.len()) as f32;
    let mut inter = 0;
    for g in &ba {
        if let Some(p) = bb.iter().position(|x| x == g) {
            bb.swap_remove(p);
            inter += 1;
        }
    }
    2.0 * inter as f32 / total
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cles() {
        assert_eq!(key("L'Été 36"), "ete 36");
        assert_eq!(key("l.ete.36".replace('.', " ").as_str()), "ete 36");
        assert_eq!(key("J'ai menti"), key("J ai Menti"));
        assert_eq!(key("The Walking Dead"), "walking dead");
        assert_eq!(compact("working.girls"), compact("Working Girls"));
        assert!(similarity("workingirls", "Working Girls") > 0.85);
    }
    #[test]
    fn ressemblance() {
        assert_eq!(similarity("Bureau des légendes", "bureau des legendes"), 1.0);
        assert!(similarity("Shameless", "Shameless US") > 0.7);
        assert!(similarity("GIGN", "Elite Force") < 0.3);
    }
}
