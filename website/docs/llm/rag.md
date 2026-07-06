---
title: "RAG: retrieve, rerank, answer"
description: Build a retrieval-augmented generation pipeline in Sema — embeddings, vector search, cross-encoder reranking, and a grounded answer.
---

# RAG: retrieve, rerank, answer

Retrieval-Augmented Generation (RAG) answers a question by first **finding the relevant documents** and then asking the model to answer **using only those documents**. It's how you get grounded, citable answers over a corpus the model was never trained on — your docs, your codebase, your knowledge base.

Sema has the whole pipeline as first-class primitives:

| Step | Primitive | What it does |
| --- | --- | --- |
| **Embed** | `llm/embed` | Turn text into vectors (a *bi-encoder* — query and document embedded independently) |
| **Retrieve** | `vector-store/*` | Cosine nearest-neighbour search over those vectors |
| **Rerank** | `llm/rerank` | A *cross-encoder* reorders the candidates by reading query + document together |
| **Answer** | `llm/complete` | Generate an answer grounded in the top reranked documents |

The recipe everyone converges on is **retrieve many, rerank to a few**. Vector search has high recall but coarse ordering — because the query and each document are embedded *separately*, the score can't model how they interact. A reranker reads them *together*, so it's far more precise. You retrieve a generous shortlist by cosine (say top 12), then let the reranker pick the best 4.

This guide builds a working example that indexes Sema's **own builtin documentation** and answers "which function do I use?" questions. The full file is [`examples/llm/rag-docs-search.sema`](https://github.com/sema-lisp/sema/blob/main/examples/llm/rag-docs-search.sema).

## Setup

You need two kinds of key in your environment:

- An **embedding + rerank** provider: `JINA_API_KEY`, `VOYAGE_API_KEY`, or `COHERE_API_KEY`. All three offer both embeddings and reranking from the same key, and Sema auto-configures them on startup.
- A **chat** provider for the final answer: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, …

## 1. Index the corpus

Read each document, embed it, and add the vector to a store. Two things make this fast and cheap:

- **Batch the embeddings.** `llm/embed` takes a *list* and returns a list of vectors, so you embed many documents per network call.
- **Cache the store to disk.** `vector-store/open` loads a saved store if the file exists (and starts empty otherwise), so you only pay to index once.

```sema
(define (build-index!)
  (let* ((files (file/glob "crates/sema-docs/entries/stdlib/**/*.md"))
         (docs  (map (lambda (p)
                       {:name (path/stem p) :path p
                        :text (string/take (file/read p) 900)})  ; bounds-safe truncate
                     files))
         ;; Embed in batches: list/chunk splits into 64s, flat-map runs one call per
         ;; batch and flattens the per-batch vector lists back into one.
         (vecs  (flat-map (lambda (b) (llm/embed b))
                          (list/chunk 64 (map (lambda (d) (:text d)) docs)))))
    ;; map walks docs + vecs in lockstep — Scheme-style multi-list map.
    (map (lambda (doc vec) (vector-store/add "docs" (:name doc) vec doc))
         docs vecs)
    (vector-store/save "docs")))

(vector-store/open "docs" "/tmp/sema-docs.vec")
(when (= (vector-store/count "docs") 0) (build-index!))
```

This leans entirely on stdlib: `string/take` truncates safely (no manual length check), `list/chunk` batches a list into fixed-size groups, `flat-map` maps-then-flattens, and `map` walks several lists in lockstep. We store the whole document map as the vector's metadata (the 4th argument to `vector-store/add`) — that's how we get the text back at query time.

## 2. Retrieve

Embed the question and pull a generous shortlist by cosine similarity. The query embedding must come from the same model as the stored vectors — which it does, since we use the same configured provider.

```sema
(define question "How do I read a file from disk and split it into lines?")
(define query-vec (llm/embed question))
(define candidates (vector-store/search "docs" query-vec 12))
```

Each candidate is a map `{:id :score :metadata}`, sorted by cosine score.

## 3. Rerank

Pull the document text out of each candidate's metadata and let the cross-encoder reorder them to the best 4:

```sema
(define candidate-texts (map (lambda (c) (:text (:metadata c))) candidates))
(define reranked (llm/rerank question candidate-texts {:top-k 4}))
```

`llm/rerank` returns `{:index :score :document}` maps, highest relevance first. `:index` points back into the list you passed in, so you can recover the original candidate (and its id/metadata):

```sema
(for-each (lambda (r)
            (let ((name  (:id (nth candidates (:index r))))
                  (score (math/round-to (:score r) 3)))
              (println f"  ${score}  ${name}")))
          reranked)
```

```
  0.467  file-read-lines
  0.304  read-line
  0.293  file-for-each-line
  0.239  io-read-line
```

The reranker pushed `file-read-lines` to the top — exactly the function the question is about. Treat the scores as an *ordering*, not a calibrated probability: they're query-dependent, so 0.47 isn't "twice as relevant" as 0.24.

## 4. Answer

Concatenate the top documents and instruct the model to answer using only them:

```sema
(define context
  (string/join (map (lambda (r) (nth candidate-texts (:index r))) reranked)
               "\n\n---\n\n"))

(define prompt
  f"Using ONLY the Sema documentation below, answer the question and name the exact functions to call.\n\nDOCS:\n${context}\n\nQUESTION: ${question}")

(println (llm/complete prompt {:max-tokens 400}))
```

> **Reading a File as Lines** — Use `file/read-lines` to read a file from disk and get back a list of lines directly. For large files, use `file/for-each-line` to iterate without loading everything into memory.

Grounded, correct, and citable — the model only saw the four documents the reranker chose.

## Choosing a reranker

`llm/rerank` uses your configured rerank provider; override per call with `:provider` (a keyword like `:cohere` or the string `"cohere"`) and `:model`.

| Provider | `:provider` | Default model | Billing |
| --- | --- | --- | --- |
| Cohere | `:cohere` | `rerank-v3.5` | per search (flat per call) |
| Jina | `:jina` | `jina-reranker-v2-base-multilingual` | per token |
| Voyage | `:voyage` | `rerank-2.5` | per token |

```sema
(llm/rerank query docs {:top-k 5 :provider :voyage :model "rerank-2.5"})
```

Override `:model` for a newer version, multilingual support, or to trade cost for quality — check the provider's docs for current model names.

## Scores, top-k, and cost

**Scores rank; they don't threshold.** The `:score` is the provider's raw relevance score — query-dependent, *not* a calibrated probability, and on a different scale for each of Cohere, Jina, and Voyage. Use scores to order results *within a single rerank call*; don't compare them across providers or read a fixed cutoff as meaningful. If everything comes back with uniformly low scores, that's a signal the query and corpus don't match — not that the reranker failed.

**Choosing top-k.** `:top-k` is how many of the best documents to keep — typically **3–10** for RAG, sized so the kept documents fit comfortably in the answer prompt's context window. Omit `:top-k` to rerank and return *all* documents in relevance order.

**Cost and latency scale with the candidate set.** A reranker scores *every* candidate against the query, so cost and latency grow with the number of candidates and their length (Cohere bills per search, Jina/Voyage per token). That's why you **retrieve-then-rerank** — pull a shortlist with cheap vector search (say top-20), then rerank to the top-k — instead of reranking the whole corpus. Reranking is a *refinement* on a shortlist, never a standalone search over everything.

## Error handling

```sema
(try
  (llm/rerank query candidates {:top-k 5})
  (catch e (println "rerank failed:" (:message e))))
```

- An **empty document list** returns `()` immediately, with no API call.
- **API / network / rate-limit / invalid-model** failures raise a `SemaError` (catch with `try`).
- An **unknown `:provider`** — or no rerank provider configured at all — raises a "rerank provider not found" error. Set `COHERE_API_KEY` / `JINA_API_KEY` / `VOYAGE_API_KEY`, or pass `:provider` explicitly.

## Observability

With [OpenTelemetry](/docs/llm/observability) on and a compat backend selected, the retrieve and rerank steps emit OpenInference `RETRIEVER` and `RERANKER` spans — the reranker span carries the model name, `top-k`, and (with [content capture](/docs/llm/otel-compat) enabled) the reordered documents and their scores. A full RAG trace renders natively in Phoenix/Arize alongside the embedding and chat spans, which makes "why did this answer cite the wrong doc?" debuggable end to end.

## When do you actually need a reranker?

Reranking is the highest-leverage, lowest-effort quality lever in RAG — but it isn't free, so it's worth knowing when it pays off. The honest test is to **A/B it**: measure answer quality (and added latency) with retrieve-only vs. retrieve-then-rerank on your own queries.

**Reach for it when** cosine top-k returns *roughly* relevant results but the *ordering* is off, your documents are long or ambiguously worded (where a bi-encoder's single vector blurs detail a cross-encoder recovers), or you retrieve a large shortlist and need to trim it to what fits the prompt.

**You can skip it when** the corpus is small, your embedding model already nails ordering for your queries, or context-window space isn't a constraint — `vector-store/search` alone is a complete retriever.
