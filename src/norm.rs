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
    // « Rocky II » = « Rocky 2 » : chiffres romains ii…xii → chiffres (jamais en 1er mot, ni « i », « v », « x » seuls)
    const ROMAN: &[(&str, &str)] = &[("ii", "2"), ("iii", "3"), ("iv", "4"), ("vi", "6"), ("vii", "7"), ("viii", "8"), ("ix", "9"), ("xi", "11"), ("xii", "12")];
    for w in words.iter_mut().skip(1) {
        if let Some((_, n)) = ROMAN.iter().find(|(r, _)| *r == w.as_str()) {
            *w = n.to_string();
        }
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

/// Les nombres entiers du titre (« Nordholm 2 » → ["2"]). Deux titres qui n'ont pas les mêmes nombres
/// sont deux œuvres différentes (suites, numéros), même s'ils se ressemblent presque lettre pour lettre.
fn numbers(s: &str) -> Vec<String> {
    // « Rocky III » ≠ « Rocky II » : les chiffres romains ii…xii comptent comme des nombres (pas « i », « v », « x » seuls : ce sont des mots)
    const ROMAN: &[(&str, &str)] = &[("ii", "2"), ("iii", "3"), ("iv", "4"), ("vi", "6"), ("vii", "7"), ("viii", "8"), ("ix", "9"), ("xi", "11"), ("xii", "12")];
    key(s)
        .split(' ')
        .filter(|w| !w.is_empty())
        .filter_map(|w| {
            if w.chars().all(|c| c.is_ascii_digit()) {
                Some(w.to_string())
            } else {
                ROMAN.iter().find(|(r, _)| *r == w).map(|(_, n)| n.to_string())
            }
        })
        .collect()
}

/// Ressemblance entre deux titres, de 0 (rien à voir) à 1 (identiques après normalisation).
pub fn similarity(a: &str, b: &str) -> f32 {
    let s = raw_similarity(a, b);
    if s < 1.0 && numbers(a) != numbers(b) {
        return s.min(0.5);
    }
    s
}

fn raw_similarity(a: &str, b: &str) -> f32 {
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
    fn les_chiffres_comptent() {
        assert!(similarity("Meurtres à Nordholm 2", "Meurtres à Nordholm 1") <= 0.5);
        assert!(similarity("21 Jump Street", "22 Jump Street") <= 0.5);
        assert_eq!(similarity("Paris Police 1905", "paris police 1905"), 1.0);
    }

    #[test]
    fn chiffres_romains() {
        assert!(similarity("The Godfather Part II", "The Godfather Part III") <= 0.5);
        assert!(similarity("Rocky III", "Rocky II") <= 0.5);
        assert!(similarity("Rocky II", "Rocky 2") >= 0.9, "romain et arabe s'équivalent");
        assert_eq!(similarity("Rocky II", "rocky ii"), 1.0);
    }

    #[test]
    fn ressemblance() {
        assert_eq!(similarity("Bureau des légendes", "bureau des legendes"), 1.0);
        assert!(similarity("Shameless", "Shameless US") > 0.7);
        assert!(similarity("GIGN", "Elite Force") < 0.3);
    }
}
