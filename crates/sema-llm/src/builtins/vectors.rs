use super::*;

pub(super) fn register(env: &Env) {
    register_fn(env, "vector-store/create", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("vector-store/create", "1", args.len()));
        }
        let name = args.str_at(0, "vector-store/create")?;
        VECTOR_STORES.with(|s| s.borrow_mut().insert(name.to_string(), VectorStore::new()));
        Ok(Value::string(name))
    });

    register_fn(env, "vector-store/add", |args| {
        if args.len() != 4 {
            return Err(SemaError::arity("vector-store/add", "4", args.len()));
        }
        let name = args.str_at(0, "vector-store/add")?;
        let id = args.str_at(1, "vector-store/add")?;
        let emb = args.bytes_at(2, "vector-store/add")?;
        if emb.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "vector-store/add: embedding length {} not multiple of 8",
                emb.len()
            )));
        }
        let metadata = args[3].clone();
        VECTOR_STORES.with(|s| {
            let mut s = s.borrow_mut();
            let store = s
                .get_mut(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            store.add(VectorDocument {
                id: id.to_string(),
                embedding: emb.to_vec(),
                metadata,
            });
            Ok(Value::string(id))
        })
    });

    register_fn(env, "vector-store/search", |args| {
        if args.len() != 3 {
            return Err(SemaError::arity("vector-store/search", "3", args.len()));
        }
        let name = args.str_at(0, "vector-store/search")?;
        let query = args.bytes_at(1, "vector-store/search")?;
        let k = args.int_at(2, "vector-store/search")? as usize;
        // OpenInference RETRIEVER span (no-op unless telemetry + compat are on).
        let span = sema_otel::retriever_span(query.len() / 8, k);
        VECTOR_STORES.with(|s| {
            let s = s.borrow();
            let store = s
                .get(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            let results = store.search(query, k).inspect_err(|e| {
                span.record_error("retrieval_error", &e.to_string());
            })?;
            // (id, content, score) for the span — content pulled from metadata :text/:content.
            let docs: Vec<(String, String, f64)> = results
                .iter()
                .map(|r| (r.id.clone(), metadata_text(&r.metadata), r.score))
                .collect();
            span.set_documents(&docs);
            Ok(Value::list(results.iter().map(|r| r.to_value()).collect()))
        })
    });

    register_fn(env, "vector-store/delete", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("vector-store/delete", "2", args.len()));
        }
        let name = args.str_at(0, "vector-store/delete")?;
        let id = args.str_at(1, "vector-store/delete")?;
        VECTOR_STORES.with(|s| {
            let mut s = s.borrow_mut();
            let store = s
                .get_mut(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            Ok(Value::bool(store.delete(id)))
        })
    });

    register_fn(env, "vector-store/count", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("vector-store/count", "1", args.len()));
        }
        let name = args.str_at(0, "vector-store/count")?;
        VECTOR_STORES.with(|s| {
            let s = s.borrow();
            let store = s
                .get(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            Ok(Value::int(store.count() as i64))
        })
    });

    // (vector-store/save name) or (vector-store/save name path)
    register_fn(env, "vector-store/save", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("vector-store/save", "1-2", args.len()));
        }
        let name = args.str_at(0, "vector-store/save")?;
        let explicit_path = args.get(1).and_then(|v| v.as_str()).map(|s| s.to_string());
        VECTOR_STORES.with(|s| {
            let s = s.borrow();
            let store = s
                .get(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            let path = explicit_path
                .as_deref()
                .or(store.path.as_deref())
                .ok_or_else(|| {
                    SemaError::eval(
                        "vector-store/save: no path associated. Use (vector-store/save name path)",
                    )
                })?;
            let data = store.to_json().map_err(SemaError::Io)?;
            let tmp = format!("{path}.tmp");
            std::fs::write(&tmp, &data).io_ctx("vector-store/save")?;
            std::fs::rename(&tmp, path).io_ctx("vector-store/save")?;
            Ok(Value::string(path))
        })
    });

    // (vector-store/open name path) — load from disk or create empty, associate path
    register_fn(env, "vector-store/open", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("vector-store/open", "2", args.len()));
        }
        let name = args.str_at(0, "vector-store/open")?;
        let path = args.str_at(1, "vector-store/open")?;
        let mut store = if std::path::Path::new(path).exists() {
            let data = std::fs::read(path).io_ctx("vector-store/open")?;
            VectorStore::from_json(&data).io_ctx("vector-store/open")?
        } else {
            VectorStore::new()
        };
        store.path = Some(path.to_string());
        VECTOR_STORES.with(|s| s.borrow_mut().insert(name.to_string(), store));
        Ok(Value::string(name))
    });

    // --- Vector math builtins ---

    register_fn(env, "vector/cosine-similarity", |args| {
        let (a, b) = require_matching_bytevectors("vector/cosine-similarity", args)?;
        let (mut dot, mut ma, mut mb) = (0.0_f64, 0.0_f64, 0.0_f64);
        for (ca, cb) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
            let (fa, fb) = (f64::from_le_bytes(*ca), f64::from_le_bytes(*cb));
            dot += fa * fb;
            ma += fa * fa;
            mb += fb * fb;
        }
        Ok(Value::float(if ma == 0.0 || mb == 0.0 {
            0.0
        } else {
            dot / (ma.sqrt() * mb.sqrt())
        }))
    });

    register_fn(env, "vector/dot-product", |args| {
        let (a, b) = require_matching_bytevectors("vector/dot-product", args)?;
        let mut dot = 0.0_f64;
        for (ca, cb) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
            dot += f64::from_le_bytes(*ca) * f64::from_le_bytes(*cb);
        }
        Ok(Value::float(dot))
    });

    register_fn(env, "vector/normalize", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("vector/normalize", "1", args.len()));
        }
        let bv = args.bytes_at(0, "vector/normalize")?;
        if bv.is_empty() || bv.len() % 8 != 0 {
            return Err(SemaError::eval("vector/normalize: invalid bytevector"));
        }
        let floats: Vec<f64> = bv
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| f64::from_le_bytes(*c))
            .collect();
        let mag: f64 = floats.iter().map(|f| f * f).sum::<f64>().sqrt();
        let out: Vec<u8> = if mag == 0.0 {
            floats.iter().flat_map(|_| 0.0_f64.to_le_bytes()).collect()
        } else {
            floats
                .iter()
                .flat_map(|f| (f / mag).to_le_bytes())
                .collect()
        };
        Ok(Value::bytevector(out))
    });

    register_fn(env, "vector/distance", |args| {
        let (a, b) = require_matching_bytevectors("vector/distance", args)?;
        let mut sum_sq = 0.0_f64;
        for (ca, cb) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
            let d = f64::from_le_bytes(*ca) - f64::from_le_bytes(*cb);
            sum_sq += d * d;
        }
        Ok(Value::float(sum_sq.sqrt()))
    });

    // --- Rate limiting ---
}
