.PHONY: all build release install uninstall test test-lsp test-embedding-bench test-http test-llm check clippy fmt fmt-check clean run lint lint-links docs docs-check examples examples-vm smoke-bytecode test-providers fuzz fuzz-reader fuzz-eval setup bench-1m bench-10m bench-100m site-dev site-build site-preview site-deploy deploy coverage coverage-html bench bench-vm bench-tree bench-save bench-suite bench-closure bench-numeric bench-compare bench-baseline profile profile-vm profile-tree ts-setup ts-generate ts-test ts-playground js-lib-build js-lib-dev
build:
	cargo build

release:
	cargo build --release

install:
	cargo install --path crates/sema

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

# Regenerate the builtin doc index (and, later, the API-reference site) from the canonical
# structured source in crates/sema-docs/entries/stdlib + special-forms.
docs:
	cargo run -q -p sema-docs -- gen

# CI gate: every entry must have a summary (--strict), the committed index must be up to date with
# the source, and every registered builtin/special form must be documented (coverage test).
docs-check:
	cargo run -q -p sema-docs -- gen --strict
	git diff --exit-code crates/sema-docs/builtin_docs.generated.json
	cargo test -q -p sema-lsp builtin_doc_coverage

lint-links:
	lychee --config lychee.toml --no-progress '**/*.md'

examples: build
	@echo "=== Running examples ==="
	@for f in examples/*.sema; do \
		case "$$(basename $$f)" in \
			web-server.sema|eliza-web.sema) echo "  SKIP $$f (server)"; continue;; \
			eliza.sema) echo "  SKIP $$f (interactive)"; continue;; \
		esac; \
		echo "--- $$f ---"; \
		timeout 30 cargo run --quiet -- --no-llm "$$f" || true; \
	done
	@echo "=== Running stdlib examples ==="
	@for f in examples/stdlib/*.sema; do \
		echo "--- $$f ---"; \
		timeout 30 cargo run --quiet -- --no-llm "$$f" || true; \
	done

examples-vm: build
	@echo "=== Running examples (--vm) ==="
	@failed=""; \
	for f in examples/*.sema; do \
		case "$$(basename $$f)" in \
			web-server.sema|eliza-web.sema) echo "  SKIP $$f (server)"; continue;; \
			eliza.sema) echo "  SKIP $$f (interactive)"; continue;; \
		esac; \
		echo "--- $$f ---"; \
		if ! timeout 30 cargo run --quiet -- --vm --no-llm "$$f"; then \
			failed="$$failed $$f"; \
		fi; \
	done; \
	echo "=== Running stdlib examples (--vm) ==="; \
	for f in examples/stdlib/*.sema; do \
		echo "--- $$f ---"; \
		if ! timeout 30 cargo run --quiet -- --vm --no-llm "$$f"; then \
			failed="$$failed $$f"; \
		fi; \
	done; \
	if [ -n "$$failed" ]; then \
		echo ""; \
		echo "=== FAILED (--vm) ==="; \
		for f in $$failed; do echo "  $$f"; done; \
		echo ""; \
	else \
		echo ""; \
		echo "=== ALL PASSED (--vm) ==="; \
	fi

example-notebook: build
	@echo "=== Running example notebook ==="
	cargo run --quiet -- notebook run examples/notebook/demo.sema-nb || true

test-notebook-e2e: build
	@echo "=== Running notebook E2E tests ==="
	cd crates/sema-notebook/tests/e2e && npx playwright test

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

test-providers: build
	@echo "=== Testing all LLM providers ==="
	cargo run --quiet -- examples/providers/test-all.sema

test-provider-%: build
	cargo run --quiet -- examples/providers/test-$*.sema

setup:
	rustup toolchain install nightly
	cargo install cargo-fuzz

fuzz: fuzz-reader fuzz-eval

fuzz-reader:
	cd crates/sema-reader && rustup run nightly cargo fuzz run fuzz_read -- -max_total_time=60
	cd crates/sema-reader && rustup run nightly cargo fuzz run fuzz_read_many -- -max_total_time=60

fuzz-eval:
	cd crates/sema-eval && rustup run nightly cargo fuzz run fuzz_eval -- -max_total_time=120 -timeout=10

bench-1m: release
	time ./target/release/sema examples/benchmarks/1brc.sema -- bench-1m.txt

bench-10m: release
	time ./target/release/sema examples/benchmarks/1brc.sema -- bench-10m.txt

bench-100m: release
	time ./target/release/sema examples/benchmarks/1brc.sema -- bench-100m.txt

all: lint test build

# Website
.PHONY: site-dev site-build site-preview site-deploy

site-dev:
	cd website && npm run dev

site-build:
	cd website && npm run build

site-preview: site-build
	cd website && npm run preview

site-deploy: site-build
	cd website && npx vercel --prod --yes

# JS Embedding Library
.PHONY: js-lib-build js-lib-dev

js-lib-build:
	wasm-pack build crates/sema-wasm --target web --release --scope sema-lang --out-dir ../../packages/sema-wasm/pkg
	cd packages/sema && npm install && npm run build

js-lib-dev:
	wasm-pack build crates/sema-wasm --target web --scope sema-lang --out-dir ../../packages/sema-wasm/pkg

# Playground
deploy: site-deploy playground-deploy

.PHONY: playground-build playground-dev playground-deploy deploy

playground-build:
	cd crates/sema-wasm && wasm-pack build --target web --out-dir ../../playground/pkg
	cd playground && node build.mjs

playground-dev: playground-build
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

bench: release
	@./scripts/bench.sh --mode both --suite $(BENCH_SUITE) --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-vm: release
	@./scripts/bench.sh --mode vm --suite $(BENCH_SUITE) --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-tree: release
	@./scripts/bench.sh --mode tree --suite $(BENCH_SUITE) --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-save: release
	@mkdir -p target/bench
	@./scripts/bench.sh --mode both --suite $(BENCH_SUITE) --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP) \
		--export target/bench/bench-$$(git rev-parse --short HEAD 2>/dev/null || echo "nogit").json

bench-suite: release
	@./scripts/bench.sh --mode both --suite $(BENCH_SUITE) --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-closure: release
	@./scripts/bench.sh --mode vm --suite closure --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-numeric: release
	@./scripts/bench.sh --mode vm --suite numeric --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP)

bench-compare: release
	@mkdir -p target/bench
	@./scripts/bench.sh --mode vm --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP) \
		--export target/bench/current.json \
		--compare target/bench/baseline.json

bench-baseline: release
	@mkdir -p target/bench
	@./scripts/bench.sh --mode vm --runs $(BENCH_RUNS) --warmup $(BENCH_WARMUP) \
		--export target/bench/baseline.json

# Profiling (requires: cargo install samply)
PROFILE_DIR := target/profiles
PROFILE_BENCH ?= tak
PROFILE_MODE ?= vm

profile:
	@mkdir -p $(PROFILE_DIR)
	RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile release-with-debug -p sema-lang
	@modeflag=""; if [ "$(PROFILE_MODE)" = "vm" ]; then modeflag="--vm"; fi; \
	samply record --save-only --output $(PROFILE_DIR)/$(PROFILE_BENCH)-$(PROFILE_MODE).json -- \
		./target/release-with-debug/sema --no-llm $$modeflag examples/benchmarks/$(PROFILE_BENCH).sema
	@echo "Profile saved: $(PROFILE_DIR)/$(PROFILE_BENCH)-$(PROFILE_MODE).json"
	@echo "Open with: samply load $(PROFILE_DIR)/$(PROFILE_BENCH)-$(PROFILE_MODE).json"

profile-vm: 
	@$(MAKE) profile PROFILE_MODE=vm PROFILE_BENCH=$(PROFILE_BENCH)

profile-tree:
	@$(MAKE) profile PROFILE_MODE=tree PROFILE_BENCH=$(PROFILE_BENCH)

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
