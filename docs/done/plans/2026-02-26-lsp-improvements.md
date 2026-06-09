# LSP Improvements Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add E2E tests for new LSP features, completion documentation, VS Code initializationOptions, document highlight, trim Helix highlights, and plan cross-file references/rename + semantic tokens range for later.

**Architecture:** The LSP uses a single-threaded backend (`BackendState`) communicating with the async tower-lsp layer via mpsc channel. All state (documents, parse caches, scope trees, import caches) lives in `BackendState`. E2E tests use pytest-lsp connecting to the real server binary.

**Tech Stack:** Rust (tower-lsp), Python (pytest-lsp), TypeScript (VS Code extension), Tree-sitter queries (Helix)

---

## Task 1: E2E tests for semantic tokens

**Files:**
- Create: `crates/sema-lsp/tests/e2e/test_semantic_tokens.py`

**Step 1: Write the test file**

```python
"""Test textDocument/semanticTokens/full."""

import pytest
from lsprotocol.types import (
    SemanticTokensParams,
    TextDocumentIdentifier,
)
from pytest_lsp import LanguageClient

from helpers import open_doc


@pytest.mark.asyncio
async def test_semantic_tokens_returns_data(client: LanguageClient):
    """Semantic tokens should return token data for a document."""
    uri = await open_doc(client, "(define x 42)\n(defun foo (a) (+ a x))")
    result = await client.text_document_semantic_tokens_full_async(
        SemanticTokensParams(
            text_document=TextDocumentIdentifier(uri=uri),
        )
    )
    assert result is not None
    assert len(result.data) > 0
    # Data comes in groups of 5 (deltaLine, deltaStart, length, tokenType, modifiers)
    assert len(result.data) % 5 == 0


@pytest.mark.asyncio
async def test_semantic_tokens_classifies_keyword(client: LanguageClient):
    """Special forms should be classified as keyword (type index 0)."""
    uri = await open_doc(client, "(define x 42)")
    result = await client.text_document_semantic_tokens_full_async(
        SemanticTokensParams(
            text_document=TextDocumentIdentifier(uri=uri),
        )
    )
    assert result is not None
    # First token should be 'define' at (0,1) — keyword type = 0
    tokens = result.data
    assert len(tokens) >= 5
    # tokenType is at index 3 in each group of 5
    assert tokens[3] == 0  # KEYWORD


@pytest.mark.asyncio
async def test_semantic_tokens_empty_doc(client: LanguageClient):
    """Empty document should return None or empty tokens."""
    uri = await open_doc(client, "")
    result = await client.text_document_semantic_tokens_full_async(
        SemanticTokensParams(
            text_document=TextDocumentIdentifier(uri=uri),
        )
    )
    # Either None or empty data is acceptable
    if result is not None:
        assert len(result.data) == 0
```

**Step 2: Run test to verify it passes**

Run: `cd crates/sema-lsp/tests/e2e && uv run pytest test_semantic_tokens.py -v`
Expected: PASS (these test against the already-implemented server)

**Step 3: Commit**

```bash
git add crates/sema-lsp/tests/e2e/test_semantic_tokens.py
git commit -m "test(lsp): add E2E tests for semantic tokens"
```

---

## Task 2: E2E tests for folding ranges

**Files:**
- Create: `crates/sema-lsp/tests/e2e/test_folding_range.py`

**Step 1: Write the test file**

```python
"""Test textDocument/foldingRange."""

import pytest
from lsprotocol.types import (
    FoldingRangeParams,
    TextDocumentIdentifier,
)
from pytest_lsp import LanguageClient

from helpers import open_doc


@pytest.mark.asyncio
async def test_folding_range_multiline(client: LanguageClient):
    """Multi-line forms should produce folding ranges."""
    uri = await open_doc(
        client,
        "(defun foo (x)\n  (+ x 1))",
    )
    result = await client.text_document_folding_range_async(
        FoldingRangeParams(
            text_document=TextDocumentIdentifier(uri=uri),
        )
    )
    assert result is not None
    assert len(result) >= 1
    # The fold should start at line 0
    assert result[0].start_line == 0


@pytest.mark.asyncio
async def test_folding_range_single_line(client: LanguageClient):
    """Single-line forms should not produce folding ranges."""
    uri = await open_doc(client, "(define x 42)")
    result = await client.text_document_folding_range_async(
        FoldingRangeParams(
            text_document=TextDocumentIdentifier(uri=uri),
        )
    )
    assert result is not None
    assert len(result) == 0


@pytest.mark.asyncio
async def test_folding_range_nested(client: LanguageClient):
    """Nested multi-line forms should produce multiple fold ranges."""
    uri = await open_doc(
        client,
        "(defun foo (x)\n  (let ((y 1))\n    (+ x y)))",
    )
    result = await client.text_document_folding_range_async(
        FoldingRangeParams(
            text_document=TextDocumentIdentifier(uri=uri),
        )
    )
    assert result is not None
    assert len(result) >= 2  # outer defun + inner let
```

**Step 2: Run test to verify it passes**

Run: `cd crates/sema-lsp/tests/e2e && uv run pytest test_folding_range.py -v`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/sema-lsp/tests/e2e/test_folding_range.py
git commit -m "test(lsp): add E2E tests for folding ranges"
```

---

## Task 3: E2E test for document highlight + update capability test

**Files:**
- Create: `crates/sema-lsp/tests/e2e/test_document_highlight.py`
- Modify: `crates/sema-lsp/tests/e2e/test_capabilities.py`

**Step 1: Write the test file**

Note: This test will fail initially — it depends on Task 6 (implementing document highlight). Write it now so it serves as the failing test for Task 6.

```python
"""Test textDocument/documentHighlight."""

import pytest
from lsprotocol.types import (
    DocumentHighlightParams,
    Position,
    TextDocumentIdentifier,
)
from pytest_lsp import LanguageClient

from helpers import open_doc


@pytest.mark.asyncio
async def test_highlight_variable_occurrences(client: LanguageClient):
    """Highlighting a variable should mark all occurrences."""
    uri = await open_doc(client, "(define x 42)\n(+ x 1)")
    result = await client.text_document_document_highlight_async(
        DocumentHighlightParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=8),  # on 'x'
        )
    )
    assert result is not None
    assert len(result) >= 2  # definition + usage


@pytest.mark.asyncio
async def test_highlight_local_only(client: LanguageClient):
    """Highlighting a local variable should not include shadowed occurrences."""
    uri = await open_doc(
        client,
        "(define x 1)\n(let ((x 2)) x)",
    )
    result = await client.text_document_document_highlight_async(
        DocumentHighlightParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=1, character=13),  # on 'x' in let body
        )
    )
    assert result is not None
    # Should only highlight the let-bound x and its usage, not the top-level x
    assert len(result) == 2


@pytest.mark.asyncio
async def test_highlight_builtin(client: LanguageClient):
    """Highlighting a builtin should return all occurrences in the document."""
    uri = await open_doc(client, "(map inc (list 1 2))\n(map dec (list 3 4))")
    result = await client.text_document_document_highlight_async(
        DocumentHighlightParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=1),  # on 'map'
        )
    )
    assert result is not None
    assert len(result) == 2


@pytest.mark.asyncio
async def test_highlight_no_symbol(client: LanguageClient):
    """Highlighting whitespace should return None."""
    uri = await open_doc(client, "  (+ 1 2)")
    result = await client.text_document_document_highlight_async(
        DocumentHighlightParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=0),
        )
    )
    assert result is None or len(result) == 0
```

**Step 2: Update capabilities test**

Add `document_highlight_provider` check to `test_capabilities.py`.

**Step 3: Run — these will FAIL until Task 6 is implemented**

Run: `cd crates/sema-lsp/tests/e2e && uv run pytest test_document_highlight.py -v`
Expected: FAIL (server doesn't advertise or implement document highlight yet)

---

## Task 4: Completion documentation

**Files:**
- Modify: `crates/sema-lsp/src/lib.rs` — `handle_complete` method (~line 1724)

**Step 1: Write unit test**

Add a unit test in `lib.rs` `mod tests` that verifies completion items include documentation. This requires creating a `BackendState` in the test — but since `handle_complete` needs documents + builtin_docs, and the integration test is simpler, we'll test via E2E instead.

Create `crates/sema-lsp/tests/e2e/test_completion_docs.py`:

```python
"""Test that completion items include documentation."""

import pytest
from lsprotocol.types import (
    CompletionList,
    CompletionParams,
    Position,
    TextDocumentIdentifier,
)
from pytest_lsp import LanguageClient

from helpers import open_doc


@pytest.mark.asyncio
async def test_completion_builtin_has_documentation(client: LanguageClient):
    """Completion items for builtins should include documentation."""
    uri = await open_doc(client, "(ma")
    result = await client.text_document_completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=3),  # after 'ma'
        )
    )
    assert result is not None
    items = result.items if isinstance(result, CompletionList) else result
    map_items = [i for i in items if i.label == "map"]
    assert len(map_items) >= 1
    item = map_items[0]
    assert item.documentation is not None


@pytest.mark.asyncio
async def test_completion_special_form_has_documentation(client: LanguageClient):
    """Completion items for special forms should include documentation."""
    uri = await open_doc(client, "(def")
    result = await client.text_document_completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=4),
        )
    )
    assert result is not None
    items = result.items if isinstance(result, CompletionList) else result
    define_items = [i for i in items if i.label == "define"]
    assert len(define_items) >= 1
    item = define_items[0]
    assert item.documentation is not None
```

**Step 2: Implement completion documentation**

In `handle_complete` (~line 1724), add `documentation` field to each `CompletionItem` by looking up `self.builtin_docs`:

For special forms (line ~1729):
```rust
items.push(CompletionItem {
    label: name.to_string(),
    kind: Some(CompletionItemKind::KEYWORD),
    documentation: self.builtin_docs.get(*name).map(|doc| {
        Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: doc.clone(),
        })
    }),
    ..Default::default()
});
```

For builtins (line ~1740):
```rust
items.push(CompletionItem {
    label: name.clone(),
    kind: Some(CompletionItemKind::FUNCTION),
    documentation: self.builtin_docs.get(name).map(|doc| {
        Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: doc.clone(),
        })
    }),
    ..Default::default()
});
```

**Step 3: Run tests**

Run: `cargo test -p sema-lsp && cd crates/sema-lsp/tests/e2e && uv run pytest test_completion_docs.py -v`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/sema-lsp/src/lib.rs crates/sema-lsp/tests/e2e/test_completion_docs.py
git commit -m "feat(lsp): add documentation to completion items"
```

---

## Task 5: VS Code initializationOptions for semaPath

**Files:**
- Modify: `editors/vscode/sema/src/extension.ts`

**Step 1: Add initializationOptions to the client config**

In `extension.ts`, modify `clientOptions` (~line 39) to include `initializationOptions`:

```typescript
const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: 'file', language: 'sema' }],
    outputChannel,
    initializationOptions: {
        semaPath: semaPath,
    },
};
```

**Step 2: Verify** — rebuild the extension (`npm run compile` in `editors/vscode/sema/`) and check no TypeScript errors.

**Step 3: Commit**

```bash
git add editors/vscode/sema/src/extension.ts
git commit -m "feat(vscode): pass semaPath via initializationOptions"
```

---

## Task 6: Document highlight (textDocument/documentHighlight)

**Files:**
- Modify: `crates/sema-lsp/src/lib.rs` — add `DocumentHighlight` variant to `LspRequest`, handler, capability, and async method

### Step 1: Add LspRequest variant

In the `LspRequest` enum (~line 1462), add:

```rust
/// Document highlight request.
DocumentHighlight {
    uri: Url,
    position: Position,
    reply: tokio::sync::oneshot::Sender<Option<Vec<DocumentHighlight>>>,
},
```

### Step 2: Add handler method to BackendState

After `handle_references` (~line 1956), add `handle_document_highlight`:

```rust
fn handle_document_highlight(
    &self,
    uri: &Url,
    position: &Position,
) -> Option<Vec<DocumentHighlight>> {
    let uri_str = uri.as_str();
    let text = self.documents.get(uri_str)?;
    let line_idx = position.line as usize;
    let line = text.lines().nth(line_idx)?;
    let byte_offset = utf16_to_byte_offset(line, position.character);
    let symbol = extract_symbol_at(line, byte_offset);
    if symbol.is_empty() {
        return None;
    }

    let cached = self.cached_parses.get(uri_str)?;
    let sema_line = position.line as usize + 1;
    let sema_col = position.character as usize + 1;

    // Use scope-aware references for locally scoped symbols
    if cached.scope_tree.is_locally_scoped(symbol, sema_line, sema_col) {
        let refs = cached.scope_tree.find_scope_aware_references(
            symbol, sema_line, sema_col, &cached.symbol_spans,
        );
        let highlights: Vec<DocumentHighlight> = refs
            .into_iter()
            .map(|span| DocumentHighlight {
                range: span_to_range(&span),
                kind: None,
            })
            .collect();
        return if highlights.is_empty() { None } else { Some(highlights) };
    }

    // Top-level/global: all occurrences in this document that resolve to top-level
    let mut highlights = Vec::new();
    for (name, span) in &cached.symbol_spans {
        if name != symbol {
            continue;
        }
        match cached.scope_tree.resolve_at(name, span.line, span.col) {
            Some(resolved) if !resolved.is_top_level => continue,
            _ => {}
        }
        highlights.push(DocumentHighlight {
            range: span_to_range(span),
            kind: None,
        });
    }

    if highlights.is_empty() { None } else { Some(highlights) }
}
```

### Step 3: Add capability in initialize

In `ServerCapabilities` (~line 2694), add:

```rust
document_highlight_provider: Some(OneOf::Left(true)),
```

### Step 4: Add async method in Backend impl

After the `references` async method, add:

```rust
async fn document_highlight(
    &self,
    params: DocumentHighlightParams,
) -> Result<Option<Vec<DocumentHighlight>>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _ = self.tx.send(LspRequest::DocumentHighlight {
        uri: params.text_document_position_params.text_document.uri,
        position: params.text_document_position_params.position,
        reply: tx,
    });
    Ok(rx.await.unwrap_or(None))
}
```

### Step 5: Add handler in backend message loop

In the backend match (~line 3117 area), add:

```rust
LspRequest::DocumentHighlight { uri, position, reply } => {
    let result = state.handle_document_highlight(&uri, &position);
    let _ = reply.send(result);
}
```

### Step 6: Update capabilities E2E test

In `test_capabilities.py`, add:

```python
assert caps.document_highlight_provider is not None
```

### Step 7: Run all tests

Run: `cargo test -p sema-lsp && cd crates/sema-lsp/tests/e2e && uv run pytest -v`
Expected: ALL PASS

### Step 8: Commit

```bash
git add crates/sema-lsp/src/lib.rs crates/sema-lsp/tests/e2e/test_document_highlight.py crates/sema-lsp/tests/e2e/test_capabilities.py
git commit -m "feat(lsp): implement textDocument/documentHighlight"
```

---

## Task 7: Trim Helix highlights.scm builtin list

**Files:**
- Modify: `editors/helix/queries/sema/highlights.scm`

### Step 1: Replace the massive builtin function list

The 100+ hardcoded builtin names (lines 169-306) are now redundant because the LSP provides semantic tokens. Replace the giant `#any-of?` block with a much smaller curated set.

Helix applies semantic token highlighting from the LSP when available, falling back to tree-sitter queries when the LSP is not running. The tree-sitter queries should cover the most common/recognizable builtins so files still look good without the LSP.

Replace lines 167-306 (the `@function.builtin` block) with a minimal curated list:

```scheme
; --- Core builtin functions (fallback when LSP semantic tokens unavailable) ---

(list
  .
  (symbol) @function.builtin
  (#any-of? @function.builtin
    ; Higher-order / functional
    "map" "filter" "foldl" "foldr" "reduce" "for-each" "apply" "flat-map"
    ; I/O
    "display" "print" "println" "format" "read" "read-line"
    ; Lists
    "list" "cons" "car" "cdr" "first" "rest" "nth"
    "append" "reverse" "length" "sort" "range"
    ; Hash maps
    "hash-map" "get" "assoc" "keys" "vals" "merge"
    ; Type predicates
    "number?" "string?" "symbol?" "pair?" "boolean?" "nil?" "list?" "map?"
    ; Conversions
    "string->number" "number->string" "string->symbol" "symbol->string"
    ; Math
    "abs" "min" "max" "round" "floor" "ceiling" "sqrt"
    ; Strings
    "string-append" "substring" "string-length"
    ; Misc
    "not" "error" "gensym" "type"))
```

### Step 2: Add a comment explaining the design decision

At the top of the trimmed block, add:
```scheme
; --- Core builtin functions (fallback when LSP semantic tokens unavailable) ---
; The LSP provides full semantic token classification for all builtins.
; This list covers only the most common functions so files look reasonable
; when editing without the LSP (e.g., quick file viewing).
```

### Step 3: Verify Helix queries still parse

Run: `helix --health sema` (if available) or visually inspect a `.sema` file in Helix.

### Step 4: Commit

```bash
git add editors/helix/queries/sema/highlights.scm
git commit -m "refactor(helix): trim builtin highlights, rely on LSP semantic tokens"
```

---

## Task 8: Plan — Cross-file references and rename (future)

> This task is a **design document only**, not implementation.

### Current state

- `handle_references` iterates `self.cached_parses` (only open documents)
- `handle_rename` iterates `self.cached_parses` (only open documents)
- `import_cache` has parsed ASTs for imported files discovered via `get_import_cache`
- Workspace scanning on `initialized` populates `import_cache` for all `.sema` files

### Design: Cross-file references

**Approach:** When the symbol is top-level and not locally scoped, search both `cached_parses` (open docs) AND `import_cache` (workspace files).

**Changes needed:**

1. **`ImportCache` needs `symbol_spans` and `scope_tree`** — Currently `ImportCache` stores `ast`, `span_map`, `symbol_spans`, but NOT a `ScopeTree`. Add `scope_tree: ScopeTree` to `ImportCache` so we can do scope-aware reference filtering on workspace files too.

2. **`handle_references` extension:**
   ```
   // After searching cached_parses for open docs...
   // Also search import_cache for workspace files not currently open
   for (path, import_cached) in &self.import_cache {
       let import_uri = Url::from_file_path(path)?;
       let import_uri_str = import_uri.as_str();
       // Skip if already searched in cached_parses
       if self.cached_parses.contains_key(import_uri_str) { continue; }
       // Search symbol_spans, filtering with scope_tree
       for (name, span) in &import_cached.symbol_spans {
           if name != symbol { continue; }
           match import_cached.scope_tree.resolve_at(name, span.line, span.col) {
               Some(resolved) if !resolved.is_top_level => continue,
               _ => {}
           }
           locations.push(Location { uri: import_uri.clone(), range: span_to_range(span) });
       }
   }
   ```

3. **`handle_rename` extension:** Same pattern — iterate `import_cache` entries not in `cached_parses`, collect `TextEdit`s for unshadowed occurrences.

4. **Consideration: file watching** — Currently import_cache uses mtime-based invalidation. For cross-file rename to work correctly, the server should re-read affected files after rename. Consider adding `textDocument/didSave` handling or `workspace/didChangeWatchedFiles` support.

5. **Performance** — For large workspaces, iterating all import_cache entries on every reference request could be slow. Consider:
   - A reverse index: `symbol_name → Vec<(PathBuf, Span)>` built during workspace scan
   - Lazy population: only build the index when first needed
   - Debounced rebuilds on file changes

### Estimated effort: 2-3 hours implementation + testing

---

## Task 9: Plan — Semantic tokens range support (future)

> This task is a **design document only**, not implementation.

### Current state

- `textDocument/semanticTokens/full` is implemented, returning tokens for the entire document
- `range` is set to `None` in `SemanticTokensOptions`
- Every edit re-tokenizes the full document

### Design: Semantic tokens range

**Approach:** Implement `textDocument/semanticTokens/range` to only tokenize visible lines.

**Changes needed:**

1. **Add `SemanticTokensRange` variant to `LspRequest`:**
   ```rust
   SemanticTokensRange {
       uri: Url,
       range: Range,
       reply: tokio::sync::oneshot::Sender<Option<SemanticTokensRangeResult>>,
   },
   ```

2. **Add capability:**
   ```rust
   range: Some(SemanticTokensRangeOptions::Bool(true)),
   ```

3. **Handler:** Reuse `handle_semantic_tokens_full` logic but filter `raw_tokens` to only those within the requested range before encoding deltas:
   ```rust
   fn handle_semantic_tokens_range(&self, uri: &Url, range: &Range) -> Option<SemanticTokensRangeResult> {
       // Same as full, but filter:
       // raw_tokens.retain(|(line, ..)| {
       //     let lsp_line = (line - 1) as u32;
       //     lsp_line >= range.start.line && lsp_line <= range.end.line
       // });
   }
   ```

4. **Async method + backend dispatch:** Standard pattern matching the other handlers.

### Consideration

The main benefit is reduced payload size. Since Sema files are typically small (< 1000 lines), the performance gain is marginal. The full tokenization is already fast (sub-millisecond for typical files). **Priority: low.**

### Estimated effort: 1 hour implementation + testing
