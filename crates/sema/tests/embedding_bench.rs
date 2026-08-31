// Benchmark: embeddings as dedicated f64 vectors vs Value::List(Vec<Value::Float>).

use std::rc::Rc;
use std::time::Instant;

use sema_core::Value;

/// Simulate a typical embedding vector (e.g., OpenAI text-embedding-3-small = 1536 dims,
/// text-embedding-3-large = 3072 dims, Cohere = 1024 dims).
fn make_f64_vec(dims: usize) -> Vec<f64> {
    (0..dims).map(|i| (i as f64 * 0.001).sin()).collect()
}

/// Current approach: List of Value::Float
fn embedding_as_value_list(data: &[f64]) -> Value {
    Value::list(data.iter().map(|&f| Value::float(f)).collect())
}

/// Alternative A: Bytevector storing IEEE 754 f64 bytes (little-endian)
fn embedding_as_bytevector(data: &[f64]) -> Value {
    let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
    Value::bytevector(bytes)
}

/// Extract f64 values from a Value::List (current approach)
fn extract_from_list(val: &Value) -> Vec<f64> {
    val.as_list()
        .map(|l| l.iter().map(|v| v.as_float().unwrap()).collect())
        .expect("expected list")
}

/// Extract f64 values from a bytevector
fn extract_from_bytevector(val: &Value) -> Vec<f64> {
    val.as_bytevector()
        .map(|bv| {
            bv.as_chunks::<8>()
                .0
                .iter()
                .map(|chunk| f64::from_le_bytes(*chunk))
                .collect()
        })
        .expect("expected bytevector")
}

/// Cosine similarity on Value::List (current approach — extracts then computes)
fn similarity_from_list(a: &Value, b: &Value) -> f64 {
    let va = extract_from_list(a);
    let vb = extract_from_list(b);
    cosine_similarity(&va, &vb)
}

/// Cosine similarity on bytevector (zero-copy iteration)
fn similarity_from_bytevector(a: &Value, b: &Value) -> f64 {
    let ba = a.as_bytevector().unwrap();
    let bb = b.as_bytevector().unwrap();
    let mut dot = 0.0_f64;
    let mut mag_a = 0.0_f64;
    let mut mag_b = 0.0_f64;
    let chunks_a = ba.as_chunks::<8>().0;
    let chunks_b = bb.as_chunks::<8>().0;
    for (ca, cb) in chunks_a.iter().zip(chunks_b) {
        let fa = f64::from_le_bytes(*ca);
        let fb = f64::from_le_bytes(*cb);
        dot += fa * fb;
        mag_a += fa * fa;
        mag_b += fb * fb;
    }
    dot / (mag_a.sqrt() * mag_b.sqrt())
}

fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let mag_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    dot / (mag_a * mag_b)
}

/// Fused single-pass cosine similarity on Vec<f64>
fn cosine_similarity_fused(a: &[f64], b: &[f64]) -> f64 {
    let mut dot = 0.0_f64;
    let mut mag_a = 0.0_f64;
    let mut mag_b = 0.0_f64;
    for i in 0..a.len() {
        let x = a[i];
        let y = b[i];
        dot += x * y;
        mag_a += x * x;
        mag_b += y * y;
    }
    dot / (mag_a.sqrt() * mag_b.sqrt())
}

fn bench<F: Fn() -> R, R>(name: &str, iterations: usize, f: F) -> std::time::Duration {
    // Warmup
    for _ in 0..iterations / 10 {
        std::hint::black_box(f());
    }
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(f());
    }
    let elapsed = start.elapsed();
    let per_iter = elapsed / iterations as u32;
    println!("  {name}: {per_iter:?}/iter ({iterations} iters, total {elapsed:?})");
    elapsed
}

#[test]
#[ignore = "expensive benchmark test; run with `jake test.embedding-bench`"]
fn bench_embedding_representations() {
    println!("\n=== EMBEDDING REPRESENTATION BENCHMARK ===\n");

    for dims in [768, 1024, 1536, 3072] {
        println!("--- {dims} dimensions ---");
        let raw = make_f64_vec(dims);
        let raw2: Vec<f64> = raw.iter().map(|x| x + 0.1).collect();

        // === 1. Memory size ===
        let value_size = std::mem::size_of::<Value>();
        let list_val = embedding_as_value_list(&raw);
        let bv_val = embedding_as_bytevector(&raw);

        // List: Rc<Vec<Value>> = 1 Rc alloc + Vec of N Values (each value_size bytes)
        // The Vec heap allocation is N * value_size
        let list_heap = dims * value_size + std::mem::size_of::<Vec<Value>>();
        // Bytevector: Rc<Vec<u8>> = 1 Rc alloc + N*8 bytes
        let bv_heap = dims * 8 + std::mem::size_of::<Vec<u8>>();

        println!("  Value enum size: {value_size} bytes");
        println!(
            "  List<Float> heap:  {list_heap} bytes ({:.1} KB)",
            list_heap as f64 / 1024.0
        );
        println!(
            "  Bytevector heap:   {bv_heap} bytes ({:.1} KB)",
            bv_heap as f64 / 1024.0
        );
        println!(
            "  Ratio: {:.1}x smaller with bytevector",
            list_heap as f64 / bv_heap as f64
        );

        let iters = 10_000;

        // === 2. Construction time ===
        println!("\n  Construction:");
        let t_list = bench("  List<Float>", iters, || embedding_as_value_list(&raw));
        let t_bv = bench("  Bytevector ", iters, || embedding_as_bytevector(&raw));
        println!(
            "  Speedup: {:.1}x",
            t_list.as_nanos() as f64 / t_bv.as_nanos() as f64
        );

        // === 3. Clone time (what happens when you pass embeddings around) ===
        println!("\n  Clone (Rc::clone, shallow):");
        let list_val_c = list_val.clone();
        let bv_val_c = bv_val.clone();
        bench("  List<Float>", iters * 100, || list_val_c.clone());
        bench("  Bytevector ", iters * 100, || bv_val_c.clone());

        // === 4. Extraction to Vec<f64> ===
        println!("\n  Extract to Vec<f64>:");
        let t_ex_list = bench("  List<Float>", iters, || extract_from_list(&list_val));
        let t_ex_bv = bench("  Bytevector ", iters, || extract_from_bytevector(&bv_val));
        println!(
            "  Speedup: {:.1}x",
            t_ex_list.as_nanos() as f64 / t_ex_bv.as_nanos() as f64
        );

        // === 5. Cosine similarity (the main operation) ===
        let list_val2 = embedding_as_value_list(&raw2);
        let bv_val2 = embedding_as_bytevector(&raw2);

        println!("\n  Cosine similarity:");
        let t_sim_list = bench("  List<Float>", iters, || {
            similarity_from_list(&list_val, &list_val2)
        });
        let t_sim_bv = bench("  Bytevector ", iters, || {
            similarity_from_bytevector(&bv_val, &bv_val2)
        });
        println!(
            "  Speedup: {:.1}x",
            t_sim_list.as_nanos() as f64 / t_sim_bv.as_nanos() as f64
        );

        // Verify correctness
        let sim_list = similarity_from_list(&list_val, &list_val2);
        let sim_bv = similarity_from_bytevector(&bv_val, &bv_val2);
        assert!(
            (sim_list - sim_bv).abs() < 1e-10,
            "Results differ: {sim_list} vs {sim_bv}"
        );

        // === 6. Batch scenario: storing 1000 embeddings (like a vector store) ===
        println!("\n  Store 1000 embeddings:");
        let t_store_list = bench("  List<Float>", 10, || {
            let store: Vec<Value> = (0..1000)
                .map(|i| {
                    let v: Vec<f64> = (0..dims)
                        .map(|j| ((i * dims + j) as f64 * 0.001).sin())
                        .collect();
                    embedding_as_value_list(&v)
                })
                .collect();
            store
        });
        let t_store_bv = bench("  Bytevector ", 10, || {
            let store: Vec<Value> = (0..1000)
                .map(|i| {
                    let v: Vec<f64> = (0..dims)
                        .map(|j| ((i * dims + j) as f64 * 0.001).sin())
                        .collect();
                    embedding_as_bytevector(&v)
                })
                .collect();
            store
        });
        println!(
            "  Speedup: {:.1}x",
            t_store_list.as_nanos() as f64 / t_store_bv.as_nanos() as f64
        );

        // === 7. kNN search: find most similar among 1000 embeddings ===
        let store_list: Vec<Value> = (0..1000)
            .map(|i| {
                let v: Vec<f64> = (0..dims)
                    .map(|j| ((i * dims + j) as f64 * 0.001).sin())
                    .collect();
                embedding_as_value_list(&v)
            })
            .collect();
        let store_bv: Vec<Value> = (0..1000)
            .map(|i| {
                let v: Vec<f64> = (0..dims)
                    .map(|j| ((i * dims + j) as f64 * 0.001).sin())
                    .collect();
                embedding_as_bytevector(&v)
            })
            .collect();
        let query_list = embedding_as_value_list(&raw);
        let query_bv = embedding_as_bytevector(&raw);

        println!("\n  kNN search (1000 embeddings):");
        let t_knn_list = bench("  List<Float>", 100, || {
            store_list
                .iter()
                .map(|e| similarity_from_list(&query_list, e))
                .collect::<Vec<_>>()
        });
        let t_knn_bv = bench("  Bytevector ", 100, || {
            store_bv
                .iter()
                .map(|e| similarity_from_bytevector(&query_bv, e))
                .collect::<Vec<_>>()
        });
        println!(
            "  Speedup: {:.1}x",
            t_knn_list.as_nanos() as f64 / t_knn_bv.as_nanos() as f64
        );

        println!();
    }

    // === Bonus: What about a proper FloatVector type? ===
    println!("--- Alternative C: Hypothetical Value::FloatVector(Rc<Vec<f64>>) ---");
    println!("  (Simulated — not adding to Value enum, just measuring raw Vec<f64>)\n");

    for dims in [1536, 3072] {
        println!("  {dims} dims:");
        let raw = make_f64_vec(dims);
        let raw2: Vec<f64> = raw.iter().map(|x| x + 0.1).collect();

        // FloatVector would be Rc<Vec<f64>> — construction is just wrapping
        let fv = Rc::new(raw.clone());
        let fv2 = Rc::new(raw2.clone());
        let bv_val = embedding_as_bytevector(&raw);
        let bv_val2 = embedding_as_bytevector(&raw2);
        let list_val = embedding_as_value_list(&raw);
        let list_val2 = embedding_as_value_list(&raw2);

        let iters = 10_000;

        // Construction
        println!("\n  Construction:");
        bench("    List<Float>   ", iters, || {
            embedding_as_value_list(&raw)
        });
        bench("    Bytevector    ", iters, || {
            embedding_as_bytevector(&raw)
        });
        bench("    Rc<Vec<f64>>  ", iters, || Rc::new(raw.clone()));

        // Similarity: FloatVector is zero-extraction
        println!("\n  Cosine similarity:");
        bench("    List<Float>   ", iters, || {
            similarity_from_list(&list_val, &list_val2)
        });
        bench("    Bytevector    ", iters, || {
            similarity_from_bytevector(&bv_val, &bv_val2)
        });
        bench("    Vec<f64> 3pass", iters, || cosine_similarity(&fv, &fv2));
        bench("    Vec<f64> fused", iters, || {
            cosine_similarity_fused(&fv, &fv2)
        });

        // Memory
        let list_heap = dims * std::mem::size_of::<Value>() + std::mem::size_of::<Vec<Value>>();
        let bv_heap = dims * 8 + std::mem::size_of::<Vec<u8>>();
        let fv_heap = dims * 8 + std::mem::size_of::<Vec<f64>>();
        println!("\n  Memory:");
        println!("    List<Float>:  {:.1} KB", list_heap as f64 / 1024.0);
        println!("    Bytevector:   {:.1} KB", bv_heap as f64 / 1024.0);
        println!("    Vec<f64>:     {:.1} KB", fv_heap as f64 / 1024.0);
        println!();
    }
}
