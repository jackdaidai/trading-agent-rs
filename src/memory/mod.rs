//! BM25 memory system for tracking past trading situations and lessons

use regex::Regex;
use std::collections::HashMap;

/// A stored memory entry with situation and recommendation
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub situation: String,
    pub recommendation: String,
}

/// BM25-based memory system
pub struct BM25Memory {
    name: String,
    documents: Vec<String>,
    recommendations: Vec<String>,
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

    /// Tokenize text into words (simple ASCII tokenizer)
    fn tokenize(&self, text: &str) -> Vec<String> {
        let re = Regex::new(r"\b[a-zA-Z0-9]+\b").unwrap();
        re.find_iter(text)
            .map(|m| m.as_str().to_lowercase())
            .collect()
    }

    /// Add a situation and its recommendation to memory
    pub fn add(&mut self, situation: &str, recommendation: &str) {
        let tokens = self.tokenize(situation);
        self.documents.push(situation.to_string());
        self.recommendations.push(recommendation.to_string());
        self.tokenized_docs.push(tokens.clone());
        self.doc_lengths.push(tokens.len());
        self.recompute_idf();
    }

    /// Compute IDF values from document frequencies
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

        self.idf = df.iter()
            .map(|(term, &df)| {
                let idf = (n - df + 0.5) / (df + 0.5);
                (term.clone(), idf.ln())
            })
            .collect();

        let total_len: usize = self.doc_lengths.iter().sum();
        self.avgdl = total_len as f64 / n;
    }

    /// Get top-N similar memories for a given situation
    pub fn get_memories(&self, query: &str, n_matches: usize) -> Vec<MemoryMatch> {
        if self.documents.is_empty() {
            return Vec::new();
        }

        let query_tokens = self.tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        let mut scores: Vec<(usize, f64)> = self.tokenized_docs.iter()
            .enumerate()
            .map(|(i, doc)| (i, self.bm25_score(doc, &query_tokens)))
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scores.iter()
            .take(n_matches)
            .map(|(idx, score)| {
                let normalized_score = (score / 10.0).min(1.0).max(0.0);
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
    pub fn add_batch(&mut self, entries: &[(String, String)]) {
        for (situation, recommendation) in entries {
            self.add(situation, recommendation);
        }
    }

    pub fn len(&self) -> usize {
        self.documents.len()
    }

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
        assert!(tokens.contains(&"nvda"));
        assert!(tokens.contains(&"stock"));
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
}