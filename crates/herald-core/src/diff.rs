//! A word-level diff between two prose bodies.
//!
//! Herold shows the proposed send body as a diff against the last message in
//! the thread, so the reviewer sees which words the reply introduces and
//! which it carries over. The diff is deterministic (a longest-common-
//! subsequence over whitespace tokens) and side-effect free: it is computed
//! once at queue time and stored beside the pending approval, never re-derived
//! at display time.
//!
//! Token equality is exact (case- and punctuation-sensitive), so rejoining
//! `Eq` + `Add` in order reproduces `after`, and `Eq` + `Del` reproduces
//! `before`, up to whitespace normalisation. Callers cap the input size (see
//! [`MAX_DIFF_WORDS`]) so the quadratic table stays bounded.

use serde::{Deserialize, Serialize};

/// Largest body one side of a diff may have, in words. Above this the diff is
/// skipped entirely: prose reviews never need a 2000-word alignment, and the
/// DP table must stay small.
pub const MAX_DIFF_WORDS: usize = 2_000;

/// One run of words: `Eq` means the words appear in both bodies, `Add` only
/// in the proposed body, `Del` only in the thread message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Eq,
    Add,
    Del,
}

/// A maximal run of same-kind words, joined by single spaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSegment {
    pub kind: DiffKind,
    pub text: String,
}

/// Word-level diff of `before` (a thread message) against `after` (the
/// proposed body). Returns an empty vec when either side is empty or exceeds
/// [`MAX_DIFF_WORDS`]; callers treat that as "no diff available".
#[must_use]
pub fn word_diff(before: &str, after: &str) -> Vec<DiffSegment> {
    let before_words: Vec<&str> = before.split_whitespace().collect();
    let after_words: Vec<&str> = after.split_whitespace().collect();
    if before_words.is_empty()
        || after_words.is_empty()
        || before_words.len() > MAX_DIFF_WORDS
        || after_words.len() > MAX_DIFF_WORDS
    {
        return Vec::new();
    }

    let table = lcs_table(&before_words, &after_words);
    // Backtrack from the end, then reverse the per-word decisions so each
    // segment's words and the segments themselves read left to right.
    let mut decisions: Vec<(&DiffKind, &str)> = Vec::new();
    let (mut i, mut j) = (before_words.len(), after_words.len());
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && before_words[i - 1] == after_words[j - 1] {
            decisions.push((&DiffKind::Eq, after_words[j - 1]));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || table[i][j - 1] >= table[i - 1][j]) {
            // Prefer insertions on ties so the script stays deterministic.
            decisions.push((&DiffKind::Add, after_words[j - 1]));
            j -= 1;
        } else {
            decisions.push((&DiffKind::Del, before_words[i - 1]));
            i -= 1;
        }
    }
    decisions.reverse();

    let mut segments: Vec<DiffSegment> = Vec::new();
    for (kind, word) in decisions {
        if let Some(last) = segments.last_mut()
            && last.kind == *kind
        {
            last.text.push(' ');
            last.text.push_str(word);
            continue;
        }
        segments.push(DiffSegment {
            kind: kind.clone(),
            text: word.to_string(),
        });
    }
    segments
}

/// Longest-common-subsequence lengths: `table[a][b]` is the LCS length of the
/// first `a` before-words and the first `b` after-words.
fn lcs_table(before: &[&str], after: &[&str]) -> Vec<Vec<usize>> {
    let mut table = vec![vec![0usize; after.len() + 1]; before.len() + 1];
    for a in 1..=before.len() {
        for b in 1..=after.len() {
            table[a][b] = if before[a - 1] == after[b - 1] {
                table[a - 1][b - 1] + 1
            } else {
                table[a - 1][b].max(table[a][b - 1])
            };
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(segments: &[DiffSegment], kinds: &[DiffKind]) -> String {
        segments
            .iter()
            .filter(|segment| kinds.contains(&segment.kind))
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn identical_text_is_all_context() {
        let segments = word_diff("thanks for the update", "thanks for the update");
        assert_eq!(
            segments,
            [DiffSegment {
                kind: DiffKind::Eq,
                text: "thanks for the update".to_string()
            }]
        );
    }

    #[test]
    fn changed_words_split_into_add_and_del_segments() {
        let segments = word_diff("thanks for the update", "thanks for the lease");
        assert_eq!(
            segments,
            [
                DiffSegment {
                    kind: DiffKind::Eq,
                    text: "thanks for the".to_string()
                },
                DiffSegment {
                    kind: DiffKind::Del,
                    text: "update".to_string()
                },
                DiffSegment {
                    kind: DiffKind::Add,
                    text: "lease".to_string()
                },
            ]
        );
        // Reconstruction: Eq+Del reproduces the original, Eq+Add the proposal.
        assert_eq!(
            joined(&segments, &[DiffKind::Eq, DiffKind::Del]),
            "thanks for the update"
        );
        assert_eq!(
            joined(&segments, &[DiffKind::Eq, DiffKind::Add]),
            "thanks for the lease"
        );
    }

    #[test]
    fn disjoint_text_is_del_then_add() {
        let segments = word_diff("one two", "three four");
        assert_eq!(
            segments,
            [
                DiffSegment {
                    kind: DiffKind::Del,
                    text: "one two".to_string()
                },
                DiffSegment {
                    kind: DiffKind::Add,
                    text: "three four".to_string()
                },
            ]
        );
    }

    #[test]
    fn prefix_only_changes_stay_minimal() {
        let segments = word_diff("hello world", "hello there world");
        assert_eq!(
            segments,
            [
                DiffSegment {
                    kind: DiffKind::Eq,
                    text: "hello".to_string()
                },
                DiffSegment {
                    kind: DiffKind::Add,
                    text: "there".to_string()
                },
                DiffSegment {
                    kind: DiffKind::Eq,
                    text: "world".to_string()
                },
            ]
        );
    }

    #[test]
    fn empty_or_oversized_inputs_yield_no_diff() {
        assert_eq!(word_diff("", "hello"), Vec::<DiffSegment>::new());
        assert_eq!(word_diff("hello", ""), Vec::<DiffSegment>::new());
        assert_eq!(word_diff("   ", " \t "), Vec::<DiffSegment>::new());
        let huge = "word ".repeat(MAX_DIFF_WORDS + 1);
        assert_eq!(word_diff(&huge, "hello"), Vec::<DiffSegment>::new());
        assert_eq!(word_diff("hello", &huge), Vec::<DiffSegment>::new());
    }

    #[test]
    fn segments_are_serialisable_for_stored_approvals() {
        let segments = word_diff("a b", "a c");
        let value = serde_json::to_value(&segments).unwrap();
        assert_eq!(value[0]["kind"], "eq");
        assert_eq!(value[1]["kind"], "del");
        assert_eq!(value[2]["kind"], "add");
        let back: Vec<DiffSegment> = serde_json::from_value(value).unwrap();
        assert_eq!(back, segments);
    }
}
