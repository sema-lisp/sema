.PHONY: all build release web-runtime test-web-e2e build-pgo pgo-profile install install-pgo uninstall test test-lsp test-embedding-bench test-http test-llm check clippy fmt fmt-check clean run lint lint-links docs docs-check update-pricing examples examples-vm smoke-bytecode rag-demo test-providers fuzz fuzz-reader fuzz-eval fuzz-grammar fuzz-grammar-emit setup docs-search-gate bench-1m bench-10m bench-100m site-dev site-build site-preview site-deploy deploy coverage coverage-html bench bench-vm bench-save bench-suite bench-closure bench-numeric bench-compare bench-baseline profile profile-vm ts-setup ts-generate ts-test ts-playground js-lib-build js-lib-dev sema-web-example sema-web-example-build

SEMA_WEB_EXAMPLE_DIR := examples/sema-web-app

build:
	cargo build

release:
	cargo build --release

# PGO build (instrument -> train -> rebuild). ~25% faster on 1BRC; see
# docs/performance-roadmap.md. `make pgo-profile` emits only the .profdata
# (target/pgo/merged.profdata) that CI consumes via -Cprofile-use.
build-pgo:
	./scripts/pgo-build.sh

pgo-profile:
	./scripts/pgo-build.sh --profile-only

install:
	cargo install --path crates/sema

# Like `install`, but PGO-optimized: runs the full instrument->train->rebuild
# pipeline and drops the resulting binary into the cargo bin dir (replacing any
# `sema` already there). Slower to build than `install`, faster at runtime.
install-pgo: build-pgo
	@install -m 0755 target/release/sema "$${CARGO_HOME:-$$HOME/.cargo}/bin/sema"
	@echo "Installed PGO-optimized sema -> $${CARGO_HOME:-$$HOME/.cargo}/bin/sema"

uninstall:
	cargo uninstall sema-lang

test:
	cargo test

test-lsp: release
	cargo test -p sema-lsp
	cd crates/sema-lsp/tests/e2e && uv run pytest -v

test-embedding-bench:
	cargo test -p sema-lang --test embedding_bench -- --ignored --nocapture

test-http:
	cargo test -p sema-lang --test http_test -- --ignored --nocapture

test-llm:
	cargo test -p sema-lang --test llm_test -- --ignored --nocapture

check:
	cargo check

clippy:
	cargo clippy -p sema-core -p sema-reader -p sema-eval -p sema-llm -p sema-stdlib -p sema-vm -p sema-lang -p sema-wasm -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt -- --check

clean:
	cargo clean

run:
	cargo run

lint: fmt-check clippy

# Fail if a publishable workspace crate is missing from publish.yml's order
# (guards against a half-published release on a newly-added crate).
.PHONY: check-publish-list
check-publish-list:
	@./scripts/check-publish-list.sh

# Regenerate the builtin doc index (and, later, the API-reference site) from the canonical
# structured source in crates/sema-docs/entries/stdlib + special-forms.
docs:
	cargo run -q -p sema-docs -- gen

# Regenerate the vendored LLM pricing snapshot (crates/sema-llm/src/pricing-data.json) from
# models.dev. Run this and commit the diff to ship updated prices in a patch release; we embed
# the snapshot at build time and do not fetch pricing at runtime.
update-pricing:
	./scripts/update-pricing.sh

# CI gate: every entry must have a summary (--strict), the committed index must be up to date with
# the source, and every registered builtin/special form must be documented (coverage test).
docs-check:
	cargo run -q -p sema-docs -- gen --strict
	git diff --exit-code crates/sema-docs/builtin_docs.generated.json
	cargo test -q -p sema-lsp builtin_doc_coverage

lint-links:
	lychee --config lychee.toml --no-progress '**/*.md'

# Run every runnable example headless and report pass/skip/fail. Interactive,
# server, and hardware examples are skipped (see scripts/run-examples.sh for the
# blacklist + rationale). Uses the release binary and a per-example timeout so it
# never hangs; exits non-zero if any runnable example fails.
examples: release
	@EXAMPLE_TIMEOUT=30 ./scripts/run-examples.sh

# Stress test: compile every runnable example into a standalone binary with
# `sema build` and execute it (exercises the whole release/portability path).
examples-build: release
	@EXAMPLE_TIMEOUT=30 ./scripts/build-examples.sh

example-notebook: build
	@echo "=== Running example notebook ==="
	cargo run --quiet -- notebook run examples/notebook/demo.sema-nb || true

test-notebook-e2e: build
	@echo "=== Running notebook E2E tests ==="
	cd crates/sema-notebook/tests/e2e && npx playwright test

# E2E for the `sema web` dev server: vendor the browser runtime, build the
# release binary (which embeds it), then drive the real server in a browser.
test-web-e2e: web-runtime
	cargo build --release -p sema-lang
	@echo "=== Running sema web dev-server E2E tests ==="
	cd packages/sema-web && npx playwright test --config playwright.dev-server.config.ts

mutants:
	@echo "=== Mutation testing (high-value crates) ==="
	cargo mutants -p sema-stdlib --timeout 30 -- --test-threads=1

mutants-core:
	cargo mutants -p sema-core --timeout 30

mutants-notebook:
	cargo mutants -p sema-notebook --timeout 30

mutants-all:
	@echo "=== Full mutation testing (all crates, slow) ==="
	cargo mutants --workspace --timeout 60 -- --test-threads=1

example-notebook-serve: build
	cargo run --quiet -- notebook serve examples/notebook/demo.sema-nb

smoke-bytecode: build
	@./scripts/smoke-bytecode.sh ./target/debug/sema

# Hermetic gate: docs_search must work from the binary alone in a FROM-scratch
# container (no source, no uncompiled docs, --network none). Requires docker + jq.
docs-search-gate:
	@./scripts/docs-search-gate.sh

test-providers: build
	@echo "=== Testing all LLM providers ==="
	cargo run --quiet -- examples/providers/test-all.sema

test-provider-%: build
	cargo run --quiet -- examples/providers/test-$*.sema

# Live RAG smoke test: index Sema's docs, retrieve → rerank → answer. Needs an
# embedding+rerank key (JINA/VOYAGE/COHERE) and a chat key. Caches to /tmp.
rag-demo: build
	@echo "=== RAG over Sema docs (embed -> search -> rerank -> answer) ==="
	cargo run --quiet -- examples/llm/rag-docs-search.sema

setup:
	rustup toolchain install nightly
	cargo install cargo-fuzz

fuzz: fuzz-reader fuzz-eval

fuzz-reader:
	cd crates/sema-reader && rustup run nightly cargo fuzz run fuzz_read -- -max_total_time=60
	cd crates/sema-reader && rustup run nightly cargo fuzz run fuzz_read_many -- -max_total_time=60

fuzz-eval:
	cd crates/sema-eval && rustup run nightly cargo fuzz run fuzz_eval -- -max_total_time=120 -timeout=10

# Grammar-based fuzzer written in Sema itself (fuzz/grammar-fuzz.sema). Generates
# random *valid* Sema programs and checks two correctness oracles: printer/reader
# round-trip and a compiler/VM value oracle. No nightly / cargo-fuzz needed — runs
# on the release binary. Every finding is reproducible from one integer seed.
#   make fuzz-grammar                  # default sweep (random seed)
#   make fuzz-grammar SEED=123 N=20000 DEPTH=5
#   make fuzz-grammar-emit             # print a few sample generated programs
fuzz-grammar: release
	@./scripts/grammar-fuzz.sh check \
		$(if $(N),-n $(N)) $(if $(DEPTH),-d $(DEPTH)) $(if $(SEED),-s $(SEED)) $(if $(V),-v)

fuzz-grammar-emit: release
	@./scripts/grammar-fuzz.sh emit \
		$(if $(N),-n $(N)) $(if $(DEPTH),-d $(DEPTH)) $(if $(SEED),-s $(SEED)) $(if $(OUT),-o $(OUT))

bench-1m: release
	time ./target/release/sema examples/benchmarks/1brc.sema -- bench-1m.txt

bench-10m: release
	time ./target/release/sema examples/benchmarks/1brc.sema -- bench-10m.txt

bench-100m: release
	time ./target/release/sema examples/benchmarks/1brc.sema -- bench-100m.txt

all: lint test build

# Website
.PHONY: site-dev site-build site-preview site-deploy site-og

site-dev:
	cd website && npm run dev

# Check vendored OG assets and regenerate per-page cards (public/og/*.jpg).
# Run after editing the template, logo, page titles, or the version, then
# commit the regenerated images before deploying.
site-og:
	cd website && npm run og:check
	cd website && npm run og

site-build:
	cd website && npm run build

site-preview: site-build
	cd website && npm run preview

site-deploy: site-build
	cd website && npx vercel --prod --yes

# JS Embedding Library
.PHONY: js-lib-build js-lib-dev

js-lib-build:
	wasm-pack build crates/sema-wasm --target web --release --scope sema-lang --out-dir ../../packages/sema-wasm/pkg -- --config 'profile.release.package.sema-wasm.opt-level="s"'
	cd packages/sema && npm install && npm run build

js-lib-dev:
	wasm-pack build crates/sema-wasm --target web --scope sema-lang --out-dir ../../packages/sema-wasm/pkg

sema-web-example-build:
	npm run build:wasm
	npm run build
	mkdir -p $(SEMA_WEB_EXAMPLE_DIR)/dist
	mkdir -p $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor
	mkdir -p $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor/sema
	mkdir -p $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor/sema/backends
	cp packages/sema-web/dist/index.js $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor/sema-web.js
	cp packages/sema/dist/index.js $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor/sema/index.js
	cp packages/sema/dist/vfs.js $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor/sema/vfs.js
	cp packages/sema/dist/backends/*.js $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor/sema/backends/
	cp packages/sema-wasm/pkg/sema_wasm.js $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor/sema_wasm.js
	cp packages/sema-wasm/pkg/sema_wasm_bg.wasm $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor/sema_wasm_bg.wasm
	cp node_modules/@preact/signals-core/dist/signals-core.module.js $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor/signals-core.module.js
	cp node_modules/morphdom/dist/morphdom-esm.js $(SEMA_WEB_EXAMPLE_DIR)/dist/vendor/morphdom-esm.js
	cargo run -p sema-lang -- build --target web $(SEMA_WEB_EXAMPLE_DIR)/app.sema -o $(SEMA_WEB_EXAMPLE_DIR)/dist/app.vfs
	@echo "Built $(SEMA_WEB_EXAMPLE_DIR)/dist/app.vfs"

# Vendor the browser runtime the `sema web` dev server embeds. Builds the WASM
# VM + JS packages, then copies the ~8 files the browser needs into the sema
# crate's assets dir, where build.rs picks them up (`web_runtime` cfg) and
# include_bytes! embeds them. These artifacts are gitignored (built, multi-MB);
# run this before `cargo build` if you want a `sema web`-capable binary.
WEB_RUNTIME_DIR := crates/sema/src/web/assets
web-runtime:
	npm run build:wasm
	npm run build
	mkdir -p $(WEB_RUNTIME_DIR)/sema/backends
	cp packages/sema-web/dist/index.js $(WEB_RUNTIME_DIR)/sema-web.js
	cp packages/sema/dist/index.js $(WEB_RUNTIME_DIR)/sema/index.js
	cp packages/sema/dist/vfs.js $(WEB_RUNTIME_DIR)/sema/vfs.js
	cp packages/sema/dist/backends/*.js $(WEB_RUNTIME_DIR)/sema/backends/
	cp packages/sema-wasm/pkg/sema_wasm.js $(WEB_RUNTIME_DIR)/sema_wasm.js
	cp packages/sema-wasm/pkg/sema_wasm_bg.wasm $(WEB_RUNTIME_DIR)/sema_wasm_bg.wasm
	cp node_modules/@preact/signals-core/dist/signals-core.module.js $(WEB_RUNTIME_DIR)/signals-core.module.js
	cp node_modules/morphdom/dist/morphdom-esm.js $(WEB_RUNTIME_DIR)/morphdom-esm.js
	@echo "Vendored web runtime -> $(WEB_RUNTIME_DIR) (rebuild the sema binary to embed)"

sema-web-example: sema-web-example-build
	@echo ""
	@echo "Serving the Sema Web example folder."
	@echo "Open: http://127.0.0.1:8788"
	npx serve -l 8788 $(SEMA_WEB_EXAMPLE_DIR)

# Playground
deploy: site-deploy playground-deploy

# One-shot "ship the web" pipeline: build the WASM playground, gate on the
# playground + notebook E2E suites, then deploy both the docs site and the
# playground to production. `deploy` is the quick path that skips the E2E gate.
# (Run `make site-og` first and commit the cards if titles/version changed.)
deploy-all: playground-build
	cd playground && npx playwright test
	$(MAKE) test-notebook-e2e
	$(MAKE) site-deploy
	$(MAKE) playground-deploy

.PHONY: playground-build playground-dev playground-deploy deploy deploy-all

playground-build:
	cd crates/sema-wasm && wasm-pack build --target web --out-dir ../../playground/pkg -- --config 'profile.release.package.sema-wasm.opt-level="s"'
	cd playground && node build.mjs

playground-dev: playground-build
	cd playground && node scripts/gen-devtools-json.mjs
	cd playground && npx serve -l 8787

playground-deploy: playground-build
	cd playground && npx vercel --prod --yes

# Coverage
coverage:
	cargo llvm-cov --workspace --lcov --output-path lcov.info

coverage-html:
	cargo llvm-cov --workspace --html
	@echo "Coverage report: target/llvm-cov/html/index.html"

# Benchmarking
BENCH_RUNS ?= 10
BENCH_WARMUP ?= 3
BENCH_SUITE ?= all

# The bytecode VM is the sole evaluator; `bench` == `bench-vm`.
bench: release
	@./scripts/bench.sh --suite $(BENCH_SUITE) --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-vm: release
	@./scripts/bench.sh --suite $(BENCH_SUITE) --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-save: release
	@mkdir -p target/bench
	@./scripts/bench.sh --suite $(BENCH_SUITE) --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP) \
		--export target/bench/bench-$$(git rev-parse --short HEAD 2>/dev/null || echo "nogit").json

bench-suite: release
	@./scripts/bench.sh --suite $(BENCH_SUITE) --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-closure: release
	@./scripts/bench.sh --suite closure --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-numeric: release
	@./scripts/bench.sh --suite numeric --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-compare: release
	@mkdir -p target/bench
	@./scripts/bench.sh --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP) \
		--export target/bench/current.json \
		--compare target/bench/baseline.json

bench-baseline: release
	@mkdir -p target/bench
	@./scripts/bench.sh --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP) \
		--export target/bench/baseline.json

# Profiling (requires: cargo install samply)
PROFILE_DIR := target/profiles
PROFILE_BENCH ?= tak
PROFILE_MODE ?= vm

profile:
	@mkdir -p $(PROFILE_DIR)
	RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile release-with-debug -p sema-lang
	samply record --save-only --output $(PROFILE_DIR)/$(PROFILE_BENCH)-$(PROFILE_MODE).json -- \
		./target/release-with-debug/sema --no-llm examples/benchmarks/$(PROFILE_BENCH).sema
	@echo "Profile saved: $(PROFILE_DIR)/$(PROFILE_BENCH)-$(PROFILE_MODE).json"
	@echo "Open with: samply load $(PROFILE_DIR)/$(PROFILE_BENCH)-$(PROFILE_MODE).json"

profile-vm:
	@$(MAKE) profile PROFILE_MODE=vm PROFILE_BENCH=$(PROFILE_BENCH)

# Tree-sitter grammar
TS_DIR := editors/tree-sitter-sema

ts-setup:
	cd $(TS_DIR) && npm install

ts-generate: $(TS_DIR)/node_modules
	cd $(TS_DIR) && npx tree-sitter generate

$(TS_DIR)/node_modules:
	cd $(TS_DIR) && npm install

ts-test: ts-generate
	cd $(TS_DIR) && npx tree-sitter test

ts-playground: ts-generate
	cd $(TS_DIR) && npx tree-sitter build --wasm && npx tree-sitter playground
