use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sema_core::{SemaError, Value};

#[derive(Debug, Clone)]
pub struct VectorDocument {
    pub id: String,
    pub embedding: Vec<u8>,
    pub metadata: Value,
}

#[derive(Debug, Default)]
pub struct VectorStore {
    documents: Vec<VectorDocument>,
    /// Associated file path for persistence (set by vector-store/open).
    pub path: Option<String>,
}

impl VectorStore {
    pub fn new() -> Self {
        VectorStore {
            documents: Vec::new(),
            path: None,
        }
    }

    pub fn add(&mut self, doc: VectorDocument) {
        self.documents.retain(|d| d.id != doc.id);
        self.documents.push(doc);
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.documents.len();
        self.documents.retain(|d| d.id != id);
        self.documents.len() < before
    }

    pub fn count(&self) -> usize {
        self.documents.len()
    }

    pub fn documents(&self) -> &[VectorDocument] {
        &self.documents
    }

    /// Serialize the store to JSON bytes.
    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        let docs: Vec<serde_json::Value> = self
            .documents
            .iter()
            .map(|doc| {
                let mut obj = serde_json::Map::new();
                obj.insert("id".into(), serde_json::Value::String(doc.id.clone()));
                obj.insert(
                    "embedding".into(),
                    serde_json::Value::String(B64.encode(&doc.embedding)),
                );
                obj.insert("metadata".into(), sema_val_to_json(&doc.metadata));
                serde_json::Value::Object(obj)
            })
            .collect();
        let root = serde_json::json!({ "version": 1, "documents": docs });
        serde_json::to_string_pretty(&root)
            .map(|s| s.into_bytes())
            .map_err(|e| e.to_string())
    }

    /// Deserialize a store from JSON bytes.
    pub fn from_json(data: &[u8]) -> Result<Self, String> {
        let root: serde_json::Value =
            serde_json::from_slice(data).map_err(|e| format!("invalid JSON: {e}"))?;
        let docs_arr = root
            .get("documents")
            .and_then(|v| v.as_array())
            .ok_or("missing 'documents' array")?;
        let mut documents = Vec::with_capacity(docs_arr.len());
        for entry in docs_arr {
            let id = entry
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or("document missing 'id'")?
                .to_string();
            let emb_b64 = entry
                .get("embedding")
                .and_then(|v| v.as_str())
                .ok_or("document missing 'embedding'")?;
            let embedding = B64
                .decode(emb_b64)
                .map_err(|e| format!("invalid base64 embedding for '{id}': {e}"))?;
            let metadata = entry
                .get("metadata")
                .map(json_to_sema_val)
                .unwrap_or_else(Value::nil);
            documents.push(VectorDocument {
                id,
                embedding,
                metadata,
            });
        }
        Ok(VectorStore {
            documents,
            path: None,
        })
    }

    pub fn search(&self, query: &[u8], k: usize) -> Result<Vec<SearchResult>, SemaError> {
        let mut scored: Vec<SearchResult> = Vec::with_capacity(self.documents.len());
        for doc in &self.documents {
            // Surface dimension mismatches instead of silently dropping the
            // document as if it were irrelevant (returning a 0 score / skipping
            // it would hide a misconfigured / mixed-model store from the user).
            if query.len() != doc.embedding.len() {
                return Err(SemaError::eval(format!(
                    "vector-store/search: embedding dimension mismatch for document '{}': \
                     query has {} bytes ({} dims) but stored embedding has {} bytes ({} dims) \
                     — the query and stored embeddings must come from the same model",
                    doc.id,
                    query.len(),
                    query.len() / 8,
                    doc.embedding.len(),
                    doc.embedding.len() / 8,
                )));
            }
            let score = cosine_similarity(query, &doc.embedding).ok_or_else(|| {
                SemaError::eval(format!(
                    "vector-store/search: invalid embedding for document '{}': \
                     length {} is not a positive multiple of 8 bytes",
                    doc.id,
                    doc.embedding.len(),
                ))
            })?;
            scored.push(SearchResult {
                id: doc.id.clone(),
                score,
                metadata: doc.metadata.clone(),
            });
        }
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(k);
        Ok(scored)
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub score: f64,
    pub metadata: Value,
}

impl SearchResult {
    pub fn to_value(&self) -> Value {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("id"), Value::string(&self.id));
        map.insert(Value::keyword("score"), Value::float(self.score));
        map.insert(Value::keyword("metadata"), self.metadata.clone());
        Value::map(map)
    }
}

fn cosine_similarity(a: &[u8], b: &[u8]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() || !a.len().is_multiple_of(8) {
        return None;
    }
    let (mut dot, mut ma, mut mb) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (ca, cb) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
        let fa = f64::from_le_bytes(*ca);
        let fb = f64::from_le_bytes(*cb);
        dot += fa * fb;
        ma += fa * fa;
        mb += fb * fb;
    }
    if ma == 0.0 || mb == 0.0 {
        Some(0.0)
    } else {
        Some(dot / (ma.sqrt() * mb.sqrt()))
    }
}

// --- JSON <-> Value conversion (lossless for typical metadata) ---

fn sema_val_to_json(val: &Value) -> serde_json::Value {
    if val.is_nil() {
        return serde_json::Value::Null;
    }
    if let Some(b) = val.as_bool() {
        return serde_json::Value::Bool(b);
    }
    if let Some(i) = val.as_int() {
        return serde_json::Value::Number(i.into());
    }
    if let Some(f) = val.as_float() {
        return serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null);
    }
    if let Some(s) = val.as_str() {
        return serde_json::Value::String(s.to_string());
    }
    if let Some(kw) = val.as_keyword() {
        return serde_json::Value::String(format!(":{kw}"));
    }
    if let Some(l) = val.as_list() {
        return serde_json::Value::Array(l.iter().map(sema_val_to_json).collect());
    }
    if let Some(m) = val.as_map_rc() {
        let mut obj = serde_json::Map::new();
        for (k, v) in m.iter() {
            let key = k
                .as_keyword()
                .or_else(|| k.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| k.to_string());
            obj.insert(key, sema_val_to_json(v));
        }
        return serde_json::Value::Object(obj);
    }
    serde_json::Value::String(val.to_string())
}

fn json_to_sema_val(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::nil(),
        serde_json::Value::Bool(b) => Value::bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::int(i)
            } else if let Some(f) = n.as_f64() {
                Value::float(f)
            } else {
                Value::nil()
            }
        }
        serde_json::Value::String(s) => {
            if let Some(kw) = s.strip_prefix(':') {
                Value::keyword(kw)
            } else {
                Value::string(s)
            }
        }
        serde_json::Value::Array(a) => Value::list(a.iter().map(json_to_sema_val).collect()),
        serde_json::Value::Object(o) => {
            let mut map = BTreeMap::new();
            for (k, v) in o {
                map.insert(Value::keyword(k), json_to_sema_val(v));
            }
            Value::map(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emb(vals: &[f64]) -> Vec<u8> {
        vals.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    #[test]
    fn test_cosine_identical() {
        let a = emb(&[1.0, 0.0]);
        assert!((cosine_similarity(&a, &a).unwrap() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_cosine_orthogonal() {
        assert!(
            cosine_similarity(&emb(&[1.0, 0.0]), &emb(&[0.0, 1.0]))
                .unwrap()
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn test_store_crud() {
        let mut s = VectorStore::new();
        s.add(VectorDocument {
            id: "a".into(),
            embedding: emb(&[1.0, 0.0]),
            metadata: Value::nil(),
        });
        assert_eq!(s.count(), 1);
        assert!(s.delete("a"));
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn test_json_roundtrip() {
        let mut s = VectorStore::new();
        let mut meta = BTreeMap::new();
        meta.insert(Value::keyword("source"), Value::string("test.txt"));
        meta.insert(Value::keyword("page"), Value::int(3));
        s.add(VectorDocument {
            id: "d1".into(),
            embedding: emb(&[1.0, 0.0, 0.5]),
            metadata: Value::map(meta),
        });
        s.add(VectorDocument {
            id: "d2".into(),
            embedding: emb(&[0.0, 1.0, 0.5]),
            metadata: Value::nil(),
        });

        let json = s.to_json().unwrap();
        let s2 = VectorStore::from_json(&json).unwrap();
        assert_eq!(s2.count(), 2);
        assert_eq!(s2.documents()[0].id, "d1");
        assert_eq!(s2.documents()[1].id, "d2");
        assert_eq!(s2.documents()[0].embedding, emb(&[1.0, 0.0, 0.5]));
        assert_eq!(s2.documents()[1].embedding, emb(&[0.0, 1.0, 0.5]));
    }

    #[test]
    fn test_search_matching_dims_ok() {
        let mut s = VectorStore::new();
        s.add(VectorDocument {
            id: "a".into(),
            embedding: emb(&[1.0, 0.0]),
            metadata: Value::nil(),
        });
        s.add(VectorDocument {
            id: "b".into(),
            embedding: emb(&[0.0, 1.0]),
            metadata: Value::nil(),
        });
        let results = s.search(&emb(&[1.0, 0.0]), 10).unwrap();
        assert_eq!(results.len(), 2);
        // Identical vector ("a") must rank first.
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn test_search_dimension_mismatch_errors() {
        // Regression for LLM-7: a document whose embedding dimension does not
        // match the query must surface an error, not be silently dropped.
        let mut s = VectorStore::new();
        s.add(VectorDocument {
            id: "wrong-dims".into(),
            embedding: emb(&[1.0, 0.0, 0.0]), // 3 dims
            metadata: Value::nil(),
        });
        let err = s
            .search(&emb(&[1.0, 0.0]), 10) // 2-dim query
            .expect_err("dimension mismatch must error instead of silently dropping the doc");
        let msg = err.to_string();
        assert!(msg.contains("dimension mismatch"), "got: {msg}");
        assert!(msg.contains("wrong-dims"), "got: {msg}");
    }

    #[test]
    fn test_json_empty_store() {
        let s = VectorStore::new();
        let json = s.to_json().unwrap();
        let s2 = VectorStore::from_json(&json).unwrap();
        assert_eq!(s2.count(), 0);
    }
}
