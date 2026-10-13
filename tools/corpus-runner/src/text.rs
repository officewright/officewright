//! Text comparison: normalisation and the Levenshtein similarity of `text_score`.

/// Text as both sides are compared: ligatures decomposed, soft hyphens removed, curly
/// quotes made straight, white space collapsed. Both sides come out of the same pdftotext,
/// so no Unicode normalisation is needed beyond that.
pub fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\u{AD}' => {}
            '\u{FB00}' => out.push_str("ff"),
            '\u{FB01}' => out.push_str("fi"),
            '\u{FB02}' => out.push_str("fl"),
            '\u{FB03}' => out.push_str("ffi"),
            '\u{FB04}' => out.push_str("ffl"),
            '\u{FB05}' | '\u{FB06}' => out.push_str("st"),
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => out.push('\''),
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => out.push('"'),
            other => out.push(other),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 1 − Levenshtein distance / the longer length, over characters; 1 for two empty texts.
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let longer = a.len().max(b.len());
    if longer == 0 {
        return 1.0;
    }
    1.0 - levenshtein(&a, &b) as f64 / longer as f64
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        if let Some(first) = current.first_mut() {
            *first = i + 1;
        }
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous.get(j).copied().unwrap_or(usize::MAX) + usize::from(ca != cb);
            let delete = previous.get(j + 1).copied().unwrap_or(usize::MAX) + 1;
            let insert = current.get(j).copied().unwrap_or(usize::MAX) + 1;
            if let Some(slot) = current.get_mut(j + 1) {
                *slot = substitute.min(delete).min(insert);
            }
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous.last().copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation() {
        assert_eq!(
            normalise("  o\u{FB03}ce  \u{201C}co\u{AD}operate\u{201D}\n\tit\u{2019}s "),
            "office \"cooperate\" it's"
        );
    }

    #[test]
    fn levenshtein_distances() {
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(levenshtein(&chars("kitten"), &chars("sitting")), 3);
        assert_eq!(levenshtein(&chars(""), &chars("abc")), 3);
        assert_eq!(levenshtein(&chars("flaw"), &chars("lawn")), 2);
        assert_eq!(levenshtein(&chars("день"), &chars("дань")), 1);
    }

    #[test]
    fn similarity_is_relative_to_the_longer_text() {
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity("abcd", ""), 0.0);
        assert!((similarity("kitten", "sitting") - (1.0 - 3.0 / 7.0)).abs() < 1e-12);
    }
}
