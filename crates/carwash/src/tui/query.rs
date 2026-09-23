//! The filter language: fuzzy text plus facets.
//!
//! `turbo eco:rust,node kind:deps size>1g age>30d is:ready` keeps artifacts whose path
//! fuzzy-matches `turbo`, from Rust or Node projects, installed dependencies, freeing more
//! than 1 GB, untouched for 30 days and selectable by default. Unknown tokens are search text.

use carwash_core::select::Hold;
use carwash_core::{ArtifactKind, EcoId, Registry};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFacet {
    Ready,
    Review,
    Recent,
    Protected,
    Marked,
}

#[derive(Debug, Default)]
pub struct Query {
    pub raw: String,
    pattern: Option<Pattern>,
    ecosystems: Vec<EcoId>,
    kinds: Vec<ArtifactKind>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    older_than: Option<Duration>,
    newer_than: Option<Duration>,
    states: Vec<StateFacet>,
}

/// What a query is evaluated against.
pub struct Candidate<'a> {
    pub haystack: &'a str,
    pub ecosystem: Option<EcoId>,
    pub kind: ArtifactKind,
    pub reclaimable: Option<u64>,
    pub age: Option<Duration>,
    pub hold: Option<Hold>,
    pub marked: bool,
}

impl Query {
    pub fn parse(raw: &str, registry: &Registry) -> Self {
        let mut query = Query {
            raw: raw.to_owned(),
            ..Query::default()
        };
        let mut text = Vec::new();
        for token in raw.split_whitespace() {
            if !query.facet(token, registry) {
                text.push(token);
            }
        }
        if !text.is_empty() {
            query.pattern = Some(Pattern::parse(
                &text.join(" "),
                CaseMatching::Smart,
                Normalization::Smart,
            ));
        }
        query
    }

    /// Applies `token` if it is a facet; returns false for plain search text.
    fn facet(&mut self, token: &str, registry: &Registry) -> bool {
        if let Some((key, values)) = token.split_once(':') {
            let values = values.split(',').filter(|v| !v.is_empty());
            match key {
                "eco" | "lang" | "e" => {
                    let ecos: Vec<EcoId> = values.filter_map(|v| registry.find(v)).collect();
                    if ecos.is_empty() {
                        return false;
                    }
                    self.ecosystems.extend(ecos);
                }
                "kind" | "k" => {
                    let kinds: Vec<ArtifactKind> = values.filter_map(ArtifactKind::parse).collect();
                    if kinds.is_empty() {
                        return false;
                    }
                    self.kinds.extend(kinds);
                }
                "is" => {
                    let states: Vec<StateFacet> = values
                        .filter_map(|v| match v {
                            "ready" => Some(StateFacet::Ready),
                            "review" => Some(StateFacet::Review),
                            "recent" => Some(StateFacet::Recent),
                            "protected" => Some(StateFacet::Protected),
                            "marked" => Some(StateFacet::Marked),
                            _ => None,
                        })
                        .collect();
                    if states.is_empty() {
                        return false;
                    }
                    self.states.extend(states);
                }
                _ => return false,
            }
            return true;
        }
        for (key, is_size) in [("size", true), ("age", false)] {
            let Some(rest) = token.strip_prefix(key) else {
                continue;
            };
            let (greater, value) =
                if let Some(v) = rest.strip_prefix(">=").or(rest.strip_prefix('>')) {
                    (true, v)
                } else if let Some(v) = rest.strip_prefix("<=").or(rest.strip_prefix('<')) {
                    (false, v)
                } else {
                    return false;
                };
            if is_size {
                let Some(bytes) = carwash_core::fmt::parse_bytes(value) else {
                    return false;
                };
                if greater {
                    self.min_size = Some(bytes);
                } else {
                    self.max_size = Some(bytes);
                }
            } else {
                let Some(age) = carwash_core::fmt::parse_age(value) else {
                    return false;
                };
                if greater {
                    self.older_than = Some(age);
                } else {
                    self.newer_than = Some(age);
                }
            }
            return true;
        }
        false
    }

    pub fn is_empty(&self) -> bool {
        self.raw.trim().is_empty()
    }

    pub fn matches(&self, candidate: &Candidate<'_>, fuzzy: &mut Fuzzy) -> bool {
        if !self.ecosystems.is_empty()
            && !candidate
                .ecosystem
                .is_some_and(|e| self.ecosystems.contains(&e))
        {
            return false;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&candidate.kind) {
            return false;
        }
        if let Some(min) = self.min_size
            && candidate.reclaimable.is_none_or(|r| r < min)
        {
            return false;
        }
        if let Some(max) = self.max_size
            && candidate.reclaimable.is_none_or(|r| r > max)
        {
            return false;
        }
        if let Some(older) = self.older_than
            && candidate.age.is_none_or(|a| a < older)
        {
            return false;
        }
        if let Some(newer) = self.newer_than
            && candidate.age.is_none_or(|a| a > newer)
        {
            return false;
        }
        if !self.states.is_empty()
            && !self.states.iter().any(|state| match state {
                StateFacet::Ready => candidate.hold.is_none(),
                StateFacet::Review => candidate.hold == Some(Hold::Review),
                StateFacet::Recent => candidate.hold == Some(Hold::Recent),
                StateFacet::Protected => candidate.hold == Some(Hold::Protected),
                StateFacet::Marked => candidate.marked,
            })
        {
            return false;
        }
        match &self.pattern {
            Some(pattern) => fuzzy.matches(pattern, candidate.haystack),
            None => true,
        }
    }
}

/// Reusable fuzzy matcher state (allocation-free after warm-up).
pub struct Fuzzy {
    matcher: Matcher,
    buf: Vec<char>,
}

impl Default for Fuzzy {
    fn default() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            buf: Vec::new(),
        }
    }
}

impl std::fmt::Debug for Fuzzy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Fuzzy")
    }
}

impl Fuzzy {
    fn matches(&mut self, pattern: &Pattern, haystack: &str) -> bool {
        pattern
            .score(Utf32Str::new(haystack, &mut self.buf), &mut self.matcher)
            .is_some()
    }

    /// Plain fuzzy match, for lists without facets.
    pub fn matches_text(&mut self, pattern: &Pattern, haystack: &str) -> bool {
        self.matches(pattern, haystack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate<'a>(haystack: &'a str, registry: &Registry) -> Candidate<'a> {
        Candidate {
            haystack,
            ecosystem: registry.find("rust"),
            kind: ArtifactKind::Build,
            reclaimable: Some(2_000_000_000),
            age: Some(Duration::from_secs(40 * 86_400)),
            hold: None,
            marked: false,
        }
    }

    #[test]
    fn facets_and_text() {
        let registry = Registry::builtin();
        let mut fuzzy = Fuzzy::default();
        let c = candidate("work/turbomcp/target", &registry);
        for (raw, expected) in [
            ("", true),
            ("turbo", true),
            ("tmcp", true),
            ("nomatch", false),
            ("eco:rust", true),
            ("eco:node", false),
            ("lang:node,rust", true),
            ("kind:deps", false),
            ("size>1g", true),
            ("size>5gb", false),
            ("size<3g", true),
            ("age>30d", true),
            ("age<7d", false),
            ("is:ready", true),
            ("is:review", false),
            ("turbo eco:rust size>1g is:ready", true),
        ] {
            let query = Query::parse(raw, &registry);
            assert_eq!(query.matches(&c, &mut fuzzy), expected, "{raw}");
        }
    }

    #[test]
    fn malformed_facets_become_text() {
        let registry = Registry::builtin();
        let query = Query::parse("eco:cobol", &registry);
        assert!(query.pattern.is_some());
        assert!(query.ecosystems.is_empty());
        let query = Query::parse("size>lots", &registry);
        assert!(query.min_size.is_none());
    }
}
