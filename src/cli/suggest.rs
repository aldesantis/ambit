//! "Did you mean …?" for an unknown command or flag, matching commander 15's `suggestSimilar`.

use crate::util::cmp::js_cmp;

/// Suggestions further than this many edits away are not offered.
const MAX_DISTANCE: usize = 3;

/// Below this share of matching characters a candidate is not offered, however close.
const MIN_SIMILARITY: f64 = 0.4;

/// Optimal string alignment distance (Damerau-Levenshtein where no substring is edited more than
/// once), over UTF-16 code units as JavaScript strings index.
fn edit_distance(a: &[u16], b: &[u16]) -> usize {
    // Quick early exit, returning the worst case.
    if a.len().abs_diff(b.len()) > MAX_DISTANCE {
        return a.len().max(b.len());
    }

    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];

    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }

    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }

    for j in 1..=b.len() {
        for i in 1..=a.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);

            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);

            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }

    d[a.len()][b.len()]
}

/// The suggestion suffix for an unknown `word`, starting with a newline, or `""` when no candidate
/// is close enough.
///
/// A `word` starting with `--` is matched as a flag: the dashes are stripped from it and from every
/// candidate before comparing, and put back in the suggestion. Ties are all offered, sorted.
pub fn suggest_similar(word: &str, candidates: &[String]) -> String {
    if candidates.is_empty() {
        return String::new();
    }

    let mut unique: Vec<&str> = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        if !unique.contains(&candidate.as_str()) {
            unique.push(candidate);
        }
    }

    let searching_options = word.starts_with("--");
    let (word, unique): (&str, Vec<&str>) = if searching_options {
        (
            &word[2..],
            unique
                .into_iter()
                .map(|candidate| candidate.get(2..).unwrap_or(""))
                .collect(),
        )
    } else {
        (word, unique)
    };
    let word_units: Vec<u16> = word.encode_utf16().collect();

    let mut similar: Vec<&str> = Vec::new();
    let mut best_distance = MAX_DISTANCE;

    for candidate in unique {
        let candidate_units: Vec<u16> = candidate.encode_utf16().collect();

        // No one-character guesses.
        if candidate_units.len() <= 1 {
            continue;
        }

        let distance = edit_distance(&word_units, &candidate_units);
        let length = word_units.len().max(candidate_units.len());
        #[allow(clippy::cast_precision_loss)]
        let similarity = (length as f64 - distance as f64) / length as f64;

        if similarity > MIN_SIMILARITY {
            if distance < best_distance {
                best_distance = distance;
                similar = vec![candidate];
            } else if distance == best_distance {
                similar.push(candidate);
            }
        }
    }

    // commander sorts with `localeCompare`; candidates are ASCII command and flag names, for which
    // code-unit order gives the same answer.
    similar.sort_by(|a, b| js_cmp(a, b));

    let similar: Vec<String> = similar
        .into_iter()
        .map(|candidate| {
            if searching_options {
                format!("--{candidate}")
            } else {
                candidate.to_owned()
            }
        })
        .collect();

    match similar.len() {
        0 => String::new(),
        1 => format!("\n(Did you mean {}?)", similar[0]),
        _ => format!("\n(Did you mean one of {}?)", similar.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|&name| name.to_owned()).collect()
    }

    #[test]
    fn suggests_the_closest_command() {
        assert_eq!(
            suggest_similar("instal", &names(&["init", "install", "status"])),
            "\n(Did you mean install?)"
        );
    }

    #[test]
    fn suggests_nothing_when_nothing_is_close() {
        assert_eq!(suggest_similar("frob", &names(&["install", "status"])), "");
        assert_eq!(suggest_similar("x", &[]), "");
    }

    #[test]
    fn matches_flags_without_their_dashes() {
        assert_eq!(
            suggest_similar("--dr-run", &names(&["--dry-run", "--json", "--help"])),
            "\n(Did you mean --dry-run?)"
        );
        assert_eq!(
            suggest_similar("--json=1", &names(&["--json", "--help"])),
            "\n(Did you mean --json?)"
        );
    }

    #[test]
    fn offers_every_tie_sorted() {
        assert_eq!(
            suggest_similar("--lnk", &names(&["--link", "--ink", "--lint"])),
            "\n(Did you mean one of --ink, --link?)"
        );
    }

    #[test]
    fn counts_a_transposition_as_one_edit() {
        let a: Vec<u16> = "ab".encode_utf16().collect();
        let b: Vec<u16> = "ba".encode_utf16().collect();

        assert_eq!(edit_distance(&a, &b), 1);
    }

    #[test]
    fn makes_no_one_character_guesses() {
        assert_eq!(suggest_similar("y", &names(&["x"])), "");
    }
}
