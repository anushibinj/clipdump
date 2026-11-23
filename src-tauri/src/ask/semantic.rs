//! Semantic ranking for Ask: items found by what they mean, not by the words they share with
//! the question ("the boss wants the numbers" against "Priya asked for the Q4 report").
//!
//! It is opt-in (an embedding model in Settings) because the text of every note, task and
//! clipboard item is sent to the server once to be embedded. The vectors live in memory only,
//! in an [`EmbedCache`] that outlives one question: a vector is rebuilt only when an item's text
//! changes, so after the first question a new one costs a single call, for the question itself.
//! Nothing is written to disk: a file of vectors would be a searchable shadow of the encrypted
//! history.
//!
//! The first question embeds the newest [`MAX_NEW_PER_ASK`] items; a bigger history warms up
//! over the next few questions instead of making the first one wait for thousands of vectors.
//! Everything here only *ranks*: the answer is still checked against the excerpts, so a
//! wrong neighbour costs a slot among the excerpts, never a wrong answer.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::{Candidate, Source};
use crate::llm::{HttpPost, LlmConfig, clip_to, embed};

/// Characters of an item that are embedded (its title and the start of its text).
const EMBED_CHARS: usize = 1_000;
/// Items per embeddings call.
const BATCH: usize = 32;
/// Items embedded at most during one question.
pub const MAX_NEW_PER_ASK: usize = 800;
/// Items returned at most.
pub const MAX_SEMANTIC: usize = 60;
/// Cosine similarity below this is noise, not a neighbour.
const MIN_SIMILARITY: f32 = 0.25;

type Key = (Source, u64);

/// Vectors of the items seen so far, for one embedding model.
#[derive(Default)]
pub struct EmbedCache {
    model: String,
    vectors: HashMap<Key, (u64, Vec<f32>)>,
}

impl EmbedCache {
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }
}

/// What of an item is embedded: its title, then the start of its text.
fn input_of(c: &Candidate) -> String {
    let mut s = String::new();
    if !c.title.is_empty() {
        s.push_str(&c.title);
        s.push('\n');
    }
    s.push_str(clip_to(&c.text, EMBED_CHARS));
    s
}

fn stamp(input: &str) -> u64 {
    let mut h = DefaultHasher::new();
    input.hash(&mut h);
    h.finish()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// The items closest in meaning to `query`, nearest first, at most [`MAX_SEMANTIC`].
///
/// Items missing from the cache (or whose text changed) are embedded first, newest first, up to
/// [`MAX_NEW_PER_ASK`]; items that are gone are forgotten. A server that cannot embed is an
/// error, and the caller then searches by words alone.
pub fn rank(
    cfg: &LlmConfig,
    model: &str,
    http: &dyn HttpPost,
    cache: &mut EmbedCache,
    query: &str,
    candidates: &[Candidate],
) -> Result<Vec<(Source, u64)>, String> {
    if cache.model != model {
        cache.vectors.clear();
        cache.model = model.to_owned();
    }
    let keep: HashMap<Key, &Candidate> = candidates.iter().map(|c| ((c.source, c.id), c)).collect();
    cache.vectors.retain(|k, _| keep.contains_key(k));

    let mut todo: Vec<(Key, String, u64, i64)> = Vec::new();
    for c in candidates {
        let key = (c.source, c.id);
        let input = input_of(c);
        let at = stamp(&input);
        if cache.vectors.get(&key).is_none_or(|(old, _)| *old != at) {
            todo.push((key, input, at, c.touched_at));
        }
    }
    todo.sort_by(|a, b| b.3.cmp(&a.3));
    todo.truncate(MAX_NEW_PER_ASK);
    for chunk in todo.chunks(BATCH) {
        let inputs: Vec<&str> = chunk.iter().map(|(_, i, _, _)| i.as_str()).collect();
        let vectors = embed(cfg, model, http, &inputs)?;
        for ((key, _, at, _), v) in chunk.iter().zip(vectors) {
            cache.vectors.insert(*key, (*at, v));
        }
    }

    let q = embed(cfg, model, http, &[clip_to(query, EMBED_CHARS)])?
        .into_iter()
        .next()
        .ok_or_else(|| "the server returned no embedding".to_owned())?;
    let mut scored: Vec<(Key, f32, i64)> = cache
        .vectors
        .iter()
        .filter_map(|(k, (_, v))| {
            let s = cosine(&q, v);
            (s >= MIN_SIMILARITY).then(|| (*k, s, keep[k].touched_at))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.0.1.cmp(&b.0.1))
    });
    scored.truncate(MAX_SEMANTIC);
    Ok(scored.into_iter().map(|(k, _, _)| k).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::cell::RefCell;

    /// A server whose "meaning" is the share of `a`s and `b`s in the text, so a test can say
    /// which items are neighbours.
    struct Letters {
        calls: RefCell<Vec<Vec<String>>>,
        fail: bool,
    }

    impl Letters {
        fn new() -> Self {
            Self {
                calls: RefCell::default(),
                fail: false,
            }
        }
        fn inputs(&self) -> usize {
            self.calls.borrow().iter().map(Vec::len).sum()
        }
    }

    impl HttpPost for Letters {
        fn post_json(&self, _: &str, _: Option<&str>, body: &Value) -> Result<String, String> {
            if self.fail {
                return Err("could not reach the LLM server".into());
            }
            let inputs: Vec<String> = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            let data: Vec<Value> = inputs
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let a = t.matches('a').count() as f64;
                    let b = t.matches('b').count() as f64;
                    json!({ "index": i, "embedding": [a, b] })
                })
                .collect();
            self.calls.borrow_mut().push(inputs);
            Ok(json!({ "data": data }).to_string())
        }
    }

    fn cfg() -> LlmConfig {
        LlmConfig {
            base_url: "http://h/v1".into(),
            model: "chat".into(),
            api_key: String::new(),
        }
    }

    fn cand(id: u64, touched: i64, text: &str) -> Candidate {
        Candidate::new(Source::Note, id, touched, touched, "", text)
    }

    #[test]
    fn the_nearest_in_meaning_come_first_and_noise_is_left_out() {
        let http = Letters::new();
        let mut cache = EmbedCache::default();
        let all = [
            cand(1, 1, "bbbb"),
            cand(2, 2, "aaab"),
            cand(3, 3, "aaaa"),
            cand(4, 4, "zzzz"),
        ];
        let got = rank(&cfg(), "emb", &http, &mut cache, "aaa", &all).unwrap();
        assert_eq!(
            got,
            vec![(Source::Note, 3), (Source::Note, 2)],
            "bbbb is orthogonal and zzzz has no meaning at all"
        );
    }

    #[test]
    fn a_second_question_embeds_only_the_question() {
        let http = Letters::new();
        let mut cache = EmbedCache::default();
        let all = [cand(1, 1, "aa"), cand(2, 2, "ab")];
        rank(&cfg(), "emb", &http, &mut cache, "a", &all).unwrap();
        assert_eq!(http.inputs(), 3);
        rank(&cfg(), "emb", &http, &mut cache, "b", &all).unwrap();
        assert_eq!(http.inputs(), 4);
        // An edited item is embedded again, a deleted one is forgotten.
        let edited = [cand(1, 3, "aaa")];
        rank(&cfg(), "emb", &http, &mut cache, "a", &edited).unwrap();
        assert_eq!(http.inputs(), 6);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn another_model_starts_from_nothing() {
        let http = Letters::new();
        let mut cache = EmbedCache::default();
        let all = [cand(1, 1, "aa")];
        rank(&cfg(), "emb", &http, &mut cache, "a", &all).unwrap();
        rank(&cfg(), "other", &http, &mut cache, "a", &all).unwrap();
        assert_eq!(http.inputs(), 4, "the item is embedded once per model");
    }

    #[test]
    fn a_big_history_is_embedded_newest_first_and_in_batches() {
        let http = Letters::new();
        let mut cache = EmbedCache::default();
        let n = MAX_NEW_PER_ASK as u64 + 50;
        let all: Vec<Candidate> = (0..n).map(|i| cand(i, i as i64, "a")).collect();
        rank(&cfg(), "emb", &http, &mut cache, "a", &all).unwrap();
        assert_eq!(cache.len(), MAX_NEW_PER_ASK);
        assert!(cache.vectors.contains_key(&(Source::Note, n - 1)));
        assert!(!cache.vectors.contains_key(&(Source::Note, 0)));
        assert!(http.calls.borrow().iter().all(|c| c.len() <= BATCH));
        // The next question picks up the rest.
        rank(&cfg(), "emb", &http, &mut cache, "a", &all).unwrap();
        assert_eq!(cache.len(), n as usize);
    }

    #[test]
    fn a_title_counts_and_the_text_is_bounded() {
        let long = Candidate::new(Source::Note, 1, 1, 1, "bb", &"a".repeat(EMBED_CHARS * 3));
        assert_eq!(input_of(&long).chars().count(), 3 + EMBED_CHARS);
        assert_eq!(input_of(&cand(2, 1, "x")), "x");
    }

    #[test]
    fn a_server_that_cannot_embed_is_an_error() {
        let mut http = Letters::new();
        http.fail = true;
        let mut cache = EmbedCache::default();
        let e = rank(&cfg(), "emb", &http, &mut cache, "a", &[cand(1, 1, "a")]).unwrap_err();
        assert!(e.contains("could not reach"));
    }

    #[test]
    fn cosine_is_zero_for_nothing_to_compare() {
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
        assert!((cosine(&[1.0, 1.0], &[2.0, 2.0]) - 1.0).abs() < 1e-6);
    }
}
