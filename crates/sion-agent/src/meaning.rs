//! Searching by meaning as well as by words: each text's embedding (a
//! vector a model gives it) compared with the query's, and the ranking that
//! gives fused with BM25's over the words, by reciprocal rank fusion, as
//! hybrid search does. A text found by either ranks; one both rank high
//! ranks first.
//!
//! Embeddings come from a provider's embeddings API ([`Api`]: OpenAI's,
//! Voyage's, or a server on this machine that speaks OpenAI's, such as
//! Ollama's), and the embeddings of what is searched are kept on disk by
//! the digest of each text ([`Cache`]), so that a text is embedded once.
//! Without an embedder, a search is BM25's alone.
//!
//! Scores here are for ranking only: nothing on the ledger depends on them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};
use sion_core::hash::hash_domain;

use crate::search::terms;

/// What reciprocal rank fusion adds to every rank (`1 / (K + rank)`).
pub const RRF_K: f64 = 60.0;

/// The most texts embedded in one request.
pub const BATCH: usize = 64;

/// The most texts not embedded yet that one search embeds: the rest are
/// ranked by their words alone until a later search embeds them.
pub const MAX_NEW: usize = 512;

/// The most characters of a text embedded.
pub const MAX_CHARS: usize = 8_000;

/// What a text is embedded as: some providers embed a query apart from
/// what it is to find.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// What is searched for.
    Query,
    /// What is searched.
    Document,
}

/// What turns texts into vectors.
pub trait Embedder: Send + Sync {
    /// The model, by name: vectors of two models are never compared.
    fn id(&self) -> &str;

    /// The vectors of `texts`, in order.
    fn embed(&self, texts: &[String], purpose: Purpose) -> Result<Vec<Vec<f32>>, String>;

    /// Whether it embeds on this machine, so the texts never leave it.
    fn is_local(&self) -> bool {
        false
    }
}

/// An embeddings API that speaks OpenAI's: `POST {base}/embeddings` with
/// the model and the texts, answered with a vector for each.
#[derive(Clone, Debug)]
pub struct Api {
    /// The client.
    pub client: sion_http::Client,
    /// The API's base (`https://api.openai.com/v1`).
    pub base: String,
    /// Its key, if it takes one.
    pub key: Option<String>,
    /// The model.
    pub model: String,
    /// Whether it takes Voyage's `input_type` (a query is embedded apart).
    pub voyage: bool,
}

impl Embedder for Api {
    fn id(&self) -> &str {
        &self.model
    }

    fn is_local(&self) -> bool {
        crate::oauth::is_loopback(&self.base)
    }

    fn embed(&self, texts: &[String], purpose: Purpose) -> Result<Vec<Vec<f32>>, String> {
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH) {
            let input: Vec<String> = batch
                .iter()
                .map(|t| t.chars().take(MAX_CHARS).collect())
                .collect();
            let mut body = json!({"model": self.model, "input": input});
            if self.voyage {
                body["input_type"] = json!(match purpose {
                    Purpose::Query => "query",
                    Purpose::Document => "document",
                });
            } else {
                body["encoding_format"] = json!("float");
            }
            let authorization = self.key.as_ref().map(|key| format!("Bearer {key}"));
            let mut headers = Vec::new();
            if let Some(value) = &authorization {
                headers.push(("Authorization", value.as_str()));
            }
            let url = format!("{}/embeddings", self.base.trim_end_matches('/'));
            let answer = sion_http::retrying(2, 10, &sion_http::sleep_seconds, &|| {
                self.client
                    .post_json(&url, &headers, body.to_string().as_bytes(), 64 << 20)
            })
            .map_err(|e| format!("the embeddings API: {e}"))?;
            let value: Value = serde_json::from_slice(&answer.body)
                .map_err(|e| format!("the embeddings API's answer: {e}"))?;
            let mut data: Vec<&Value> = value["data"]
                .as_array()
                .ok_or("the embeddings API gave no data")?
                .iter()
                .collect();
            data.sort_by_key(|d| d["index"].as_u64().unwrap_or(0));
            if data.len() != batch.len() {
                return Err(format!(
                    "the embeddings API gave {} vectors for {} texts",
                    data.len(),
                    batch.len()
                ));
            }
            for item in data {
                let vector: Vec<f32> = item["embedding"]
                    .as_array()
                    .ok_or("the embeddings API gave a vector that is not a list")?
                    .iter()
                    .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                    .collect();
                vectors.push(vector);
            }
        }
        Ok(vectors)
    }
}

/// How alike two vectors are: the cosine of the angle between them, from
/// −1 to 1; 0 for vectors of different lengths or of no length.
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// The embeddings of texts already embedded, by model and the digest of
/// each text, kept in a file of their own for each model.
#[derive(Debug)]
pub struct Cache {
    path: Option<PathBuf>,
    vectors: BTreeMap<[u8; 32], Vec<f32>>,
    changed: bool,
}

/// The most vectors a cache keeps for a model.
pub const MAX_CACHED: usize = 50_000;

fn digest(model: &str, text: &str) -> [u8; 32] {
    let mut data = model.as_bytes().to_vec();
    data.push(0);
    data.extend_from_slice(text.as_bytes());
    hash_domain("sion-agent/embedding/v1", &data).0
}

impl Cache {
    /// The cache of `model`'s vectors in `dir`, read if it is there; none
    /// kept on disk without a directory.
    pub fn open(dir: Option<PathBuf>, model: &str) -> Cache {
        let path = dir.map(|dir| {
            let name: String = hash_domain("sion-agent/embedding-model/v1", model.as_bytes()).0
                [..12]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            dir.join(format!("{name}.bin"))
        });
        let mut vectors = BTreeMap::new();
        if let Some(bytes) = path.as_ref().and_then(|p| std::fs::read(p).ok()) {
            let mut at = 0;
            while at + 36 <= bytes.len() {
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes[at..at + 32]);
                let dims = u32::from_le_bytes(bytes[at + 32..at + 36].try_into().unwrap_or([0; 4]))
                    as usize;
                let end = at + 36 + dims * 4;
                if end > bytes.len() {
                    break;
                }
                let vector = bytes[at + 36..end]
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                vectors.insert(key, vector);
                at = end;
            }
        }
        Cache {
            path,
            vectors,
            changed: false,
        }
    }

    /// How many vectors it holds.
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    /// Writes what was added (to a sibling first, then renamed).
    pub fn save(&mut self) -> Result<(), String> {
        let (Some(path), true) = (&self.path, self.changed) else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut bytes = Vec::new();
        for (key, vector) in &self.vectors {
            bytes.extend_from_slice(key);
            bytes.extend_from_slice(&(vector.len() as u32).to_le_bytes());
            for x in vector {
                bytes.extend_from_slice(&x.to_le_bytes());
            }
        }
        let partial = path.with_extension("partial");
        std::fs::write(&partial, bytes)
            .and_then(|()| std::fs::rename(&partial, path))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        self.changed = false;
        Ok(())
    }
}

/// The vectors of `texts` as documents: from the cache, the rest embedded
/// (at most [`MAX_NEW`] of them) and kept in it; none for a text left for
/// later.
pub fn embedded(
    embedder: &dyn Embedder,
    cache: &mut Cache,
    texts: &[String],
) -> Result<Vec<Option<Vec<f32>>>, String> {
    embedded_except(embedder, cache, texts, &[])
}

/// [`embedded`], the texts `kept[i]` says are kept here neither embedded
/// nor given a vector: they are found by words alone.
pub fn embedded_except(
    embedder: &dyn Embedder,
    cache: &mut Cache,
    texts: &[String],
    kept: &[bool],
) -> Result<Vec<Option<Vec<f32>>>, String> {
    let held = |i: usize| kept.get(i).copied().unwrap_or(false);
    let model = embedder.id().to_string();
    let keys: Vec<[u8; 32]> = texts.iter().map(|t| digest(&model, t)).collect();
    let mut missing: Vec<usize> = (0..texts.len())
        .filter(|i| !held(*i) && !cache.vectors.contains_key(&keys[*i]))
        .collect();
    // The same text once.
    let mut seen = std::collections::BTreeSet::new();
    missing.retain(|i| seen.insert(keys[*i]));
    missing.truncate(MAX_NEW);
    if !missing.is_empty() {
        let wanted: Vec<String> = missing.iter().map(|i| texts[*i].clone()).collect();
        let vectors = embedder.embed(&wanted, Purpose::Document)?;
        for (i, vector) in missing.iter().zip(vectors) {
            if cache.vectors.len() < MAX_CACHED {
                cache.vectors.insert(keys[*i], vector);
                cache.changed = true;
            }
        }
    }
    Ok(keys
        .iter()
        .enumerate()
        .map(|(i, key)| {
            if held(i) {
                None
            } else {
                cache.vectors.get(key).cloned()
            }
        })
        .collect())
}

/// `texts` ranked by BM25 against `query`'s terms (k₁ 1.2, b 0.75): the
/// places of those with a term of it, best first, and their scores.
pub fn bm25(texts: &[String], query: &str) -> Vec<(usize, f64)> {
    let wanted: std::collections::BTreeSet<String> = terms(query).into_iter().collect();
    if wanted.is_empty() || texts.is_empty() {
        return Vec::new();
    }
    let docs: Vec<BTreeMap<String, u32>> = texts
        .iter()
        .map(|t| {
            let mut counts = BTreeMap::new();
            for term in terms(t) {
                *counts.entry(term).or_insert(0u32) += 1;
            }
            counts
        })
        .collect();
    let lengths: Vec<f64> = docs
        .iter()
        .map(|d| f64::from(d.values().sum::<u32>()))
        .collect();
    let n = docs.len() as f64;
    let average = (lengths.iter().sum::<f64>() / n).max(1.0);
    let mut ranked: Vec<(usize, f64)> = Vec::new();
    for (i, doc) in docs.iter().enumerate() {
        let mut score = 0.0;
        for term in &wanted {
            let Some(&tf) = doc.get(term) else {
                continue;
            };
            let df = docs.iter().filter(|d| d.contains_key(term)).count() as f64;
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            let tf = f64::from(tf);
            let norm = 1.2 * (1.0 - 0.75 + 0.75 * lengths[i] / average);
            score += idf * tf * 2.2 / (tf + norm);
        }
        if score > 0.0 {
            ranked.push((i, score));
        }
    }
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    ranked
}

/// `rankings` (each of places, best first) fused by reciprocal rank: each
/// place's score the sum, over the rankings it is in, of `1 / (K + its
/// rank)`, best first, ties by place.
pub fn fuse(rankings: &[Vec<usize>]) -> Vec<(usize, f64)> {
    let mut scores: BTreeMap<usize, f64> = BTreeMap::new();
    for ranking in rankings {
        for (rank, place) in ranking.iter().enumerate() {
            *scores.entry(*place).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
    }
    let mut fused: Vec<(usize, f64)> = scores.into_iter().collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    fused
}

/// How a search found what it found: by its words, by its meaning, or
/// both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// BM25 ranked it.
    Words,
    /// Its embedding was near the query's.
    Meaning,
    /// Both.
    Both,
}

/// `texts` searched for `query`, at most `limit`: by BM25 over the words,
/// and, with an embedder, by the cosine of each text's embedding with the
/// query's (those at least `floor` alike), the two rankings fused. Each
/// with its place, its fused score and how it was found; and a note when
/// the meaning could not be searched (the embedder failed), the search
/// then by words alone.
pub fn search(
    texts: &[String],
    query: &str,
    limit: usize,
    meaning: Option<(&dyn Embedder, &mut Cache)>,
    floor: f64,
) -> (Vec<(usize, f64, Found)>, Option<String>) {
    search_except(texts, query, limit, meaning, floor, &[])
}

/// [`search`], the texts `kept[i]` says are kept here (confidential, with
/// an embedder elsewhere) never sent to the embedder: found by words alone.
pub fn search_except(
    texts: &[String],
    query: &str,
    limit: usize,
    meaning: Option<(&dyn Embedder, &mut Cache)>,
    floor: f64,
    kept: &[bool],
) -> (Vec<(usize, f64, Found)>, Option<String>) {
    let words: Vec<usize> = bm25(texts, query).into_iter().map(|(i, _)| i).collect();
    let mut note = None;
    let mut near: Vec<usize> = Vec::new();
    if let Some((embedder, cache)) = meaning {
        let found = embedder
            .embed(&[query.to_string()], Purpose::Query)
            .and_then(|q| {
                q.into_iter()
                    .next()
                    .ok_or_else(|| "no vector for the query".to_string())
            })
            .and_then(|q| {
                embedded_except(embedder, cache, texts, kept).map(|vectors| (q, vectors))
            });
        match found {
            Ok((query, vectors)) => {
                let mut alike: Vec<(usize, f64)> = vectors
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| Some((i, cosine(&query, v.as_ref()?))))
                    .filter(|(_, c)| *c >= floor)
                    .collect();
                alike.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(a.0.cmp(&b.0))
                });
                near = alike.into_iter().map(|(i, _)| i).collect();
                if let Err(e) = cache.save() {
                    note = Some(format!("the embeddings could not be kept: {e}"));
                }
            }
            Err(e) => note = Some(format!("searched by words alone: {e}")),
        }
    }
    let fused = fuse(&[words.clone(), near.clone()]);
    let found = fused
        .into_iter()
        .take(limit)
        .map(|(i, score)| {
            let how = match (words.contains(&i), near.contains(&i)) {
                (true, true) => Found::Both,
                (false, true) => Found::Meaning,
                _ => Found::Words,
            };
            (i, score, how)
        })
        .collect();
    (found, note)
}

/// The tool that searches the workspace's documents.
pub const DOCUMENT_SEARCH: &str = "document_search";

/// The lines of a passage of a document searched.
pub const PASSAGE_LINES: usize = 30;

/// The most passages a document search ranks.
pub const MAX_PASSAGES: usize = 20_000;

/// The most bytes of a file a document search reads.
pub const MAX_SEARCHED_FILE: u64 = 32 << 20;

/// Searches the workspace's files (text, and the documents
/// [`crate::reading`] reads: PDFs, workbooks, Word documents, decks) for
/// passages, by words and, with an embedder, by meaning, fused.
pub struct DocumentSearch {
    /// The workspace.
    pub root: PathBuf,
    /// What embeds, if search by meaning is set up.
    pub embedder: Option<std::sync::Arc<dyn Embedder>>,
    /// Where the vectors are kept.
    pub cache_dir: Option<PathBuf>,
    /// The paths of the workspace whose content is confidential.
    pub confidential: Vec<String>,
}

impl std::fmt::Debug for DocumentSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentSearch")
            .field("root", &self.root)
            .field(
                "embedder",
                &self.embedder.as_ref().map(|e| e.id().to_string()),
            )
            .finish()
    }
}

/// A passage of a document: its file, its first line, its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Passage {
    /// The file, relative to the workspace.
    pub path: String,
    /// Its first line, from 1.
    pub line: usize,
    /// Its text.
    pub text: String,
}

/// The passages of the workspace `root`'s files, [`PASSAGE_LINES`] lines
/// each, overlapping by a few: of text files and of the documents read
/// here, the rest left out.
pub fn passages(root: &std::path::Path) -> Vec<Passage> {
    let (files, _) = crate::code::walk(root);
    let mut out = Vec::new();
    for (path, full) in files {
        if out.len() >= MAX_PASSAGES {
            break;
        }
        if std::fs::metadata(&full).map_or(true, |m| m.len() > MAX_SEARCHED_FILE) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&full) else {
            continue;
        };
        let kind = crate::reading::kind_of(&bytes, &path, None);
        let text = match crate::reading::text_of(kind, &bytes, None) {
            Some(Ok(text)) => text,
            Some(Err(_)) => continue,
            None if kind == crate::reading::Kind::Text => {
                String::from_utf8_lossy(&bytes).into_owned()
            }
            None => continue,
        };
        let lines: Vec<&str> = text.lines().collect();
        let step = PASSAGE_LINES - PASSAGE_LINES / 6;
        let mut start = 0;
        while start < lines.len() && out.len() < MAX_PASSAGES {
            let end = (start + PASSAGE_LINES).min(lines.len());
            let text = lines[start..end].join("\n");
            if !text.trim().is_empty() {
                out.push(Passage {
                    path: path.clone(),
                    line: start + 1,
                    text,
                });
            }
            if end == lines.len() {
                break;
            }
            start += step;
        }
    }
    out
}

impl crate::tools::Tool for DocumentSearch {
    fn spec(&self) -> crate::model::ToolSpec {
        let how = if self.embedder.is_some() {
            "by the words they share with yours and by what they mean, the two fused"
        } else {
            "by the words they share with yours (BM25)"
        };
        crate::model::ToolSpec {
            name: DOCUMENT_SEARCH.into(),
            description: format!(
                "Search the files of your workspace for the passages that answer a question: \
                 text files, and PDFs, spreadsheets, Word documents and slide decks read as \
                 their text, ranked {how}. Each passage comes with its file and first line, \
                 to read on with read_file."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 20}
                },
                "required": ["query"]
            }),
        }
    }

    fn effect(&self) -> crate::tools::Effect {
        // An embedder elsewhere is sent the passages and the query.
        match &self.embedder {
            Some(e) if !e.is_local() => crate::tools::Effect::External,
            _ => crate::tools::Effect::Read,
        }
    }

    fn journaled(&self) -> bool {
        false
    }

    fn call(&self, input: &Value, _key: &str) -> crate::tools::Outcome {
        use crate::tools::Outcome;
        let query = input["query"].as_str().unwrap_or_default().trim();
        if query.is_empty() {
            return Outcome::error("give the query");
        }
        let limit = input["limit"].as_u64().unwrap_or(5).clamp(1, 20) as usize;
        let found = passages(&self.root);
        if found.is_empty() {
            return Outcome::ok("the workspace holds no text to search");
        }
        let texts: Vec<String> = found
            .iter()
            .map(|p| format!("{}\n{}", p.path, p.text))
            .collect();
        let mut cache = self
            .embedder
            .as_ref()
            .map(|e| Cache::open(self.cache_dir.clone(), e.id()));
        let meaning = self.embedder.as_deref().zip(cache.as_mut());
        // A confidential file's passages go to no embedder elsewhere.
        let remote = self.embedder.as_ref().is_some_and(|e| !e.is_local());
        let kept: Vec<bool> = found
            .iter()
            .map(|p| {
                remote && crate::builtin::confidential_in(&self.confidential, &self.root, &p.path)
            })
            .collect();
        let (ranked, note) = search_except(&texts, query, limit, meaning, 0.35, &kept);
        if ranked.is_empty() {
            return Outcome::ok(format!("no passage matches {query:?}"));
        }
        let mut held = false;
        let mut outside = false;
        let mut lines = Vec::new();
        for (i, _, how) in &ranked {
            let passage = &found[*i];
            held |= crate::builtin::confidential_in(&self.confidential, &self.root, &passage.path);
            outside |= crate::provenance::is_outside(
                &self.root,
                &passage.path,
                std::fs::read(self.root.join(&passage.path)).ok().as_deref(),
            );
            let how = match how {
                Found::Words => "words",
                Found::Meaning => "meaning",
                Found::Both => "words and meaning",
            };
            let shown: String = passage.text.chars().take(1_200).collect();
            lines.push(format!(
                "{}:{} (found by {how})\n{}",
                passage.path, passage.line, shown
            ));
        }
        if let Some(note) = note {
            lines.push(format!("[{note}]"));
        }
        let outcome = Outcome::ok(lines.join("\n\n"));
        let outcome = if outside {
            outcome.from_outside()
        } else {
            outcome
        };
        if held {
            outcome.held_confidential()
        } else {
            outcome
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Embeds by which of a few ideas a text names: a vector of one
    /// dimension for each.
    struct Ideas {
        asked: std::sync::Mutex<usize>,
    }

    const IDEAS: [&[&str]; 3] = [
        &["car", "automobile", "vehicle"],
        &["dog", "puppy"],
        &["invoice", "bill"],
    ];

    impl Embedder for Ideas {
        fn id(&self) -> &str {
            "ideas"
        }

        fn is_local(&self) -> bool {
            true
        }

        fn embed(&self, texts: &[String], _: Purpose) -> Result<Vec<Vec<f32>>, String> {
            *self.asked.lock().unwrap() += texts.len();
            Ok(texts
                .iter()
                .map(|t| {
                    let t = t.to_lowercase();
                    IDEAS
                        .iter()
                        .map(|words| {
                            if words.iter().any(|w| t.contains(w)) {
                                1.0
                            } else {
                                0.0
                            }
                        })
                        .collect()
                })
                .collect())
        }
    }

    fn texts() -> Vec<String> {
        [
            "Bought a used car in March",
            "The puppy chewed the sofa",
            "Pay the electricity bill by Friday",
            "The car needs new tyres",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn words_find_words_and_meaning_finds_what_they_mean() {
        // By words, "automobile" finds nothing.
        assert!(bm25(&texts(), "automobile").is_empty());
        let (found, note) = search(&texts(), "automobile", 5, None, 0.3);
        assert!(found.is_empty() && note.is_none());
        // By meaning, both texts about cars.
        let ideas = Ideas {
            asked: std::sync::Mutex::new(0),
        };
        let mut cache = Cache::open(None, "ideas");
        let (found, note) = search(&texts(), "automobile", 5, Some((&ideas, &mut cache)), 0.3);
        assert_eq!(note, None);
        let places: Vec<usize> = found.iter().map(|(i, _, _)| *i).collect();
        assert_eq!(places, vec![0, 3]);
        assert!(found.iter().all(|(_, _, how)| *how == Found::Meaning));
        // Found by both, a text ranks above one found by either.
        let (found, _) = search(
            &texts(),
            "new tyres for my automobile",
            5,
            Some((&ideas, &mut cache)),
            0.3,
        );
        assert_eq!(found[0].0, 3);
        assert_eq!(found[0].2, Found::Both);
        assert_eq!((found[1].0, found[1].2), (0, Found::Meaning));
        // What was embedded once is not embedded again.
        let asked = *ideas.asked.lock().unwrap();
        let _ = search(&texts(), "bill", 5, Some((&ideas, &mut cache)), 0.3);
        assert_eq!(*ideas.asked.lock().unwrap(), asked + 1, "only the query");
    }

    #[test]
    fn a_document_search_finds_passages_by_words_and_meaning() {
        use crate::tools::Tool;
        let root = std::env::temp_dir().join(format!("sion-docsearch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(
            root.join("notes/garage.md"),
            "# Garage\nThe car is booked for its service on Monday.\n",
        )
        .unwrap();
        std::fs::write(
            root.join("notes/pets.md"),
            "The puppy needs a walk at noon.\n",
        )
        .unwrap();
        std::fs::write(root.join("target/build.log"), "car car car\n").unwrap();
        let tool = DocumentSearch {
            root: root.clone(),
            embedder: Some(std::sync::Arc::new(Ideas {
                asked: std::sync::Mutex::new(0),
            })),
            cache_dir: Some(root.join(".cache")),
            confidential: vec!["notes/pets.md".into()],
        };
        assert_eq!(tool.effect(), crate::tools::Effect::Read);
        let found = tool.call(&json!({"query": "when is the automobile serviced"}), "k");
        assert!(!found.is_error, "{}", found.content);
        assert!(
            found
                .content
                .starts_with("notes/garage.md:1 (found by meaning)"),
            "{}",
            found.content
        );
        assert!(
            !found.content.contains("target/"),
            "build output is not searched"
        );
        assert!(!found.confidential);
        let pets = tool.call(&json!({"query": "puppy walk"}), "k");
        assert!(
            pets.content
                .contains("notes/pets.md:1 (found by words and meaning)"),
            "{}",
            pets.content
        );
        assert!(
            pets.confidential,
            "a confidential file's passage holds the session"
        );
        // Without an embedder, by words alone.
        let words = DocumentSearch {
            embedder: None,
            cache_dir: None,
            ..tool
        };
        let none = words.call(&json!({"query": "automobile"}), "k");
        assert!(
            none.content.contains("no passage matches"),
            "{}",
            none.content
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// An embedder elsewhere that writes down every text it is sent.
    struct Away(std::sync::Mutex<Vec<String>>);

    impl Embedder for Away {
        fn id(&self) -> &str {
            "away"
        }

        fn embed(&self, texts: &[String], _: Purpose) -> Result<Vec<Vec<f32>>, String> {
            self.0.lock().unwrap().extend(texts.iter().cloned());
            Ok(texts.iter().map(|t| vec![t.len() as f32, 1.0]).collect())
        }
    }

    #[test]
    fn a_confidential_files_passages_are_not_sent_to_an_embedder_elsewhere() {
        use crate::tools::Tool;
        let root = std::env::temp_dir().join(format!("sion-docaway-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("private")).unwrap();
        std::fs::write(
            root.join("private/keys.md"),
            "The deploy key is sk-live-42.\n",
        )
        .unwrap();
        std::fs::write(root.join("plan.md"), "The deploy is on Tuesday.\n").unwrap();
        let away = std::sync::Arc::new(Away(std::sync::Mutex::new(Vec::new())));
        let tool = DocumentSearch {
            root: root.clone(),
            embedder: Some(away.clone()),
            cache_dir: None,
            confidential: vec!["private/".into()],
        };
        assert_eq!(tool.effect(), crate::tools::Effect::External);
        let found = tool.call(&json!({"query": "deploy key"}), "k");
        assert!(
            found.content.contains("private/keys.md:1"),
            "{}",
            found.content
        );
        assert!(found.confidential);
        let sent = away.0.lock().unwrap().clone();
        assert!(sent.iter().any(|t| t.contains("Tuesday")), "{sent:?}");
        assert!(!sent.iter().any(|t| t.contains("sk-live")), "{sent:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rankings_are_fused_by_reciprocal_rank() {
        let fused = fuse(&[vec![2, 0, 1], vec![0, 3]]);
        let places: Vec<usize> = fused.iter().map(|(i, _)| *i).collect();
        assert_eq!(places, vec![0, 2, 3, 1]);
        assert!((fused[0].1 - (1.0 / 62.0 + 1.0 / 61.0)).abs() < 1e-12);
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-9);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn a_cache_keeps_its_vectors_on_disk() {
        let dir = std::env::temp_dir().join(format!("sion-embeddings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ideas = Ideas {
            asked: std::sync::Mutex::new(0),
        };
        let mut cache = Cache::open(Some(dir.clone()), "ideas");
        let vectors = embedded(&ideas, &mut cache, &texts()).unwrap();
        assert!(vectors.iter().all(Option::is_some));
        cache.save().unwrap();
        let again = Cache::open(Some(dir.clone()), "ideas");
        assert_eq!(again.len(), 4);
        // Another model's vectors are apart.
        assert!(Cache::open(Some(dir.clone()), "other").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
