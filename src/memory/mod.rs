//! BM25 memory system for tracking past trading situations and lessons

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

static WORD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[a-zA-Z0-9]+\b").unwrap());

/// A stored memory entry with situation and recommendation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct MemoryEntry {
    pub situation: String,
    pub recommendation: String,
}

/// On-disk format — only raw data, IDF/tokens recomputed on load
#[derive(Serialize, Deserialize)]
#[allow(dead_code)]
struct MemoryStore {
    entries: Vec<MemoryEntry>,
}

/// BM25-based memory system
pub struct BM25Memory {
    name: String,
    // Populated when memories are added or loaded. Runtime starts with empty memory today,
    // but persistence helpers use this to compute BM25 average document length.
    #[allow(dead_code)]
    documents: Vec<String>,
    recommendations: Vec<String>,
    #[allow(dead_code)]
    doc_lengths: Vec<usize>,
    avgdl: f64,
    idf: HashMap<String, f64>,
    tokenized_docs: Vec<Vec<String>>,
}

impl BM25Memory {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            documents: Vec::new(),
            recommendations: Vec::new(),
            doc_lengths: Vec::new(),
            avgdl: 0.0,
            idf: HashMap::new(),
            tokenized_docs: Vec::new(),
        }
    }

    /// Load from a JSON file, or create empty if file doesn't exist / is invalid.
    #[allow(dead_code)]
    pub fn from_file(name: &str, path: &Path) -> Self {
        let mut mem = Self::new(name);
        if let Ok(data) = std::fs::read_to_string(path) {
            if let Ok(store) = serde_json::from_str::<MemoryStore>(&data) {
                for entry in store.entries {
                    mem.add(&entry.situation, &entry.recommendation);
                }
            }
        }
        mem
    }

    /// Persist current entries to a JSON file.
    #[allow(dead_code)]
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let store = MemoryStore {
            entries: self
                .documents
                .iter()
                .zip(self.recommendations.iter())
                .map(|(s, r)| MemoryEntry {
                    situation: s.clone(),
                    recommendation: r.clone(),
                })
                .collect(),
        };
        let json = serde_json::to_string_pretty(&store)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Tokenize text into words (simple ASCII tokenizer)
    fn tokenize(&self, text: &str) -> Vec<String> {
        WORD_RE
            .find_iter(text)
            .map(|m| m.as_str().to_lowercase())
            .collect()
    }

    /// Add a situation and its recommendation to memory
    #[allow(dead_code)]
    pub fn add(&mut self, situation: &str, recommendation: &str) {
        let tokens = self.tokenize(situation);
        self.documents.push(situation.to_string());
        self.recommendations.push(recommendation.to_string());
        self.tokenized_docs.push(tokens.clone());
        self.doc_lengths.push(tokens.len());
        self.recompute_idf();
    }

    /// Compute IDF values from document frequencies
    #[allow(dead_code)]
    fn recompute_idf(&mut self) {
        let n = self.documents.len() as f64;
        if n == 0.0 {
            return;
        }

        let mut df: HashMap<String, f64> = HashMap::new();
        for doc in &self.tokenized_docs {
            let mut seen = std::collections::HashSet::new();
            for term in doc {
                if !seen.contains(term) {
                    *df.entry(term.clone()).or_insert(0.0) += 1.0;
                    seen.insert(term.clone());
                }
            }
        }

        self.idf = df
            .iter()
            .map(|(term, &df)| {
                let idf = ((n - df + 0.5) / (df + 0.5)).max(1.0).ln();
                (term.clone(), idf)
            })
            .collect();

        let total_len: usize = self.doc_lengths.iter().sum();
        self.avgdl = total_len as f64 / n;
    }

    /// Get top-N similar memories for a given situation
    pub fn get_memories(&self, query: &str, n_matches: usize) -> Vec<MemoryMatch> {
        if self.documents.is_empty() {
            tracing::debug!("BM25 memory '{}' has no entries", self.name);
            return Vec::new();
        }

        let query_tokens = self.tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        let mut scores: Vec<(usize, f64)> = self
            .tokenized_docs
            .iter()
            .enumerate()
            .map(|(i, doc)| (i, self.bm25_score(doc, &query_tokens)))
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scores
            .iter()
            .take(n_matches)
            .map(|(idx, score)| {
                let normalized_score = (score / 10.0).clamp(0.0, 1.0);
                MemoryMatch {
                    matched_situation: self.documents[*idx].clone(),
                    recommendation: self.recommendations[*idx].clone(),
                    similarity_score: normalized_score,
                }
            })
            .collect()
    }

    /// BM25 scoring function
    fn bm25_score(&self, doc: &[String], query: &[String]) -> f64 {
        let k1 = 1.5;
        let b = 0.75;
        let doc_len = doc.len() as f64;

        let mut score = 0.0;
        for term in query {
            let tf = doc.iter().filter(|&t| t == term).count() as f64;
            if tf == 0.0 {
                continue;
            }

            let idf = self.idf.get(term).copied().unwrap_or(0.0);
            let tf_norm = (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * doc_len / self.avgdl));

            score += idf * tf_norm;
        }
        score
    }

    /// Add multiple situations at once
    #[allow(dead_code)]
    pub fn add_batch(&mut self, entries: &[(String, String)]) {
        for (situation, recommendation) in entries {
            self.add(situation, recommendation);
        }
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct MemoryMatch {
    pub matched_situation: String,
    pub recommendation: String,
    pub similarity_score: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize() {
        let mem = BM25Memory::new("test");
        let tokens = mem.tokenize("NVDA stock analysis for 2026-04-25");
        assert!(tokens.contains(&"nvda".to_string()));
        assert!(tokens.contains(&"stock".to_string()));
    }

    #[test]
    fn test_bm25_retrieval() {
        let mut mem = BM25Memory::new("test");
        mem.add("NVDA had strong earnings, stock up 10%", "BUY");
        mem.add("NVDA missed revenue expectations", "SELL");
        mem.add("Market is stable, holding positions", "HOLD");

        let results = mem.get_memories("NVDA earnings beat", 2);
        assert_eq!(results.len(), 2);
        assert!(results[0].similarity_score > 0.0);
    }

    #[test]
    fn test_empty_memory_returns_empty() {
        let mem = BM25Memory::new("test");
        let results = mem.get_memories("anything", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_empty_query_returns_empty() {
        let mut mem = BM25Memory::new("test");
        mem.add("some doc", "rec");
        let results = mem.get_memories("!!!", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_add_batch() {
        let mut mem = BM25Memory::new("test");
        mem.add_batch(&[
            ("doc one".into(), "rec one".into()),
            ("doc two".into(), "rec two".into()),
        ]);
        assert_eq!(mem.len(), 2);
    }

    #[test]
    fn test_relevance_ordering() {
        let mut mem = BM25Memory::new("test");
        mem.add("apple fruit juice drink", "fruit");
        mem.add("apple stock price nasdaq", "stock");
        mem.add("orange banana mango", "other");

        let results = mem.get_memories("apple stock market", 3);
        // "apple stock price nasdaq" should rank highest (2 matching terms)
        assert_eq!(results[0].recommendation, "stock");
    }

    #[test]
    fn test_similarity_score_bounded() {
        let mut mem = BM25Memory::new("test");
        mem.add("test document here", "rec");
        let results = mem.get_memories("test document here", 1);
        assert!(!results.is_empty());
        assert!(results[0].similarity_score >= 0.0);
        assert!(results[0].similarity_score <= 1.0);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("tagent_test_memory");
        let path = dir.join("test_roundtrip.json");

        // Save
        let mut mem = BM25Memory::new("test");
        mem.add("NVDA strong earnings", "BUY");
        mem.add("Market downturn", "SELL");
        mem.save(&path).unwrap();

        // Load into fresh instance
        let mem2 = BM25Memory::from_file("test", &path);
        assert_eq!(mem2.len(), 2);

        // Verify BM25 index was rebuilt — query should work
        let results = mem2.get_memories("NVDA earnings", 1);
        assert!(!results.is_empty());
        assert_eq!(results[0].recommendation, "BUY");

        // Cleanup
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_from_file_missing_file() {
        let mem = BM25Memory::from_file("test", Path::new("/nonexistent/path.json"));
        assert!(mem.is_empty());
    }
}
