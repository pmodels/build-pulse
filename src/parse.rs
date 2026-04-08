//! [Pattern] and [PatternSet] parsing behavior.
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Deref,
};

use arcstr::{ArcStr, Substr};
use regex::{Regex, RegexSet};

use crate::db::{InDatabase, Issue, TagInfo};

/// Set of [Pattern]s
pub struct PatternSet<T> {
    /// [Vec] of underlying [Pattern]s
    patterns: Vec<Pattern<T>>,

    /// [RegexSet] matching [Pattern]s
    pattern_set: RegexSet,
}

/// [Pattern] that can be searched for
pub struct Pattern<T> {
    /// [Regex] pattern to match
    regex: Regex,

    /// Found item
    item: T,
}

impl<T: Hash> Hash for PatternSet<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.patterns.hash(state);
    }
}

impl<T: Hash> Hash for Pattern<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.item.hash(state);
        self.regex.as_str().hash(state);
    }
}

impl<T> Deref for Pattern<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl<T> PatternSet<T> {
    /// Create a new [PatternSet] from a set of [Pattern]s
    pub fn new(patterns: Vec<Pattern<T>>) -> Result<Self, regex::Error> {
        let pattern_set = RegexSet::new(patterns.iter().map(|m| m.regex.as_str()))?;
        Ok(Self {
            patterns,
            pattern_set,
        })
    }

    /// Grep `field` for [Pattern]s
    pub fn grep_matches<S: AsRef<str>>(&self, field: S) -> impl Iterator<Item = &Pattern<T>> {
        // matches using the match set first, then the regex of all valid matches are ran again to find them
        self.pattern_set
            .matches(field.as_ref())
            .into_iter()
            .map(|i| &self.patterns[i])
    }

    /// Get the [PatternSet] schema/hash
    pub fn schema(&self) -> u64
    where
        Self: Hash,
    {
        let mut s = DefaultHasher::new();
        self.hash(&mut s);
        s.finish()
    }
}

impl<T> Pattern<T> {
    /// Create a new pattern
    pub fn new(item: T, regex: Regex) -> Self {
        Self { item, regex }
    }

    /// Grep `field` for substrings
    pub fn grep_substr(&self, field: &ArcStr) -> impl Iterator<Item = Substr> {
        self.regex
            .find_iter(field)
            .map(move |m| field.substr(m.range()))
    }
}

impl Pattern<InDatabase<TagInfo>> {
    /// Grep `field` for [Issue]s
    pub fn grep_issue(&self, field: &ArcStr) -> impl Iterator<Item = Issue> {
        self.grep_substr(&field)
            .fold(HashMap::new(), |mut acc, m| {
                *acc.entry(m).or_default() += 1;
                acc
            })
            .into_iter()
            .map(move |(snippet, duplicates)| Issue {
                snippet,
                tag_id: self.id,
                duplicates,
            })
    }
}

/// Calculate the Levenshtein Distance between two strings
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    // https://en.wikipedia.org/wiki/Levenshtein_distance#Iterative_with_two_matrix_rows
    let b_len = match b.len() {
        0 => return a.len(), // if b is empty, then distance is a
        x => x,
    };
    let mut v1: Vec<usize> = (0..=b_len).collect(); // current row (init with edit distance from "" to b)

    for (i, c1) in a.chars().enumerate() {
        // calculate v1 distances from v0 in-place
        let mut v0 = v1[0]; // save last substitution cost

        // v1[0] is the edit distance from a[..=i] to ""
        v1[0] = i + 1;

        for (j, c2) in b.chars().enumerate() {
            // v1[j + 1] is the character being calculated in a
            // v1[j] is the previous character in a
            let delete_cost = v1[j] + 1; // not including this character is better
            let insert_cost = v1[j + 1] + 1; // keeping this character is better
            let substitution_cost = if c1 == c2 {
                v0 // no change
            } else {
                v0 + 1 // substituting this character is better
            };

            v0 = v1[j + 1]; // save last substitution cost
            v1[j + 1] = delete_cost.min(insert_cost).min(substitution_cost);
        }
    }

    v1[b_len]
}

/// Calculate a normalized [levenshtein_distance] using an exponential decay model
///
/// <https://www.cse.lehigh.edu/%7Elopresti/Publications/1996/sdair96.pdf>
#[inline]
pub fn normalized_levenshtein_distance(a: &str, b: &str) -> f32 {
    let d = levenshtein_distance(a, b) as f32;
    let m = a.len().max(b.len()) as f32;
    (d / (m - d)).exp().recip()
}
