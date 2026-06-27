//! Lexical (BM25) full-text search over the baked-in Sema documentation corpus.
//!
//! Powers the `docs_search` MCP tool. It is deliberately self-contained: no LLM, no
//! network, no disk I/O at query time. The corpus is [`sema_docs::builtin_index`], which is
//! compiled into the binary via `include_str!`, and the inverted index is built once in
//! memory on first query behind a [`OnceLock`]. This is what lets `docs_search` run from a
//! bare release binary inside a `FROM scratch` container with nothing else present.
//!
//! Ranking is BM25 over the entry `name` (double-weighted) plus `module`, `section`,
//! `summary`, and `body`, with conservative boosts for queries that *literally* name a
//! symbol. The parameters and the small synonym table were validated against a 68-query
//! oracle (recall@5 ≈ 0.93); see `docs/plans/2026-06-25-mcp-docs-search.md`.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use sema_docs::{builtin_index, DocEntry};
use serde::Serialize;

/// BM25 term-frequency saturation parameter.
const K1: f64 = 1.2;
/// BM25 length-normalization parameter.
const B: f64 = 0.75;
/// Weight multiplier for term occurrences in an entry's name (and aliases).
const NAME_WEIGHT: f64 = 2.0;

/// Added when a query token is exactly an entry's full name (e.g. query "map" → `map`).
const EXACT_NAME_BOOST: f64 = 8.0;
/// Added when a query token equals the last segment of a namespaced name and is long
/// enough to be unambiguous (e.g. "split" → `string/split`). Short segments are skipped
/// because they pull in false friends.
const NAMESPACE_SEGMENT_BOOST: f64 = 3.0;
/// Minimum length for a namespaced last-segment match to count.
const NAMESPACE_SEGMENT_MIN: usize = 4;
/// Added when a query token names the entry's module (e.g. "string" → all `string/*`).
const MODULE_MENTION_BOOST: f64 = 1.5;

/// Default number of results returned when the caller omits `limit`.
pub const DEFAULT_LIMIT: usize = 5;
/// Upper bound on `limit` to keep payloads sane.
const MAX_LIMIT: usize = 25;

/// Hand-curated synonym expansions for the vocabulary gaps pure lexical search cannot
/// bridge (validated misses from the oracle). Each query token equal to `.0` adds the
/// tokens in `.1` as extra query terms for scoring (never for the literal-name boosts).
/// Kept intentionally tiny and conservative — broad synonyms regress precision.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("match", &["satisfy"]),
    ("matches", &["satisfy"]),
    ("matching", &["satisfy"]),
    ("star", &["*", "let*"]),
    ("memoize", &["cache"]),
    ("memoise", &["cache"]),
    ("comprehension", &["map", "filter"]),
    ("interpolate", &["format"]),
    ("interpolation", &["format"]),
    ("fstring", &["format"]),
];

/// One search result, serialized as JSON for the MCP tool payload.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchHit {
    pub name: String,
    pub module: String,
    pub summary: String,
    pub score: f64,
}

/// An in-memory BM25 index over the documentation corpus.
struct SearchIndex {
    entries: Vec<DocEntry>,
    /// term → postings list of (document index, weighted term frequency).
    postings: HashMap<String, Vec<(usize, f64)>>,
    /// Weighted length of each document (sum of weighted term frequencies).
    doc_len: Vec<f64>,
    avgdl: f64,
    n_docs: usize,
}

static INDEX: OnceLock<SearchIndex> = OnceLock::new();

fn index() -> &'static SearchIndex {
    INDEX.get_or_init(|| SearchIndex::build(builtin_index().entries.clone()))
}

/// Search the documentation corpus, returning up to `limit` ranked hits (clamped to
/// `1..=MAX_LIMIT`). `limit == 0` falls back to [`DEFAULT_LIMIT`].
pub fn search(query: &str, limit: usize) -> Vec<SearchHit> {
    let limit = match limit {
        0 => DEFAULT_LIMIT,
        n => n.min(MAX_LIMIT),
    };
    index().search(query, limit)
}

/// Tokenize text into lowercased terms. Each whitespace chunk yields its verbatim token
/// (after stripping surrounding sentence punctuation) so operator/predicate names like
/// `*`, `<=`, `null?` survive intact, plus its alphanumeric subwords (len ≥ 2) so
/// namespaced names like `string/upper` also match `string` and `upper`.
fn tokenize(text: &str) -> Vec<String> {
    const TRIM: &[char] = &[
        '.', ',', ';', ':', '(', ')', '[', ']', '{', '}', '"', '\'', '`',
    ];
    let mut out = Vec::new();
    for chunk in text.split_whitespace() {
        let word = chunk.trim_matches(TRIM).to_lowercase();
        if word.is_empty() {
            continue;
        }
        out.push(word.clone());
        if word.chars().any(|c| !c.is_alphanumeric()) {
            for sub in word.split(|c: char| !c.is_alphanumeric()) {
                if sub.chars().count() >= 2 {
                    out.push(sub.to_string());
                }
            }
        }
    }
    out
}

/// Expand query tokens with the synonym table, de-duplicated. Originals are always kept.
fn expand_query(tokens: &[String]) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut terms: Vec<String> = Vec::new();
    for t in tokens {
        if seen.insert(t.as_str()) {
            terms.push(t.clone());
        }
    }
    for t in tokens {
        for (key, exps) in SYNONYMS {
            if t == key {
                for e in *exps {
                    if seen.insert(e) {
                        terms.push(e.to_string());
                    }
                }
            }
        }
    }
    terms
}

impl SearchIndex {
    fn build(entries: Vec<DocEntry>) -> SearchIndex {
        let n_docs = entries.len();
        let mut postings: HashMap<String, Vec<(usize, f64)>> = HashMap::new();
        let mut doc_len = vec![0.0; n_docs];

        for (i, e) in entries.iter().enumerate() {
            let mut tf: HashMap<String, f64> = HashMap::new();
            let add = |text: &str, weight: f64, tf: &mut HashMap<String, f64>| {
                for term in tokenize(text) {
                    *tf.entry(term).or_insert(0.0) += weight;
                }
            };
            add(&e.name, NAME_WEIGHT, &mut tf);
            for alias in &e.aliases {
                add(alias, NAME_WEIGHT, &mut tf);
            }
            add(&e.module, 1.0, &mut tf);
            if let Some(section) = &e.section {
                add(section, 1.0, &mut tf);
            }
            add(&e.summary, 1.0, &mut tf);
            add(&e.body, 1.0, &mut tf);

            doc_len[i] = tf.values().sum();
            for (term, freq) in tf {
                postings.entry(term).or_default().push((i, freq));
            }
        }

        let total_len: f64 = doc_len.iter().sum();
        let avgdl = if n_docs == 0 {
            0.0
        } else {
            total_len / n_docs as f64
        };

        SearchIndex {
            entries,
            postings,
            doc_len,
            avgdl,
            n_docs,
        }
    }

    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let q_tokens = tokenize(query);
        if q_tokens.is_empty() || self.n_docs == 0 {
            return Vec::new();
        }
        let terms = expand_query(&q_tokens);

        let mut scores: HashMap<usize, f64> = HashMap::new();
        for term in &terms {
            let Some(postings) = self.postings.get(term) else {
                continue;
            };
            let df = postings.len() as f64;
            // BM25 idf with the +1 robustness term (always non-negative).
            let idf = ((self.n_docs as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();
            for &(doc, freq) in postings {
                let denom = freq + K1 * (1.0 - B + B * self.doc_len[doc] / self.avgdl);
                *scores.entry(doc).or_insert(0.0) += idf * (freq * (K1 + 1.0)) / denom;
            }
        }

        // Conservative literal-name boosts, keyed on the ORIGINAL query tokens only (not
        // synonym expansions) so the boost never fires on a fuzzy match.
        for (&doc, score) in scores.iter_mut() {
            let e = &self.entries[doc];
            let name = e.name.to_lowercase();
            if q_tokens.contains(&name) {
                *score += EXACT_NAME_BOOST;
            }
            if let Some(seg) = name.rsplit('/').next() {
                if seg != name
                    && seg.chars().count() >= NAMESPACE_SEGMENT_MIN
                    && q_tokens.iter().any(|t| t == seg)
                {
                    *score += NAMESPACE_SEGMENT_BOOST;
                }
            }
            let module = e.module.to_lowercase();
            if q_tokens.contains(&module) {
                *score += MODULE_MENTION_BOOST;
            }
        }

        let mut ranked: Vec<(usize, f64)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        ranked.truncate(limit);

        ranked
            .into_iter()
            .map(|(doc, score)| {
                let e = &self.entries[doc];
                SearchHit {
                    name: e.name.clone(),
                    module: e.module.clone(),
                    summary: e.summary.clone(),
                    score: (score * 1000.0).round() / 1000.0,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(query: &str) -> Vec<String> {
        search(query, 5).into_iter().map(|h| h.name).collect()
    }

    #[test]
    fn tokenize_splits_namespaced_names() {
        let toks = tokenize("string/upper");
        assert!(toks.contains(&"string/upper".to_string()));
        assert!(toks.contains(&"string".to_string()));
        assert!(toks.contains(&"upper".to_string()));
    }

    #[test]
    fn tokenize_keeps_operator_and_predicate_names() {
        assert_eq!(tokenize("*"), vec!["*".to_string()]);
        assert!(tokenize("null?").contains(&"null?".to_string()));
        assert!(tokenize("null?").contains(&"null".to_string()));
        assert!(tokenize("<=").contains(&"<=".to_string()));
    }

    #[test]
    fn tokenize_trims_sentence_punctuation() {
        let toks = tokenize("(reverse a list).");
        assert!(toks.contains(&"reverse".to_string()));
        assert!(toks.contains(&"list".to_string()));
        assert!(!toks.contains(&"list.".to_string()));
    }

    #[test]
    fn exact_name_query_ranks_first() {
        assert_eq!(
            search("map", 5).first().map(|h| h.name.clone()),
            Some("map".to_string())
        );
        assert_eq!(
            search("filter", 5).first().map(|h| h.name.clone()),
            Some("filter".to_string())
        );
    }

    #[test]
    fn descriptive_query_finds_map() {
        // An oracle-validated query (the corpus says "each element", so "apply a function"
        // carries the match, not the verb "transform").
        assert!(names("apply a function to every element of a list").contains(&"map".to_string()));
    }

    #[test]
    fn descriptive_query_finds_reverse() {
        assert!(names("reverse the order of a list").contains(&"reverse".to_string()));
    }

    #[test]
    fn parse_json_finds_a_json_entry() {
        assert!(
            names("parse a json string")
                .iter()
                .any(|n| n.starts_with("json/")),
            "expected a json/* entry in top-5"
        );
    }

    #[test]
    fn synonym_closes_match_predicate_gap() {
        // "match" lexically favors regex/match; the satisfy synonym recovers `filter`.
        assert!(
            names("keep only elements that match a predicate").contains(&"filter".to_string()),
            "expected `filter` in top-5 via synonym expansion"
        );
    }

    #[test]
    fn limit_is_respected_and_clamped() {
        assert!(search("list", 3).len() <= 3);
        assert!(search("list", 0).len() <= DEFAULT_LIMIT);
        assert!(search("list", 1000).len() <= MAX_LIMIT);
    }

    #[test]
    fn empty_query_returns_nothing() {
        assert!(search("   ", 5).is_empty());
    }

    #[test]
    fn expand_query_deduplicates() {
        let tokens = vec!["map".to_string(), "map".to_string(), "filter".to_string()];
        let expanded = expand_query(&tokens);
        // No duplicates.
        let mut seen = std::collections::HashSet::new();
        for t in &expanded {
            assert!(
                seen.insert(t.as_str()),
                "duplicate token `{t}` in expanded query"
            );
        }
    }

    #[test]
    fn expand_query_adds_synonyms() {
        let tokens = vec!["match".to_string()];
        let expanded = expand_query(&tokens);
        // Original is kept.
        assert!(expanded.contains(&"match".to_string()));
        // Synonym is added.
        assert!(expanded.contains(&"satisfy".to_string()));
    }

    #[test]
    fn expand_query_no_duplicates_with_synonyms() {
        // "star" expands to "*" and "let*"; ensure none are duplicated.
        let tokens = vec!["star".to_string(), "*".to_string()];
        let expanded = expand_query(&tokens);
        let star_count = expanded.iter().filter(|t| t.as_str() == "*").count();
        assert_eq!(star_count, 1, "`*` should appear exactly once");
    }
}
