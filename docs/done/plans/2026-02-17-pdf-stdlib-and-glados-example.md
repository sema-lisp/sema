# PDF Stdlib & GLaDOS Receipt Processor Example

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add pure-Rust PDF processing builtins (`pdf/extract-text`, `pdf/extract-text-pages`, `pdf/page-count`, `pdf/metadata`) to sema-stdlib, with full integration tests and fixture PDFs, then build a GLaDOS receipt processor example that showcases these alongside LLM extraction.

**Architecture:** New `crates/sema-stdlib/src/pdf.rs` module using the `pdf-extract` crate (pure Rust, cross-platform, no shell-out). The `pdf-extract` crate (v0.10, 685K downloads) wraps `lopdf` and handles font decoding, CMap tables, and encoding. We use `pdf-extract` for text extraction and `lopdf` (already a transitive dep) directly for page count and metadata. All functions gated behind `Caps::FS_READ`. Module excluded from WASM builds.

**Tech Stack:** `pdf-extract` v0.10 crate, `lopdf` v0.38 (transitive), Rust 2021, integration tests via `cargo test -p sema --test integration_test`

---

## Task 1: Add `pdf-extract` and `lopdf` dependencies

**Files:**

- Modify: `Cargo.toml` (workspace deps, lines 22-47)
- Modify: `crates/sema-stdlib/Cargo.toml` (lines 26-30)

**Step 1: Add workspace dependencies**

In `Cargo.toml`, add to `[workspace.dependencies]`:

```toml
pdf-extract = "0.10"
lopdf = "0.38"
```

**Step 2: Add to sema-stdlib's platform-specific deps**

In `crates/sema-stdlib/Cargo.toml`, add under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`:

```toml
pdf-extract.workspace = true
lopdf.workspace = true
```

**Step 3: Verify it compiles**

Run: `cargo check -p sema-stdlib`
Expected: Compiles with no errors (deps download and build)

**Step 4: Commit**

```bash
git add Cargo.toml crates/sema-stdlib/Cargo.toml Cargo.lock
git commit -m "deps: add pdf-extract and lopdf for PDF processing"
```

---

## Task 2: Create `pdf.rs` module with all four functions

**Files:**

- Create: `crates/sema-stdlib/src/pdf.rs`
- Modify: `crates/sema-stdlib/src/lib.rs`

**Step 1: Create `crates/sema-stdlib/src/pdf.rs`**

```rust
use sema_core::{Caps, SemaError, Value};

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // (pdf/extract-text path) → string
    // Extracts all text from a PDF file, concatenated across all pages.
    crate::register_fn_gated(env, sandbox, Caps::FS_READ, "pdf/extract-text", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("pdf/extract-text", "1", args.len()));
        }
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let bytes = std::fs::read(path)
            .map_err(|e| SemaError::Io(format!("pdf/extract-text {path}: {e}")))?;
        let text = pdf_extract::extract_text_from_mem(&bytes)
            .map_err(|e| SemaError::eval(&format!("pdf/extract-text {path}: {e}")))?;
        Ok(Value::string(&text))
    });

    // (pdf/extract-text-pages path) → list of strings (one per page)
    crate::register_fn_gated(env, sandbox, Caps::FS_READ, "pdf/extract-text-pages", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("pdf/extract-text-pages", "1", args.len()));
        }
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let pages = pdf_extract::extract_text_by_pages(path)
            .map_err(|e| SemaError::eval(&format!("pdf/extract-text-pages {path}: {e}")))?;
        Ok(Value::list(pages.into_iter().map(|s| Value::string(&s)).collect()))
    });

    // (pdf/page-count path) → integer
    crate::register_fn_gated(env, sandbox, Caps::FS_READ, "pdf/page-count", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("pdf/page-count", "1", args.len()));
        }
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let doc = lopdf::Document::load(path)
            .map_err(|e| SemaError::eval(&format!("pdf/page-count {path}: {e}")))?;
        Ok(Value::int(doc.get_pages().len() as i64))
    });

    // (pdf/metadata path) → map with :title, :author, :subject, :creator, :producer, :pages
    crate::register_fn_gated(env, sandbox, Caps::FS_READ, "pdf/metadata", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("pdf/metadata", "1", args.len()));
        }
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let doc = lopdf::Document::load(path)
            .map_err(|e| SemaError::eval(&format!("pdf/metadata {path}: {e}")))?;

        let mut map = std::collections::BTreeMap::new();
        map.insert(Value::keyword("pages"), Value::int(doc.get_pages().len() as i64));

        // Extract Info dictionary fields
        if let Ok(info_ref) = doc.trailer.get(b"Info") {
            if let Ok(info_obj) = doc.dereference(info_ref) {
                if let Ok(dict) = info_obj.1.as_dict() {
                    for (key_name, keyword) in &[
                        (b"Title".as_slice(), "title"),
                        (b"Author", "author"),
                        (b"Subject", "subject"),
                        (b"Creator", "creator"),
                        (b"Producer", "producer"),
                    ] {
                        if let Ok(val) = dict.get(*key_name) {
                            if let Ok(s) = val.as_name_str() {
                                map.insert(Value::keyword(keyword), Value::string(s));
                            } else if let Ok(s) = val.as_str() {
                                map.insert(Value::keyword(keyword), Value::string(
                                    &String::from_utf8_lossy(s.as_bytes()),
                                ));
                            }
                        }
                    }
                }
            }
        }

        Ok(Value::map(map))
    });
}
```

**Step 2: Register the module in `lib.rs`**

In `crates/sema-stdlib/src/lib.rs`:

Add after `mod text;` (line 28):

```rust
#[cfg(not(target_arch = "wasm32"))]
mod pdf;
```

Add at the end of `register_stdlib()`, before the closing `}` (after line 59):

```rust
    #[cfg(not(target_arch = "wasm32"))]
    pdf::register(env, sandbox);
```

**Step 3: Verify it compiles**

Run: `cargo check -p sema-stdlib`
Expected: Compiles with no errors

**Step 4: Commit**

```bash
git add crates/sema-stdlib/src/pdf.rs crates/sema-stdlib/src/lib.rs
git commit -m "feat: add pdf/ stdlib module (extract-text, extract-text-pages, page-count, metadata)"
```

---

## Task 3: Create PDF fixture files for testing

**Files:**

- Create: `crates/sema/tests/fixtures/sample-invoice.pdf`
- Create: `crates/sema/tests/fixtures/not-a-receipt.pdf`
- Copy: `examples/fixtures/glados-downloads/input/Invoice-P8ZVOH52-0001.pdf`
- Copy: `examples/fixtures/glados-downloads/input/Invoice-P8ZVOH52-0010.pdf`
- Copy: `examples/fixtures/glados-downloads/input/Receipt-2824-3238.pdf`

We need minimal programmatically-generated PDFs for tests (so they work in CI without external files), plus real invoice copies for the example.

**Step 1: Generate test fixture PDFs using lopdf**

Create a small Rust script `crates/sema/tests/generate_fixtures.rs` — actually, simpler: write a shell one-liner that creates minimal PDFs. The simplest valid PDF with text can be created as a raw byte file.

Create a helper script `scripts/generate-test-pdfs.sh`:

```bash
#!/bin/bash
# Generate minimal PDF fixtures for integration tests
set -euo pipefail

FIXTURE_DIR="crates/sema/tests/fixtures"
mkdir -p "$FIXTURE_DIR"

# Use Python to generate minimal PDFs (available everywhere)
python3 -c "
import struct

def make_pdf(text, path):
    # Minimal valid PDF 1.4 with one page containing text
    content = f'BT /F1 12 Tf 100 700 Td ({text}) Tj ET'
    content_bytes = content.encode()

    objects = []
    offsets = []
    pdf = b'%PDF-1.4\n'

    # Obj 1: Catalog
    offsets.append(len(pdf))
    obj = b'1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n'
    pdf += obj

    # Obj 2: Pages
    offsets.append(len(pdf))
    obj = b'2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n'
    pdf += obj

    # Obj 3: Page
    offsets.append(len(pdf))
    obj = b'3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n'
    pdf += obj

    # Obj 4: Content stream
    offsets.append(len(pdf))
    obj = f'4 0 obj\n<< /Length {len(content_bytes)} >>\nstream\n'.encode() + content_bytes + b'\nendstream\nendobj\n'
    pdf += obj

    # Obj 5: Font
    offsets.append(len(pdf))
    obj = b'5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n'
    pdf += obj

    # Obj 6: Info dictionary
    offsets.append(len(pdf))
    obj = b'6 0 obj\n<< /Title (Test Document) /Author (Sema Test Suite) >>\nendobj\n'
    pdf += obj

    # Update catalog to reference info
    # Actually, info goes in trailer, so just track it

    # Cross-reference table
    xref_offset = len(pdf)
    pdf += b'xref\n'
    pdf += f'0 {len(offsets) + 1}\n'.encode()
    pdf += b'0000000000 65535 f \n'
    for off in offsets:
        pdf += f'{off:010d} 00000 n \n'.encode()

    pdf += b'trailer\n'
    pdf += f'<< /Size {len(offsets) + 1} /Root 1 0 R /Info 6 0 R >>\n'.encode()
    pdf += b'startxref\n'
    pdf += f'{xref_offset}\n'.encode()
    pdf += b'%%EOF\n'

    with open(path, 'wb') as f:
        f.write(pdf)
    print(f'  Created {path} ({len(pdf)} bytes)')

print('Generating PDF fixtures...')
make_pdf('Invoice 2025-01-15 Acme Corp Total: 1234.56 USD Billed to: Test Customer', '$FIXTURE_DIR/sample-invoice.pdf')
make_pdf('Meeting Notes - Q4 Planning Session - Internal Document - Not an invoice', '$FIXTURE_DIR/not-a-receipt.pdf')
print('Done.')
"
```

Run: `bash scripts/generate-test-pdfs.sh`
Expected: Two PDFs created in `crates/sema/tests/fixtures/`

**Step 2: Copy real invoices for the GLaDOS example**

```bash
mkdir -p examples/fixtures/glados-downloads/input
cp /Users/helge/Downloads/sourcegraph/Invoice-P8ZVOH52-0001.pdf examples/fixtures/glados-downloads/input/
cp /Users/helge/Downloads/sourcegraph/Invoice-P8ZVOH52-0010.pdf examples/fixtures/glados-downloads/input/
cp /Users/helge/Downloads/sourcegraph/Receipt-2824-3238.pdf examples/fixtures/glados-downloads/input/
```

**Step 3: Create a synthetic non-matching PDF for the example**

Use the same Python script approach to create `examples/fixtures/glados-downloads/input/acme-corp-invoice.pdf` — a fake invoice from "Acme Corp" billed to "Wayne Enterprises" so it won't match the "Liseth" whitelist:

```bash
python3 -c "
def make_pdf(text, path):
    content = f'BT /F1 10 Tf 72 720 Td ({text}) Tj ET'
    content_bytes = content.encode()
    pdf = b'%PDF-1.4\n'
    offsets = []
    offsets.append(len(pdf)); pdf += b'1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n'
    offsets.append(len(pdf)); pdf += b'2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n'
    offsets.append(len(pdf)); pdf += b'3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n'
    offsets.append(len(pdf)); pdf += f'4 0 obj\n<< /Length {len(content_bytes)} >>\nstream\n'.encode() + content_bytes + b'\nendstream\nendobj\n'
    offsets.append(len(pdf)); pdf += b'5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n'
    xref_offset = len(pdf)
    pdf += b'xref\n' + f'0 {len(offsets)+1}\n'.encode() + b'0000000000 65535 f \n'
    for off in offsets: pdf += f'{off:010d} 00000 n \n'.encode()
    pdf += f'trailer\n<< /Size {len(offsets)+1} /Root 1 0 R >>\nstartxref\n{xref_offset}\n'.encode() + b'%%EOF\n'
    with open(path, 'wb') as f: f.write(pdf)
    print(f'Created {path}')

make_pdf('INVOICE - Acme Corp - 742 Evergreen Terrace - Invoice Number: ACM-2025-0042 - Date: 2025-03-15 - Bill To: Wayne Enterprises - Amount Due: 9999.99 USD - Payment Terms: Net 30', 'examples/fixtures/glados-downloads/input/acme-corp-invoice.pdf')
"
```

**Step 4: Create output directory for the example**

```bash
mkdir -p examples/fixtures/glados-downloads/output
```

**Step 5: Commit**

```bash
git add scripts/generate-test-pdfs.sh crates/sema/tests/fixtures/ examples/fixtures/
git commit -m "test: add PDF fixture files for tests and GLaDOS example"
```

---

## Task 4: Write integration tests for `pdf/extract-text`

**Files:**

- Modify: `crates/sema/tests/integration_test.rs` (append at end)

**Step 1: Write the tests**

Append to the end of `crates/sema/tests/integration_test.rs`:

```rust
// --- PDF processing tests ---

#[test]
fn test_pdf_extract_text() {
    let result = eval(r#"(pdf/extract-text "crates/sema/tests/fixtures/sample-invoice.pdf")"#);
    let text = result.as_str().expect("should return a string");
    assert!(text.contains("Invoice"), "should contain 'Invoice', got: {text}");
    assert!(text.contains("Acme"), "should contain 'Acme', got: {text}");
}

#[test]
fn test_pdf_extract_text_not_receipt() {
    let result = eval(r#"(pdf/extract-text "crates/sema/tests/fixtures/not-a-receipt.pdf")"#);
    let text = result.as_str().expect("should return a string");
    assert!(text.contains("Meeting"), "should contain 'Meeting', got: {text}");
    assert!(!text.contains("Invoice"), "should NOT contain 'Invoice', got: {text}");
}

#[test]
fn test_pdf_extract_text_nonexistent() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(pdf/extract-text "/nonexistent/file.pdf")"#);
    assert!(result.is_err(), "should error on nonexistent file");
}

#[test]
fn test_pdf_extract_text_empty_string_arg() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(pdf/extract-text)"#);
    assert!(result.is_err(), "should error with no args");
}
```

**Step 2: Run the tests**

Run: `cargo test -p sema --test integration_test -- test_pdf_extract_text`
Expected: All 4 tests PASS

**Step 3: Commit**

```bash
git add crates/sema/tests/integration_test.rs
git commit -m "test: add integration tests for pdf/extract-text"
```

---

## Task 5: Write integration tests for `pdf/extract-text-pages`

**Files:**

- Modify: `crates/sema/tests/integration_test.rs` (append at end)

**Step 1: Write the tests**

```rust
#[test]
fn test_pdf_extract_text_pages() {
    let result = eval(r#"(pdf/extract-text-pages "crates/sema/tests/fixtures/sample-invoice.pdf")"#);
    let pages = result.as_list().expect("should return a list");
    assert_eq!(pages.len(), 1, "single-page PDF should return 1 page");
    let page_text = pages[0].as_str().expect("page should be a string");
    assert!(page_text.contains("Invoice"), "page should contain 'Invoice'");
}

#[test]
fn test_pdf_extract_text_pages_returns_list() {
    let result = eval(r#"(length (pdf/extract-text-pages "crates/sema/tests/fixtures/sample-invoice.pdf"))"#);
    assert_eq!(result, Value::int(1));
}

#[test]
fn test_pdf_extract_text_pages_nonexistent() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(pdf/extract-text-pages "/nonexistent.pdf")"#);
    assert!(result.is_err());
}
```

**Step 2: Run the tests**

Run: `cargo test -p sema --test integration_test -- test_pdf_extract_text_pages`
Expected: All 3 tests PASS

**Step 3: Commit**

```bash
git add crates/sema/tests/integration_test.rs
git commit -m "test: add integration tests for pdf/extract-text-pages"
```

---

## Task 6: Write integration tests for `pdf/page-count`

**Files:**

- Modify: `crates/sema/tests/integration_test.rs` (append at end)

**Step 1: Write the tests**

```rust
#[test]
fn test_pdf_page_count() {
    let result = eval(r#"(pdf/page-count "crates/sema/tests/fixtures/sample-invoice.pdf")"#);
    assert_eq!(result, Value::int(1));
}

#[test]
fn test_pdf_page_count_second_fixture() {
    let result = eval(r#"(pdf/page-count "crates/sema/tests/fixtures/not-a-receipt.pdf")"#);
    assert_eq!(result, Value::int(1));
}

#[test]
fn test_pdf_page_count_nonexistent() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(pdf/page-count "/nonexistent.pdf")"#);
    assert!(result.is_err());
}

#[test]
fn test_pdf_page_count_arity() {
    let interp = Interpreter::new();
    assert!(interp.eval_str(r#"(pdf/page-count)"#).is_err());
    assert!(interp.eval_str(r#"(pdf/page-count "a" "b")"#).is_err());
}
```

**Step 2: Run the tests**

Run: `cargo test -p sema --test integration_test -- test_pdf_page_count`
Expected: All 4 tests PASS

**Step 3: Commit**

```bash
git add crates/sema/tests/integration_test.rs
git commit -m "test: add integration tests for pdf/page-count"
```

---

## Task 7: Write integration tests for `pdf/metadata`

**Files:**

- Modify: `crates/sema/tests/integration_test.rs` (append at end)

**Step 1: Write the tests**

```rust
#[test]
fn test_pdf_metadata_returns_map() {
    let result = eval(r#"(pdf/metadata "crates/sema/tests/fixtures/sample-invoice.pdf")"#);
    assert!(result.as_map_rc().is_some(), "should return a map");
}

#[test]
fn test_pdf_metadata_has_pages() {
    let result = eval(r#"(get (pdf/metadata "crates/sema/tests/fixtures/sample-invoice.pdf") :pages)"#);
    assert_eq!(result, Value::int(1));
}

#[test]
fn test_pdf_metadata_has_title() {
    let result = eval(r#"(get (pdf/metadata "crates/sema/tests/fixtures/sample-invoice.pdf") :title)"#);
    let title = result.as_str().expect("should have :title");
    assert_eq!(title, "Test Document");
}

#[test]
fn test_pdf_metadata_has_author() {
    let result = eval(r#"(get (pdf/metadata "crates/sema/tests/fixtures/sample-invoice.pdf") :author)"#);
    let author = result.as_str().expect("should have :author");
    assert_eq!(author, "Sema Test Suite");
}

#[test]
fn test_pdf_metadata_nonexistent() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(pdf/metadata "/nonexistent.pdf")"#);
    assert!(result.is_err());
}

#[test]
fn test_pdf_metadata_arity() {
    let interp = Interpreter::new();
    assert!(interp.eval_str(r#"(pdf/metadata)"#).is_err());
    assert!(interp.eval_str(r#"(pdf/metadata "a" "b")"#).is_err());
}
```

**Step 2: Run the tests**

Run: `cargo test -p sema --test integration_test -- test_pdf_metadata`
Expected: All 6 tests PASS

**Step 3: Run ALL pdf tests together**

Run: `cargo test -p sema --test integration_test -- test_pdf`
Expected: All 17 PDF tests PASS

**Step 4: Commit**

```bash
git add crates/sema/tests/integration_test.rs
git commit -m "test: add integration tests for pdf/metadata"
```

---

## Task 8: Run full test suite to confirm nothing is broken

**Step 1: Run all tests**

Run: `cargo test`
Expected: All tests pass (existing + 17 new PDF tests)

**Step 2: Run lint**

Run: `make lint`
Expected: No warnings or errors

**Step 3: Fix any issues found, then commit if needed**

---

## Task 9: Write the GLaDOS receipt processor example

**Files:**

- Create: `examples/glados-downloads.sema`

This is the showcase example. It demonstrates: `pdf/extract-text`, `pdf/page-count`, `llm/auto-configure`, `llm/extract`, `text/clean-whitespace`, `file/list`, `file/exists?`, `file/mkdir`, `file/copy`, `file/read`, `file/write`, `path/join`, `path/basename`, `path/extension`, `json/encode-pretty`, `json/decode`, `string/contains?`, `string/lower`, `filter`, `for-each`, `cond`, `let*`.

**Step 1: Create the example**

```scheme
;; glados-downloads.sema — Receipt processor inspired by all-the-languages
;; Demonstrates: pdf/*, llm/extract, file I/O, path ops, JSON, text processing
;;
;; Watches an input folder for PDFs, uses LLM to classify receipts,
;; and auto-files matching invoices to an output folder.
;;
;; Usage: cargo run -- examples/glados-downloads.sema
;; Requires: ANTHROPIC_API_KEY or OPENAI_API_KEY environment variable

;; === Configuration ===
(define input-dir  "examples/fixtures/glados-downloads/input")
(define output-dir "examples/fixtures/glados-downloads/output")
(define state-file "/tmp/glados-sema-processed.json")
(define target-whitelist '("Liseth Solutions" "Liseth Solutions AS" "Liseth"))
(define target-label "Liseth Solutions")

;; === GLaDOS personality ===
(define (glados msg)
  (println (format "  🤖 GLaDOS: ~a" msg)))

(define (log msg)
  (println (format "[glados] ~a" msg)))

(define (log-detail msg)
  (println (format "[glados]   → ~a" msg)))

;; === State tracking ===
(define (load-state)
  (if (file/exists? state-file)
    (json/decode (file/read state-file))
    '()))

(define (save-state processed)
  (file/write state-file (json/encode-pretty processed)))

;; === Whitelist matching ===
(define (matches-whitelist? text)
  (define lower-text (string/lower text))
  (list/any?
    (fn (term) (string/contains? lower-text (string/lower term)))
    target-whitelist))

;; === PDF processing ===
(define (analyze-pdf path)
  (define text (text/clean-whitespace (pdf/extract-text path)))
  (define pages (pdf/page-count path))
  (log-detail (format "Extracted ~a chars from ~a page(s)" (string-length text) pages))

  ;; Use LLM to classify the document
  (define result
    (llm/extract
      {:isReceiptOrInvoice {:type :boolean :description "Is this a receipt or invoice?"}
       :vendorName {:type :string :description "The seller/merchant who sent the invoice"}
       :recipientName {:type :string :description "The buyer/customer being billed"}
       :invoiceDate {:type :string :description "Invoice date in YYYY-MM-DD format"}
       :totalAmount {:type :string :description "Total amount with currency"}
       :summary {:type :string :description "Brief one-line summary"}
       :proposedFileName {:type :string :description "Suggested filename: YYYY.MM.DD - VendorName.pdf"}}
      text
      {:model "haiku"}))

  (assoc result :extracted-text text :page-count pages))

;; === File processing pipeline ===
(define (process-pdf path processed)
  (define filename (path/basename path))

  ;; Skip if already processed
  (when (member filename processed)
    (log-detail (format "SKIP (already processed): ~a" filename))
    (begin processed))  ;; early return via begin — caller checks

  (log (format "Processing: ~a" filename))
  (log-detail (format "File size: ~a bytes" (get (file/info path) :size)))

  (define analysis (analyze-pdf path))
  (define is-receipt (get analysis :isReceiptOrInvoice))
  (define vendor (or (get analysis :vendorName) "Unknown"))
  (define recipient (or (get analysis :recipientName) ""))
  (define invoice-date (or (get analysis :invoiceDate) ""))
  (define summary (or (get analysis :summary) "Document"))
  (define proposed (or (get analysis :proposedFileName) ""))

  (log-detail (format "Vendor:    ~a" vendor))
  (log-detail (format "Recipient: ~a" recipient))
  (log-detail (format "Date:      ~a" invoice-date))
  (log-detail (format "Summary:   ~a" summary))

  (cond
    ;; Not a receipt at all
    ((not is-receipt)
     (log-detail "RESULT: Not a receipt/invoice → skipping")
     (glados "I analyzed this document. It is not a receipt. How disappointing.")
     (cons filename processed))

    ;; Receipt but doesn't match target
    ((not (matches-whitelist? (string-append vendor " " recipient)))
     (log-detail (format "RESULT: Receipt, but not for ~a" target-label))
     (glados (format "A receipt from ~a. Not for ~a. Filing this is your problem, not mine." vendor target-label))
     (cons filename processed))

    ;; Receipt matches target — file it!
    (else
     (log-detail (format "RESULT: ~a receipt → filing!" target-label))

     ;; Determine year folder
     (define year
       (if (>= (string-length invoice-date) 4)
         (substring invoice-date 0 4)
         "2025"))
     (define dest-dir (path/join output-dir year "Kvitteringer"))
     (file/mkdir dest-dir)

     ;; Build filename
     (define dest-filename
       (if (> (string-length proposed) 0)
         proposed
         (format "~a - ~a.pdf" (string/replace invoice-date "-" ".") vendor)))

     ;; Handle duplicates
     (define dest-path (path/join dest-dir dest-filename))
     (define final-path
       (if (file/exists? dest-path)
         (let loop ((n 2))
           (define try-name (format "~a (~a).pdf"
                             (string/replace dest-filename ".pdf" "") n))
           (define try-path (path/join dest-dir try-name))
           (if (file/exists? try-path)
             (loop (+ n 1))
             try-path))
         dest-path))

     ;; Copy file (not move, so fixtures stay intact for re-runs)
     (file/copy path final-path)
     (log (format "✓ FILED: ~a → ~a/Kvitteringer/" (path/basename final-path) year))

     (glados (format "Another receipt filed. ~a. Your tax deductions grow ever larger." summary))
     (cons filename processed))))

;; === Main ===
(println "═══════════════════════════════════════════════════════════")
(println "  GLaDOS Receipt Processor — Sema Edition")
(println "═══════════════════════════════════════════════════════════")
(glados "Oh. It's you again. With more receipts, I see.")
(newline)

;; Configure LLM
(define provider (llm/auto-configure))
(when (nil? provider)
  (println "Error: Set ANTHROPIC_API_KEY or OPENAI_API_KEY")
  (exit 1))
(log (format "LLM provider: ~a" provider))

;; Find PDFs
(define all-files (file/list input-dir))
(define pdf-files
  (filter
    (fn (f) (equal? (path/extension f) "pdf"))
    all-files))

(log (format "Found ~a PDF(s) in ~a" (length pdf-files) input-dir))
(newline)

;; Process each PDF
(define processed (load-state))
(define final-processed
  (foldl
    (fn (acc filename)
      (define full-path (path/join input-dir filename))
      (if (member filename acc)
        (begin
          (log-detail (format "SKIP (already processed): ~a" filename))
          acc)
        (begin
          (println "───────────────────────────────────────────────────────────")
          (process-pdf full-path acc))))
    processed
    pdf-files))

;; Save state
(save-state final-processed)

;; Summary
(newline)
(println "═══════════════════════════════════════════════════════════")
(define new-count (- (length final-processed) (length processed)))
(log (format "Processed ~a new PDF(s), ~a total tracked" new-count (length final-processed)))
(glados "The experiment is complete. The results are... exactly as pointless as expected.")
(println (format "\n~a" (llm/session-usage)))
(println "═══════════════════════════════════════════════════════════")
```

**Step 2: Verify it runs (requires API key)**

Run: `cargo run -- examples/glados-downloads.sema`
Expected: Processes 4 PDFs — 3 filed as Liseth receipts, 1 rejected as "not for Liseth" (or "not a receipt" for the synthetic one).

**Step 3: Commit**

```bash
git add examples/glados-downloads.sema
git commit -m "example: add GLaDOS receipt processor (showcases pdf/* and llm/extract)"
```

---

## Task 10: Final verification

**Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass

**Step 2: Run lint**

Run: `make lint`
Expected: Clean

**Step 3: Test the example again (clean state)**

```bash
rm -f /tmp/glados-sema-processed.json
rm -rf examples/fixtures/glados-downloads/output/
cargo run -- examples/glados-downloads.sema
```

Expected: All 4 PDFs processed, output folder created with filed receipts.

**Step 4: Verify idempotency (re-run skips already-processed)**

Run: `cargo run -- examples/glados-downloads.sema`
Expected: All 4 PDFs skipped as "already processed"

**Step 5: Final commit**

```bash
git add -A
git commit -m "feat: PDF stdlib + GLaDOS receipt processor example

- Add pdf/extract-text, pdf/extract-text-pages, pdf/page-count, pdf/metadata
- Pure Rust via pdf-extract + lopdf crates (cross-platform, no shell-out)
- 17 integration tests with fixture PDFs
- GLaDOS receipt processor example showcasing pdf/* + llm/extract"
```

---

## Summary of new functions

| Function                 | Signature                       | Returns         | Description                                                        |
| ------------------------ | ------------------------------- | --------------- | ------------------------------------------------------------------ |
| `pdf/extract-text`       | `(pdf/extract-text path)`       | string          | All text from all pages                                            |
| `pdf/extract-text-pages` | `(pdf/extract-text-pages path)` | list of strings | Text per page                                                      |
| `pdf/page-count`         | `(pdf/page-count path)`         | integer         | Number of pages                                                    |
| `pdf/metadata`           | `(pdf/metadata path)`           | map             | `:title`, `:author`, `:subject`, `:creator`, `:producer`, `:pages` |

## Test coverage

| Test                                       | What it verifies                           |
| ------------------------------------------ | ------------------------------------------ |
| `test_pdf_extract_text`                    | Extracts text containing expected keywords |
| `test_pdf_extract_text_not_receipt`        | Different fixture returns different text   |
| `test_pdf_extract_text_nonexistent`        | Error on missing file                      |
| `test_pdf_extract_text_empty_string_arg`   | Arity error with no args                   |
| `test_pdf_extract_text_pages`              | Returns list with one page                 |
| `test_pdf_extract_text_pages_returns_list` | Length is correct                          |
| `test_pdf_extract_text_pages_nonexistent`  | Error on missing file                      |
| `test_pdf_page_count`                      | Returns 1 for single-page PDF              |
| `test_pdf_page_count_second_fixture`       | Works on both fixtures                     |
| `test_pdf_page_count_nonexistent`          | Error on missing file                      |
| `test_pdf_page_count_arity`                | Arity errors (0 and 2 args)                |
| `test_pdf_metadata_returns_map`            | Returns a map                              |
| `test_pdf_metadata_has_pages`              | `:pages` key = 1                           |
| `test_pdf_metadata_has_title`              | `:title` = "Test Document"                 |
| `test_pdf_metadata_has_author`             | `:author` = "Sema Test Suite"              |
| `test_pdf_metadata_nonexistent`            | Error on missing file                      |
| `test_pdf_metadata_arity`                  | Arity errors                               |
