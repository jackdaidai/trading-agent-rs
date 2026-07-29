//! Memory system for tracking past trading situations and lessons.
//!
//! Provides two complementary subsystems:
//! - `BM25Memory`: fast in-session retrieval by query similarity (per-agent)
//! - `DecisionLog`: persistent on-disk log of trading decisions with resolution tracking

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static WORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[a-zA-Z0-9]+\b|\p{Han}+").unwrap());

/// Write `contents` to `path` atomically: write a temp file in the same
/// directory, then rename over the target. Prevents a crash mid-write from
/// leaving a truncated JSON file behind.
fn atomic_write(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Move an unparseable file out of the way instead of letting the next save
/// overwrite it. Returns the backup path if the rename succeeded.
fn backup_corrupt_file(path: &Path) -> Option<PathBuf> {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let backup = path.with_extension(format!("json.corrupt-{ts}"));
    match std::fs::rename(path, &backup) {
        Ok(()) => Some(backup),
        Err(e) => {
            tracing::warn!("Failed to back up corrupt file {}: {}", path.display(), e);
            None
        }
    }
}

/// Cap on stored memories per agent — oldest entries are dropped beyond this.
const MAX_MEMORY_ENTRIES: usize = 500;

/// A stored memory entry with situation and recommendation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub situation: String,
    pub recommendation: String,
}

/// On-disk format — only raw data, IDF/tokens recomputed on load
#[derive(Serialize, Deserialize)]
struct MemoryStore {
    entries: Vec<MemoryEntry>,
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

    /// Default on-disk location: `~/.trading-agent-rs/memory/<name>.json`
    pub fn default_path(name: &str) -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".trading-agent-rs")
            .join("memory")
            .join(format!("{name}.json"))
    }

    /// Load from a JSON file, or create empty if file doesn't exist / is invalid.
    /// An unparseable file is moved aside so a later save can't overwrite it.
    pub fn from_file(name: &str, path: &Path) -> Self {
        let mut mem = Self::new(name);
        if let Ok(data) = std::fs::read_to_string(path) {
            match serde_json::from_str::<MemoryStore>(&data) {
                Ok(store) => {
                    for entry in store.entries {
                        mem.add(&entry.situation, &entry.recommendation);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse memory '{}' at {}: {} — backing up corrupt file",
                        name,
                        path.display(),
                        e
                    );
                    backup_corrupt_file(path);
                }
            }
        }
        mem
    }

    /// Persist current entries to a JSON file.
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
        atomic_write(path, &json)?;
        Ok(())
    }

    /// Tokenize text: ASCII words plus CJK character bigrams so Chinese
    /// summaries/reflections are retrievable.
    fn tokenize(&self, text: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        for m in WORD_RE.find_iter(text) {
            let s = m.as_str();
            if s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
                tokens.push(s.to_lowercase());
            } else {
                // Han run: emit character bigrams (single char if run length 1)
                let chars: Vec<char> = s.chars().collect();
                if chars.len() == 1 {
                    tokens.push(chars[0].to_string());
                } else {
                    for w in chars.windows(2) {
                        tokens.push(w.iter().collect());
                    }
                }
            }
        }
        tokens
    }

    /// Add a situation and its recommendation to memory
    pub fn add(&mut self, situation: &str, recommendation: &str) {
        let tokens = self.tokenize(situation);
        self.documents.push(situation.to_string());
        self.recommendations.push(recommendation.to_string());
        self.tokenized_docs.push(tokens.clone());
        self.doc_lengths.push(tokens.len());
        if self.documents.len() > MAX_MEMORY_ENTRIES {
            let overflow = self.documents.len() - MAX_MEMORY_ENTRIES;
            self.documents.drain(..overflow);
            self.recommendations.drain(..overflow);
            self.tokenized_docs.drain(..overflow);
            self.doc_lengths.drain(..overflow);
        }
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

        // Only consider documents sharing at least one term with the query —
        // otherwise unrelated entries get returned as "similar past
        // situations" whenever nothing really matches. (A plain score > 0
        // filter is wrong: in small corpora the clamped IDF is ln(1) = 0, so
        // even genuine matches can score 0.)
        let query_set: std::collections::HashSet<&String> = query_tokens.iter().collect();
        let mut scores: Vec<(usize, f64)> = self
            .tokenized_docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| doc.iter().any(|t| query_set.contains(t)))
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

// =============================================================================
// Persistent Decision Log
// =============================================================================

/// Status of a decision entry
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Pending,
    Resolved,
}

/// A single decision log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionEntry {
    pub ticker: String,
    pub date: String,
    pub rating: String,
    pub confidence: String,
    pub summary: String,
    pub status: DecisionStatus,
    /// Filled on resolution: realized return since decision
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realized_return: Option<f64>,
    /// Filled on resolution: alpha vs benchmark
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f64>,
    /// Filled on resolution: one-paragraph reflection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reflection: Option<String>,
    /// ISO timestamp when the entry was created
    pub created_at: String,
    /// ISO timestamp when the entry was resolved (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
}

/// On-disk format for the decision log
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DecisionLogStore {
    entries: Vec<DecisionEntry>,
}

/// Persistent decision log that survives across runs.
///
/// Decisions are appended after each analysis. On subsequent runs for the same
/// ticker, pending entries are surfaced as context for the portfolio manager.
pub struct DecisionLog {
    path: PathBuf,
    entries: Vec<DecisionEntry>,
    max_resolved_entries: Option<usize>,
}

impl DecisionLog {
    /// Default path: `~/.trading-agent-rs/decisions/decisions.json`
    pub fn default_path() -> PathBuf {
        std::env::var("TRADING_AGENT_MEMORY_LOG_PATH")
            .or_else(|_| std::env::var("TAGENT_MEMORY_LOG_PATH"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".trading-agent-rs")
                    .join("decisions")
                    .join("decisions.json")
            })
    }

    /// Load from the given path, or create empty if not found.
    /// An unparseable file is moved aside so a later save can't overwrite it.
    pub fn load(path: &Path, max_resolved_entries: Option<usize>) -> Self {
        let entries = if path.exists() {
            match std::fs::read_to_string(path) {
                Ok(data) => serde_json::from_str::<DecisionLogStore>(&data)
                    .map(|s| s.entries)
                    .unwrap_or_else(|e| {
                        tracing::warn!(
                            "Failed to parse decision log: {} — backing up corrupt file",
                            e
                        );
                        backup_corrupt_file(path);
                        Vec::new()
                    }),
                Err(e) => {
                    tracing::warn!("Failed to read decision log: {}", e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        Self {
            path: path.to_path_buf(),
            entries,
            max_resolved_entries,
        }
    }

    /// Merge entries written to disk by another process since we loaded.
    /// Keyed by (ticker, date): a Resolved copy wins over a Pending one;
    /// on equal status the in-memory copy wins. Disk-only entries are kept.
    fn merge_from_disk(&mut self) {
        let Ok(data) = std::fs::read_to_string(&self.path) else {
            return;
        };
        let Ok(store) = serde_json::from_str::<DecisionLogStore>(&data) else {
            return;
        };
        for disk_entry in store.entries {
            let key = (disk_entry.ticker.to_lowercase(), disk_entry.date.clone());
            match self
                .entries
                .iter_mut()
                .find(|e| e.ticker.to_lowercase() == key.0 && e.date == key.1)
            {
                Some(mem_entry) => {
                    if mem_entry.status == DecisionStatus::Pending
                        && disk_entry.status == DecisionStatus::Resolved
                    {
                        *mem_entry = disk_entry;
                    }
                }
                None => self.entries.push(disk_entry),
            }
        }
    }

    /// Persist the log to disk (atomic write, merging concurrent updates).
    pub fn save(&mut self) -> anyhow::Result<()> {
        self.merge_from_disk();
        self.prune_resolved();
        let store = DecisionLogStore {
            entries: self.entries.clone(),
        };
        let json = serde_json::to_string_pretty(&store)?;
        atomic_write(&self.path, &json)?;
        tracing::debug!("Decision log saved to {}", self.path.display());
        Ok(())
    }

    /// Append a new pending decision after a completed analysis.
    /// Re-running the same ticker+date replaces the prior pending entry
    /// instead of accumulating duplicates.
    pub fn log_decision(
        &mut self,
        ticker: &str,
        date: &str,
        rating: &str,
        confidence: &str,
        summary: &str,
    ) {
        self.entries.retain(|e| {
            !(e.status == DecisionStatus::Pending
                && e.ticker.eq_ignore_ascii_case(ticker)
                && e.date == date)
        });
        let entry = DecisionEntry {
            ticker: ticker.to_string(),
            date: date.to_string(),
            rating: rating.to_string(),
            confidence: confidence.to_string(),
            summary: summary.to_string(),
            status: DecisionStatus::Pending,
            realized_return: None,
            alpha: None,
            reflection: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            resolved_at: None,
        };
        self.entries.push(entry);
        self.prune_resolved();
    }

    /// All pending decisions across tickers (oldest first).
    pub fn pending(&self) -> Vec<&DecisionEntry> {
        self.entries
            .iter()
            .filter(|e| e.status == DecisionStatus::Pending)
            .collect()
    }

    /// Resolve a pending entry with outcome data.
    pub fn resolve(
        &mut self,
        ticker: &str,
        date: &str,
        realized_return: f64,
        alpha: f64,
        reflection: &str,
    ) {
        for entry in self.entries.iter_mut() {
            if entry.ticker == ticker
                && entry.date == date
                && entry.status == DecisionStatus::Pending
            {
                entry.status = DecisionStatus::Resolved;
                entry.realized_return = Some(realized_return);
                entry.alpha = Some(alpha);
                entry.reflection = Some(reflection.to_string());
                entry.resolved_at = Some(chrono::Utc::now().to_rfc3339());
                break;
            }
        }
        self.prune_resolved();
    }

    /// Get all pending decisions for a ticker (for context injection).
    pub fn pending_for_ticker(&self, ticker: &str) -> Vec<&DecisionEntry> {
        self.entries
            .iter()
            .filter(|e| {
                e.ticker.eq_ignore_ascii_case(ticker) && e.status == DecisionStatus::Pending
            })
            .collect()
    }

    /// Get recent resolved decisions for a ticker (lessons learned).
    pub fn resolved_for_ticker(&self, ticker: &str, limit: usize) -> Vec<&DecisionEntry> {
        self.entries
            .iter()
            .filter(|e| {
                e.ticker.eq_ignore_ascii_case(ticker) && e.status == DecisionStatus::Resolved
            })
            .rev()
            .take(limit)
            .collect()
    }

    /// Format decision history as context for prompts.
    pub fn format_context(&self, ticker: &str) -> String {
        let pending_all = self.pending_for_ticker(ticker);
        // Cap prompt context to the 5 most recent pending decisions
        let pending = &pending_all[pending_all.len().saturating_sub(5)..];
        let resolved = self.resolved_for_ticker(ticker, 5);

        if pending.is_empty() && resolved.is_empty() {
            return String::new();
        }

        let mut ctx = String::from("## Prior Decision History\n\n");

        if !resolved.is_empty() {
            ctx.push_str("### Resolved (with outcomes)\n");
            for e in &resolved {
                ctx.push_str(&format!(
                    "- [{date}] {rating} (confidence: {conf}) → Return: {ret:.1}%, Alpha: {alpha:.1}%\n  Reflection: {refl}\n",
                    date = e.date,
                    rating = e.rating,
                    conf = e.confidence,
                    ret = e.realized_return.unwrap_or(0.0),
                    alpha = e.alpha.unwrap_or(0.0),
                    refl = e.reflection.as_deref().unwrap_or("N/A"),
                ));
            }
            ctx.push('\n');
        }

        if !pending.is_empty() {
            ctx.push_str("### Pending (awaiting resolution)\n");
            for e in pending {
                ctx.push_str(&format!(
                    "- [{date}] {rating} (confidence: {conf}): {summary}\n",
                    date = e.date,
                    rating = e.rating,
                    conf = e.confidence,
                    summary = truncate_str(&e.summary, 200),
                ));
            }
            ctx.push('\n');
        }

        ctx
    }

    /// Total entry count
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Prune oldest resolved entries if over the cap.
    fn prune_resolved(&mut self) {
        if let Some(max) = self.max_resolved_entries {
            let resolved_count = self
                .entries
                .iter()
                .filter(|e| e.status == DecisionStatus::Resolved)
                .count();
            if resolved_count > max {
                let to_remove = resolved_count - max;
                let mut removed = 0;
                self.entries.retain(|e| {
                    if removed >= to_remove {
                        return true;
                    }
                    if e.status == DecisionStatus::Resolved {
                        removed += 1;
                        return false;
                    }
                    true
                });
            }
        }
    }
}

fn truncate_str(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        s
    } else {
        let end = s
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        &s[..end]
    }
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
        let dir = std::env::temp_dir().join("trading_agent_rs_test_memory");
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
    fn test_memory_caps_entries_at_max() {
        let mut mem = BM25Memory::new("test");
        for i in 0..(MAX_MEMORY_ENTRIES + 10) {
            mem.add(&format!("doc number {}", i), "rec");
        }
        assert_eq!(mem.len(), MAX_MEMORY_ENTRIES);
        // Oldest entries were dropped — doc 0 should be gone
        let results = mem.get_memories(&format!("doc number {}", MAX_MEMORY_ENTRIES + 9), 1);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_from_file_missing_file() {
        let mem = BM25Memory::from_file("test", Path::new("/nonexistent/path.json"));
        assert!(mem.is_empty());
    }

    // =========================================================================
    // DecisionLog tests
    // =========================================================================

    #[test]
    fn test_decision_log_roundtrip() {
        let dir = std::env::temp_dir().join("trading_agent_rs_test_decision_log");
        let path = dir.join("test_decisions.json");

        let mut log = DecisionLog::load(&path, None);
        log.log_decision("AAPL", "2026-05-01", "BUY", "High", "Strong earnings beat");
        log.log_decision("MSFT", "2026-05-01", "HOLD", "Medium", "Neutral outlook");
        log.save().unwrap();

        // Reload
        let log2 = DecisionLog::load(&path, None);
        assert_eq!(log2.len(), 2);
        assert_eq!(log2.pending_for_ticker("AAPL").len(), 1);
        assert_eq!(log2.pending_for_ticker("MSFT").len(), 1);

        // Cleanup
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_decision_log_resolve() {
        let dir = std::env::temp_dir().join("trading_agent_rs_test_decision_resolve");
        let path = dir.join("test_resolve.json");

        let mut log = DecisionLog::load(&path, None);
        log.log_decision("NVDA", "2026-04-15", "BUY", "High", "AI demand strong");
        log.resolve("NVDA", "2026-04-15", 12.5, 8.3, "Thesis played out well");

        assert_eq!(log.pending_for_ticker("NVDA").len(), 0);
        assert_eq!(log.resolved_for_ticker("NVDA", 5).len(), 1);

        let resolved = &log.resolved_for_ticker("NVDA", 5)[0];
        assert_eq!(resolved.realized_return, Some(12.5));
        assert_eq!(resolved.alpha, Some(8.3));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_decision_log_prune_resolved() {
        let path = std::env::temp_dir().join("trading_agent_rs_prune.json");
        let mut log = DecisionLog::load(&path, Some(2));

        // Add 4 entries and resolve 3 of them
        log.log_decision("A", "2026-01-01", "BUY", "H", "s1");
        log.log_decision("B", "2026-01-02", "SELL", "M", "s2");
        log.log_decision("C", "2026-01-03", "HOLD", "L", "s3");
        log.log_decision("D", "2026-01-04", "BUY", "H", "s4");

        log.resolve("A", "2026-01-01", 5.0, 2.0, "ok");
        log.resolve("B", "2026-01-02", -3.0, -1.0, "bad");
        log.resolve("C", "2026-01-03", 0.0, 0.0, "flat");

        // Should keep only 2 resolved (B and C pruned, keep most recent? Actually it prunes oldest first)
        let resolved_count = log
            .entries
            .iter()
            .filter(|e| e.status == DecisionStatus::Resolved)
            .count();
        assert!(resolved_count <= 2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_decision_log_pending_lists_all_tickers() {
        let path = std::env::temp_dir().join("trading_agent_rs_pending_all.json");
        let mut log = DecisionLog::load(&path, None);
        log.log_decision("AAPL", "2026-05-01", "BUY", "High", "s1");
        log.log_decision("MSFT", "2026-05-02", "HOLD", "Medium", "s2");
        log.resolve("AAPL", "2026-05-01", 1.0, 0.5, "done");

        let pending = log.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].ticker, "MSFT");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_decision_log_rerun_replaces_pending() {
        let path = std::env::temp_dir().join("trading_agent_rs_rerun.json");
        let mut log = DecisionLog::load(&path, None);
        log.log_decision("AAPL", "2026-05-01", "BUY", "High", "first run");
        log.log_decision("AAPL", "2026-05-01", "HOLD", "Medium", "second run");

        let pending = log.pending_for_ticker("AAPL");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].rating, "HOLD");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_format_context_caps_pending_entries() {
        let path = std::env::temp_dir().join("trading_agent_rs_cap.json");
        let mut log = DecisionLog::load(&path, None);
        for i in 1..=8 {
            log.log_decision("NVDA", &format!("2026-01-{:02}", i), "BUY", "High", "s");
        }

        let ctx = log.format_context("NVDA");
        // Only the 5 most recent pending decisions appear
        assert!(!ctx.contains("2026-01-01"));
        assert!(!ctx.contains("2026-01-03"));
        assert!(ctx.contains("2026-01-04"));
        assert!(ctx.contains("2026-01-08"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_decision_log_format_context() {
        let path = std::env::temp_dir().join("trading_agent_rs_ctx.json");
        let mut log = DecisionLog::load(&path, None);
        log.log_decision("AAPL", "2026-05-01", "BUY", "High", "Strong growth thesis");

        let ctx = log.format_context("AAPL");
        assert!(ctx.contains("Prior Decision History"));
        assert!(ctx.contains("Pending"));
        assert!(ctx.contains("BUY"));

        // Empty for unknown ticker
        let ctx2 = log.format_context("UNKNOWN");
        assert!(ctx2.is_empty());

        let _ = std::fs::remove_file(&path);
    }
}
