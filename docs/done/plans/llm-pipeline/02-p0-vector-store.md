# P0 — Vector Store (Tasks 8-9)

> Parent plan: [../2026-02-17-llm-pipeline-stdlib.md](../2026-02-17-llm-pipeline-stdlib.md)

---

## Task 8: In-Memory Vector Store

**Files:**

- Create: `crates/sema-llm/src/vector_store.rs`
- Modify: `crates/sema-llm/src/lib.rs` (add `pub mod vector_store`)
- Modify: `crates/sema-llm/src/builtins.rs` (register builtins, add thread-local)
- Test: `crates/sema/tests/integration_test.rs`

### Overview

Thread-local `HashMap<String, VectorStore>` holding named stores. Each store holds documents
as `(id, embedding_bytevector, metadata_map)`. Search is brute-force cosine similarity (k-NN).
Embeddings stored as packed f64 bytevectors (same format as `llm/embed` output).

### Step 1: Write failing tests

```rust
#[test]
fn test_vector_store_create() {
    let result = eval(r#"(vector-store/create "test-store")"#);
    assert_eq!(result, Value::string("test-store"));
}

#[test]
fn test_vector_store_count_empty() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "ct")"#).unwrap();
    assert_eq!(interp.eval_str(r#"(vector-store/count "ct")"#).unwrap(), Value::int(0));
}

#[test]
fn test_vector_store_add_and_count() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "add-t")"#).unwrap();
    interp.eval_str(
        r#"(vector-store/add "add-t" "doc1" (embedding/list->embedding '(1.0 0.0 0.0)) {:title "Doc 1"})"#
    ).unwrap();
    assert_eq!(interp.eval_str(r#"(vector-store/count "add-t")"#).unwrap(), Value::int(1));
}

#[test]
fn test_vector_store_search() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "s-t")"#).unwrap();
    interp.eval_str(r#"(vector-store/add "s-t" "x" (embedding/list->embedding '(1.0 0.0 0.0)) {:axis "x"})"#).unwrap();
    interp.eval_str(r#"(vector-store/add "s-t" "y" (embedding/list->embedding '(0.0 1.0 0.0)) {:axis "y"})"#).unwrap();
    interp.eval_str(r#"(vector-store/add "s-t" "z" (embedding/list->embedding '(0.0 0.0 1.0)) {:axis "z"})"#).unwrap();
    let result = interp.eval_str(
        r#"(vector-store/search "s-t" (embedding/list->embedding '(0.9 0.1 0.0)) 1)"#
    ).unwrap();
    let results = result.as_list().unwrap();
    assert_eq!(results.len(), 1);
    let first = results[0].as_map_rc().unwrap();
    assert_eq!(first.get(&Value::keyword("id")).unwrap().as_str().unwrap(), "x");
    let score = first.get(&Value::keyword("score")).unwrap().as_float().unwrap();
    assert!(score > 0.9);
}

#[test]
fn test_vector_store_search_top_k() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "tk")"#).unwrap();
    interp.eval_str(r#"(vector-store/add "tk" "a" (embedding/list->embedding '(1.0 0.0)) {})"#).unwrap();
    interp.eval_str(r#"(vector-store/add "tk" "b" (embedding/list->embedding '(0.9 0.1)) {})"#).unwrap();
    interp.eval_str(r#"(vector-store/add "tk" "c" (embedding/list->embedding '(0.0 1.0)) {})"#).unwrap();
    let result = interp.eval_str(r#"(vector-store/search "tk" (embedding/list->embedding '(1.0 0.0)) 2)"#).unwrap();
    let results = result.as_list().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].as_map_rc().unwrap().get(&Value::keyword("id")).unwrap().as_str().unwrap(), "a");
}

#[test]
fn test_vector_store_delete() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "del")"#).unwrap();
    interp.eval_str(r#"(vector-store/add "del" "d1" (embedding/list->embedding '(1.0 0.0)) {})"#).unwrap();
    interp.eval_str(r#"(vector-store/add "del" "d2" (embedding/list->embedding '(0.0 1.0)) {})"#).unwrap();
    assert_eq!(interp.eval_str(r#"(vector-store/delete "del" "d1")"#).unwrap(), Value::bool(true));
    assert_eq!(interp.eval_str(r#"(vector-store/count "del")"#).unwrap(), Value::int(1));
}

#[test]
fn test_vector_store_delete_nonexistent() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "dn")"#).unwrap();
    assert_eq!(interp.eval_str(r#"(vector-store/delete "dn" "nope")"#).unwrap(), Value::bool(false));
}

#[test]
fn test_vector_store_not_found() {
    let interp = Interpreter::new();
    assert!(interp.eval_str(r#"(vector-store/count "nonexistent")"#).is_err());
}

#[test]
fn test_vector_store_search_returns_metadata() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "mt")"#).unwrap();
    interp.eval_str(r#"(vector-store/add "mt" "d1" (embedding/list->embedding '(1.0 0.0)) {:source "f.txt" :page 3})"#).unwrap();
    let result = interp.eval_str(r#"(vector-store/search "mt" (embedding/list->embedding '(1.0 0.0)) 1)"#).unwrap();
    let meta = result.as_list().unwrap()[0].as_map_rc().unwrap()
        .get(&Value::keyword("metadata")).unwrap().as_map_rc().unwrap();
    assert_eq!(meta.get(&Value::keyword("source")).unwrap().as_str().unwrap(), "f.txt");
}

#[test]
fn test_vector_store_overwrite_id() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "ow")"#).unwrap();
    interp.eval_str(r#"(vector-store/add "ow" "d1" (embedding/list->embedding '(1.0 0.0)) {:v 1})"#).unwrap();
    interp.eval_str(r#"(vector-store/add "ow" "d1" (embedding/list->embedding '(0.0 1.0)) {:v 2})"#).unwrap();
    assert_eq!(interp.eval_str(r#"(vector-store/count "ow")"#).unwrap(), Value::int(1));
}
```

**Run:** `cargo test -p sema --test integration_test -- test_vector_store`
**Expected:** FAIL

### Step 2: Create `vector_store.rs`

Create `crates/sema-llm/src/vector_store.rs`:

```rust
use std::collections::BTreeMap;
use sema_core::Value;

#[derive(Debug, Clone)]
pub struct VectorDocument {
    pub id: String,
    pub embedding: Vec<u8>,  // packed f64, little-endian, 8 bytes per dim
    pub metadata: Value,
}

#[derive(Debug)]
pub struct VectorStore {
    documents: Vec<VectorDocument>,
}

impl VectorStore {
    pub fn new() -> Self { VectorStore { documents: Vec::new() } }

    pub fn add(&mut self, doc: VectorDocument) {
        self.documents.retain(|d| d.id != doc.id);
        self.documents.push(doc);
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.documents.len();
        self.documents.retain(|d| d.id != id);
        self.documents.len() < before
    }

    pub fn count(&self) -> usize { self.documents.len() }

    pub fn search(&self, query: &[u8], k: usize) -> Vec<SearchResult> {
        let mut scored: Vec<SearchResult> = self.documents.iter()
            .filter_map(|doc| {
                let score = cosine_similarity(query, &doc.embedding)?;
                Some(SearchResult { id: doc.id.clone(), score, metadata: doc.metadata.clone() })
            })
            .collect();
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
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
    if a.len() != b.len() || a.is_empty() || a.len() % 8 != 0 { return None; }
    let (mut dot, mut ma, mut mb) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        let fa = f64::from_le_bytes(ca.try_into().ok()?);
        let fb = f64::from_le_bytes(cb.try_into().ok()?);
        dot += fa * fb; ma += fa * fa; mb += fb * fb;
    }
    if ma == 0.0 || mb == 0.0 { Some(0.0) } else { Some(dot / (ma.sqrt() * mb.sqrt())) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn emb(vals: &[f64]) -> Vec<u8> { vals.iter().flat_map(|f| f.to_le_bytes()).collect() }

    #[test]
    fn test_cosine_identical() {
        let a = emb(&[1.0, 0.0]);
        assert!((cosine_similarity(&a, &a).unwrap() - 1.0).abs() < 1e-10);
    }
    #[test]
    fn test_cosine_orthogonal() {
        assert!(cosine_similarity(&emb(&[1.0, 0.0]), &emb(&[0.0, 1.0])).unwrap().abs() < 1e-10);
    }
    #[test]
    fn test_store_crud() {
        let mut s = VectorStore::new();
        s.add(VectorDocument { id: "a".into(), embedding: emb(&[1.0, 0.0]), metadata: Value::nil() });
        assert_eq!(s.count(), 1);
        assert!(s.delete("a"));
        assert_eq!(s.count(), 0);
    }
}
```

### Step 3: Register module in `lib.rs`

Add to `crates/sema-llm/src/lib.rs`: `pub mod vector_store;`

### Step 4: Add thread-local and builtins

In `builtins.rs`, add:

```rust
use crate::vector_store::{VectorStore, VectorDocument};

thread_local! {
    static VECTOR_STORES: RefCell<std::collections::HashMap<String, VectorStore>> =
        RefCell::new(std::collections::HashMap::new());
}
```

Add to `reset_runtime_state()`: `VECTOR_STORES.with(|s| s.borrow_mut().clear());`

Register inside `register_llm_builtins()`:

```rust
register_fn(env, "vector-store/create", |args| {
    if args.len() != 1 { return Err(SemaError::arity("vector-store/create", "1", args.len())); }
    let name = args[0].as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    VECTOR_STORES.with(|s| s.borrow_mut().insert(name.to_string(), VectorStore::new()));
    Ok(Value::string(name))
});

register_fn(env, "vector-store/add", |args| {
    if args.len() != 4 { return Err(SemaError::arity("vector-store/add", "4", args.len())); }
    let name = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let id = args[1].as_str().ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
    let emb = args[2].as_bytevector().ok_or_else(|| SemaError::type_error("bytevector", args[2].type_name()))?;
    if emb.len() % 8 != 0 {
        return Err(SemaError::eval(format!("vector-store/add: embedding length {} not multiple of 8", emb.len())));
    }
    let metadata = args[3].clone();
    VECTOR_STORES.with(|s| {
        let mut s = s.borrow_mut();
        let store = s.get_mut(name).ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
        store.add(VectorDocument { id: id.to_string(), embedding: emb.to_vec(), metadata });
        Ok(Value::string(id))
    })
});

register_fn(env, "vector-store/search", |args| {
    if args.len() != 3 { return Err(SemaError::arity("vector-store/search", "3", args.len())); }
    let name = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let query = args[1].as_bytevector().ok_or_else(|| SemaError::type_error("bytevector", args[1].type_name()))?;
    let k = args[2].as_int().ok_or_else(|| SemaError::type_error("integer", args[2].type_name()))? as usize;
    VECTOR_STORES.with(|s| {
        let s = s.borrow();
        let store = s.get(name).ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
        Ok(Value::list(store.search(query, k).iter().map(|r| r.to_value()).collect()))
    })
});

register_fn(env, "vector-store/delete", |args| {
    if args.len() != 2 { return Err(SemaError::arity("vector-store/delete", "2", args.len())); }
    let name = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let id = args[1].as_str().ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
    VECTOR_STORES.with(|s| {
        let mut s = s.borrow_mut();
        let store = s.get_mut(name).ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
        Ok(Value::bool(store.delete(id)))
    })
});

register_fn(env, "vector-store/count", |args| {
    if args.len() != 1 { return Err(SemaError::arity("vector-store/count", "1", args.len())); }
    let name = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    VECTOR_STORES.with(|s| {
        let s = s.borrow();
        let store = s.get(name).ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
        Ok(Value::int(store.count() as i64))
    })
});
```

### Step 5: Run tests, commit

**Run:** `cargo test -p sema-llm -- vector_store`
**Run:** `cargo test -p sema --test integration_test -- test_vector_store`
**Expected:** PASS

```bash
git add crates/sema-llm/src/vector_store.rs crates/sema-llm/src/lib.rs crates/sema-llm/src/builtins.rs crates/sema/tests/integration_test.rs
git commit -m "feat(llm): add in-memory vector store with cosine similarity search"
```

---

## Task 9: Vector Math Utilities

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`
- Test: `crates/sema/tests/integration_test.rs`

### Overview

Standalone vector math on packed f64 bytevectors: `vector/cosine-similarity`,
`vector/dot-product`, `vector/normalize`, `vector/distance` (Euclidean).

### Step 1: Write failing tests

```rust
#[test]
fn test_vector_cosine_similarity() {
    let r = eval(r#"(vector/cosine-similarity (embedding/list->embedding '(1.0 0.0)) (embedding/list->embedding '(1.0 0.0)))"#);
    assert!((r.as_float().unwrap() - 1.0).abs() < 1e-10);
}

#[test]
fn test_vector_cosine_orthogonal() {
    let r = eval(r#"(vector/cosine-similarity (embedding/list->embedding '(1.0 0.0)) (embedding/list->embedding '(0.0 1.0)))"#);
    assert!(r.as_float().unwrap().abs() < 1e-10);
}

#[test]
fn test_vector_dot_product() {
    let r = eval(r#"(vector/dot-product (embedding/list->embedding '(1.0 2.0 3.0)) (embedding/list->embedding '(4.0 5.0 6.0)))"#);
    assert!((r.as_float().unwrap() - 32.0).abs() < 1e-10);
}

#[test]
fn test_vector_normalize() {
    let r = eval(r#"(vector/normalize (embedding/list->embedding '(3.0 4.0)))"#);
    let bv = r.as_bytevector().unwrap();
    let x = f64::from_le_bytes(bv[0..8].try_into().unwrap());
    let y = f64::from_le_bytes(bv[8..16].try_into().unwrap());
    assert!((x - 0.6).abs() < 1e-10);
    assert!((y - 0.8).abs() < 1e-10);
}

#[test]
fn test_vector_normalize_zero() {
    let r = eval(r#"(vector/normalize (embedding/list->embedding '(0.0 0.0)))"#);
    let bv = r.as_bytevector().unwrap();
    assert!(f64::from_le_bytes(bv[0..8].try_into().unwrap()).abs() < 1e-10);
}

#[test]
fn test_vector_distance() {
    let r = eval(r#"(vector/distance (embedding/list->embedding '(0.0 0.0)) (embedding/list->embedding '(3.0 4.0)))"#);
    assert!((r.as_float().unwrap() - 5.0).abs() < 1e-10);
}

#[test]
fn test_vector_distance_same() {
    let r = eval(r#"(vector/distance (embedding/list->embedding '(1.0 2.0)) (embedding/list->embedding '(1.0 2.0)))"#);
    assert!(r.as_float().unwrap().abs() < 1e-10);
}

#[test]
fn test_vector_dimension_mismatch_error() {
    let interp = Interpreter::new();
    assert!(interp.eval_str(r#"(vector/dot-product (embedding/list->embedding '(1.0 2.0)) (embedding/list->embedding '(1.0 2.0 3.0)))"#).is_err());
}
```

### Step 2: Implement

Add helper and register in `builtins.rs`:

```rust
fn require_matching_bytevectors<'a>(name: &str, args: &'a [Value]) -> Result<(&'a [u8], &'a [u8]), SemaError> {
    if args.len() != 2 { return Err(SemaError::arity(name, "2", args.len())); }
    let a = args[0].as_bytevector().ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
    let b = args[1].as_bytevector().ok_or_else(|| SemaError::type_error("bytevector", args[1].type_name()))?;
    if a.len() != b.len() { return Err(SemaError::eval(format!("{name}: length mismatch ({} vs {})", a.len()/8, b.len()/8))); }
    if a.is_empty() || a.len() % 8 != 0 { return Err(SemaError::eval(format!("{name}: invalid bytevector length {}", a.len()))); }
    Ok((a, b))
}

// vector/cosine-similarity
register_fn(env, "vector/cosine-similarity", |args| {
    let (a, b) = require_matching_bytevectors("vector/cosine-similarity", args)?;
    let (mut dot, mut ma, mut mb) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        let (fa, fb) = (f64::from_le_bytes(ca.try_into().unwrap()), f64::from_le_bytes(cb.try_into().unwrap()));
        dot += fa * fb; ma += fa * fa; mb += fb * fb;
    }
    Ok(Value::float(if ma == 0.0 || mb == 0.0 { 0.0 } else { dot / (ma.sqrt() * mb.sqrt()) }))
});

// vector/dot-product
register_fn(env, "vector/dot-product", |args| {
    let (a, b) = require_matching_bytevectors("vector/dot-product", args)?;
    let mut dot = 0.0_f64;
    for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        dot += f64::from_le_bytes(ca.try_into().unwrap()) * f64::from_le_bytes(cb.try_into().unwrap());
    }
    Ok(Value::float(dot))
});

// vector/normalize
register_fn(env, "vector/normalize", |args| {
    if args.len() != 1 { return Err(SemaError::arity("vector/normalize", "1", args.len())); }
    let bv = args[0].as_bytevector().ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
    if bv.is_empty() || bv.len() % 8 != 0 { return Err(SemaError::eval("vector/normalize: invalid bytevector")); }
    let floats: Vec<f64> = bv.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect();
    let mag: f64 = floats.iter().map(|f| f * f).sum::<f64>().sqrt();
    let out: Vec<u8> = if mag == 0.0 {
        floats.iter().flat_map(|_| 0.0_f64.to_le_bytes()).collect()
    } else {
        floats.iter().flat_map(|f| (f / mag).to_le_bytes()).collect()
    };
    Ok(Value::bytevector(out))
});

// vector/distance (Euclidean)
register_fn(env, "vector/distance", |args| {
    let (a, b) = require_matching_bytevectors("vector/distance", args)?;
    let mut sum_sq = 0.0_f64;
    for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        let d = f64::from_le_bytes(ca.try_into().unwrap()) - f64::from_le_bytes(cb.try_into().unwrap());
        sum_sq += d * d;
    }
    Ok(Value::float(sum_sq.sqrt()))
});
```

### Step 3: Run tests, commit

**Run:** `cargo test -p sema --test integration_test -- test_vector_cosine test_vector_dot test_vector_normalize test_vector_distance test_vector_dimension`
**Expected:** PASS

```bash
git add crates/sema-llm/src/builtins.rs crates/sema/tests/integration_test.rs
git commit -m "feat(llm): add vector math — cosine-similarity, dot-product, normalize, distance"
```
