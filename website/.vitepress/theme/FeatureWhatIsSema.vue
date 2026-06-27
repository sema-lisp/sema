<script setup>
import CustomPageLayout from './CustomPageLayout.vue'
</script>

<template>
  <CustomPageLayout active-nav="what-is-sema" v-slot="{ copyText }">

    <!-- ============ HERO ============ -->
    <header class="hero">
      <span class="hero-paren l" aria-hidden="true">(</span>
      <span class="hero-paren r" aria-hidden="true">)</span>
      <div class="wrap">
        <p class="eyebrow">Overview<span class="sep">·</span>Language<span class="sep">·</span>Toolchain<span class="sep">·</span>Runtime</p>
        <h1>What is <em>Sema?</em></h1>
        <p class="lede">
          Sema is a <strong>Scheme-like Lisp</strong> with a
          <strong>Clojure-flavored surface</strong> and
          <strong>first-class LLM/agent primitives</strong>, compiled to a
          NaN-boxed bytecode VM. Single-threaded, reference-counted, embeddable.
          Implemented in Rust.
        </p>
        <div class="hero-actions">
          <a class="btn btn-gold" href="/docs/">Read the docs</a>
          <a class="btn btn-ghost" href="https://sema.run">Try the playground</a>
        </div>
        <div class="hero-actions">
          <span class="install">
            <span class="cmd-text">
              <span class="dollar">$</span>
              <span id="i1">curl -fsSL https://sema-lang.com/install.sh | sh</span>
            </span>
            <button class="copy" @click="copyText('i1', $event)">copy</button>
          </span>
        </div>
        <p class="req">v1.27.1 · MIT · Rust 2021 · 16 crates · ~125k lines</p>
      </div>
    </header>

    <!-- ============ THE LANGUAGE ============ -->
    <section id="language">
      <div class="wrap">
        <p class="kicker">The language</p>
        <h2>A Lisp you can hold in your head.</h2>
        <p class="sub">
          One syntax rule: everything is an s-expression. The surface borrows
          from Clojure — keywords, maps, vectors, short lambdas, f-strings —
          while the semantics stay Scheme at the core: tail-call optimization,
          quasiquote macros, <code>define</code>/<code>set!</code>, lexical scope.
        </p>

        <div class="lang-split">
          <div class="code-card">
            <div class="code-card-head">
              <span class="t">surface.sema</span>
              <span class="n">syntax you write</span>
            </div>
            <pre>(<span class="c-kw">define</span> <span class="c-fn">greet</span>
  (<span class="c-kw">fn</span> (name)
    f<span class="c-str">"Hello, ${name}!"</span>))

(<span class="c-kw">define</span> person
  {<span class="c-kwd">:name</span> <span class="c-str">"Ada"</span>
   <span class="c-kwd">:age</span> 36})

(<span class="c-kwd">:name</span> person)              <span class="c-com">; keyword as getter</span>
(<span class="c-kw">map</span> #(* % %) (range 1 6))  <span class="c-com">; short lambda</span>
(<span class="c-kw">match</span> (:status res)
  <span class="c-kwd">:ok</span>    (:data res)
  <span class="c-kwd">:error</span> (<span class="c-kw">throw</span> (:message res)))</pre>
          </div>

          <div class="lang-features">
            <div class="feature-bite">
              <span class="bite-label">Clojure surface</span>
              <p><code>:keywords</code>, <code>{:k v}</code> maps, <code>[1 2 3]</code> vectors, <code>#(* % %)</code> lambdas, <code>f"..."</code> strings, <code>#"regex"</code> literals.</p>
            </div>
            <div class="feature-bite">
              <span class="bite-label">Scheme core</span>
              <p><code>define</code>, <code>set!</code>, <code>lambda</code>/<code>fn</code>, <code>let</code>/<code>let*</code>/<code>letrec</code>, <code>if</code>/<code>cond</code>/<code>case</code>, <code>begin</code>, <code>and</code>/<code>or</code>, tail-call optimization.</p>
            </div>
            <div class="feature-bite">
              <span class="bite-label">Modern conveniences</span>
              <p>Threading macros (<code>-&gt;</code>, <code>-&gt;&gt;</code>), pattern matching (<code>match</code>), destructuring in <code>let</code>/<code>define</code>/fn params, <code>when-let</code>/<code>if-let</code>.</p>
            </div>
            <div class="feature-bite">
              <span class="bite-label">Error handling</span>
              <p><code>try</code>/<code>catch</code>/<code>throw</code> — caught errors are structured maps with <code>:type</code>, <code>:message</code>, <code>:value</code>, and <code>:stack-trace</code>.</p>
            </div>
            <div class="feature-bite">
              <span class="bite-label">Async</span>
              <p><code>async</code>/<code>await</code> + channels — a deterministic cooperative scheduler, not OS threads.</p>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ DATA TYPES ============ -->
    <section id="data-types">
      <div class="wrap">
        <p class="kicker">Data types</p>
        <h2>Classic types, plus LLM as first-class.</h2>
        <p class="sub">
          Prompts, messages, conversations, tools, and agents are values —
          the same as integers and strings. They can be bound, passed, inspected,
          and stored. That's the defining difference.
        </p>

        <div class="types-grid">
          <div class="type-group">
            <div class="type-group-label">Scalars</div>
            <div class="type-tags">
              <span class="type-tag">Integer</span>
              <span class="type-tag">Float</span>
              <span class="type-tag">String</span>
              <span class="type-tag">Boolean</span>
              <span class="type-tag">Nil</span>
              <span class="type-tag">Symbol</span>
              <span class="type-tag">Keyword</span>
              <span class="type-tag">Character</span>
            </div>
          </div>

          <div class="type-group">
            <div class="type-group-label">Collections</div>
            <div class="type-tags">
              <span class="type-tag">List<span class="tag-note">vector-backed</span></span>
              <span class="type-tag">Vector</span>
              <span class="type-tag">Map<span class="tag-note">sorted BTreeMap</span></span>
              <span class="type-tag">HashMap<span class="tag-note">unordered</span></span>
              <span class="type-tag">Bytevector</span>
            </div>
          </div>

          <div class="type-group type-group-llm">
            <div class="type-group-label">LLM primitives <span class="llm-badge">first-class</span></div>
            <div class="type-tags">
              <span class="type-tag type-tag-llm">Prompt</span>
              <span class="type-tag type-tag-llm">Message</span>
              <span class="type-tag type-tag-llm">Conversation</span>
              <span class="type-tag type-tag-llm">Tool</span>
              <span class="type-tag type-tag-llm">Agent</span>
            </div>
          </div>

          <div class="type-group">
            <div class="type-group-label">Special</div>
            <div class="type-tags">
              <span class="type-tag">Promise<span class="tag-note">lazy</span></span>
              <span class="type-tag">Record</span>
              <span class="type-tag">Async Promise</span>
              <span class="type-tag">Channel<span class="tag-note">bounded FIFO</span></span>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ TOOLCHAIN ============ -->
    <section id="toolchain">
      <div class="wrap">
        <p class="kicker">The toolchain</p>
        <h2>What's in the box.</h2>
        <p class="sub">
          One binary, <code>sema</code>, gives you the REPL, script runner,
          bytecode compiler, standalone executable builder, formatter, LSP,
          DAP debugger, notebook server, and MCP server.
        </p>

        <div class="toolchain-grid">
          <div class="tool-card">
            <div class="tool-name">REPL</div>
            <div class="tool-desc">Interactive prompt with history, auto-completion, and syntax highlighting. <code>sema</code> with no args.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">Script runner</div>
            <div class="tool-desc">Run <code>.sema</code> files, inline expressions with <code>--eval</code>, and shebang scripts.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">Bytecode compiler</div>
            <div class="tool-desc">Lowering → optimization → resolution → compilation. Inspect with <code>sema compile</code> / <code>sema disasm</code>.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">Standalone executables</div>
            <div class="tool-desc"><code>sema build</code> traces imports, bundles assets, emits a self-contained binary. No toolchain needed at runtime.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">Formatter</div>
            <div class="tool-desc"><code>sema fmt</code> — opinionated code formatter for <code>.sema</code> files.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">LSP server</div>
            <div class="tool-desc">Completions, hover, go-to-definition, references, rename, semantic tokens, folding, inlay hints. <code>sema lsp</code>.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">DAP debugger</div>
            <div class="tool-desc">Breakpoints, step in/over/out, stack traces, variable inspection via VM debug hooks. <code>sema dap</code>.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">Notebook server</div>
            <div class="tool-desc">Jupyter-inspired <code>.sema-nb</code> format, shared-env cells, REST API, browser UI. <code>sema notebook serve</code>.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">MCP server</div>
            <div class="tool-desc">Model Context Protocol server exposing Sema eval/build/notebook tools to AI agents. <code>sema mcp</code>.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">Editor plugins</div>
            <div class="tool-desc">VS Code, Vim/Neovim, Emacs, Helix, Zed, IntelliJ — syntax highlighting, formatting, LSP integration.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">WASM playground</div>
            <div class="tool-desc">Runs in the browser at <a href="https://sema.run">sema.run</a> and embeddable in web apps via <code>wasm-bindgen</code>.</div>
          </div>
          <div class="tool-card">
            <div class="tool-name">Embedding</div>
            <div class="tool-desc">Rust crate <code>sema-lang</code> with a builder API. <code>Interpreter::new().eval_str("(+ 1 2)")</code>.</div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ LLM LAYER ============ -->
    <section id="llm-layer">
      <div class="wrap">
        <div class="feature-row">
          <div class="feature-text">
            <p class="kicker">The LLM layer</p>
            <h2>Not an SDK. A language.</h2>
            <p class="sub">
              LLM operations are forms and values, not library calls wrapped
              in boilerplate. The runtime handles retries, caching, cost tracking,
              provider fallback, and rate limiting — so your code stays the size
              of its idea.
            </p>
            <ul class="feature-list">
              <li><strong>Eight chat providers.</strong> Anthropic, OpenAI, Gemini, Groq, xAI, Mistral, Moonshot, Ollama — auto-configured from environment variables. Plus Jina, Voyage, and Cohere for embeddings.</li>
              <li><strong>Tools &amp; agents.</strong> <code>deftool</code> defines a function with a schema. <code>defagent</code> defines a system prompt + tools + turn limit. <code>agent/run</code> handles the loop.</li>
              <li><strong>Conversations as data.</strong> Immutable, forkable, inspectable. <code>conversation/say</code> returns a new value — the old one is untouched.</li>
              <li><strong>Cassettes.</strong> Record LLM calls to a file, replay them forever. Deterministic tests without API keys.</li>
              <li><strong>Cost &amp; budgets.</strong> <code>llm/with-budget</code> caps spend for a scope. Token usage tracked per call and per session.</li>
              <li><strong>Observability.</strong> Built-in OpenTelemetry tracing with GenAI conventions. Off by default.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="code-card">
              <div class="code-card-head">
                <span class="t">llm.sema</span>
                <span class="n">primitives, not boilerplate</span>
              </div>
              <pre>(<span class="c-kw">deftool</span> <span class="c-fn">get-weather</span>
  <span class="c-str">"Get weather for a city"</span>
  {<span class="c-kwd">:city</span> {<span class="c-kwd">:type</span> <span class="c-kwd">:string</span>}}
  (<span class="c-kw">lambda</span> (city)
    (format <span class="c-str">"~a: 22°C"</span> city)))

(<span class="c-kw">defagent</span> <span class="c-fn">bot</span>
  {<span class="c-kwd">:system</span> <span class="c-str">"Weather assistant."</span>
   <span class="c-kwd">:tools</span> [get-weather]
   <span class="c-kwd">:max-turns</span> 3})

(<span class="c-kw">llm/with-budget</span>
  {<span class="c-kwd">:max-cost-usd</span> 0.10}
  (<span class="c-kw">lambda</span> ()
    (agent/run bot
      <span class="c-str">"Weather in Oslo?"</span>)))
<span class="c-com">;; =&gt; "It's 22°C in Oslo."</span>
<span class="c-com">;;    $0.003 · 1 tool call · 2 turns</span></pre>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ DIFFERENCES ============ -->
    <section id="differences">
      <div class="wrap">
        <p class="kicker">How it differs from other Lisps</p>
        <h2>The things that surprise people.</h2>

        <div class="diff-grid">
          <div class="diff-card">
            <div class="diff-from">Common Lisp / Scheme</div>
            <div class="diff-to">Sema</div>
            <div class="diff-body">
              <p>Only <code>#f</code> and <code>nil</code> are falsy. <code>0</code>, <code>""</code>, and <code>()</code> are all <strong>truthy</strong>. In CL, the empty list is false.</p>
            </div>
          </div>

          <div class="diff-card">
            <div class="diff-from">Cons cells</div>
            <div class="diff-to">Vector-backed lists</div>
            <div class="diff-body">
              <p>Lists are <code>Rc&lt;Vec&lt;Value&gt;&gt;</code> — O(1) <code>nth</code>, O(n) <code>cons</code>. Prefer <code>map</code>/<code>filter</code>/<code>fold</code> and <code>vector</code> for hot paths.</p>
            </div>
          </div>

          <div class="diff-card">
            <div class="diff-from">Clojure atoms</div>
            <div class="diff-to">define + set!</div>
            <div class="diff-body">
              <p>Mutable state is <code>(define x 0)</code> + <code>(set! x 1)</code>. No <code>atom</code>/<code>swap!</code>/<code>reset!</code>.</p>
            </div>
          </div>

          <div class="diff-card">
            <div class="diff-from">One map type</div>
            <div class="diff-to">Two map types</div>
            <div class="diff-body">
              <p><code>{:k v}</code> literals are sorted <code>BTreeMap</code>s — deterministic order, usable as keys. <code>(hashmap/new)</code> is faster and unordered.</p>
            </div>
          </div>

          <div class="diff-card">
            <div class="diff-from">syntax-rules</div>
            <div class="diff-to">auto-gensym</div>
            <div class="diff-body">
              <p>Macros use <code>defmacro</code> with quasiquote. Symbols ending in <code>#</code> inside quasiquote are auto-unique — no variable capture.</p>
            </div>
          </div>

          <div class="diff-card">
            <div class="diff-from">No LLM types</div>
            <div class="diff-to">LLM as first-class</div>
            <div class="diff-body">
              <p>Prompt, Message, Conversation, Tool, Agent are values — alongside integers and strings. This is why Sema exists.</p>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ ARCHITECTURE ============ -->
    <section id="architecture">
      <div class="wrap">
        <div class="feature-row reverse">
          <div class="feature-text">
            <p class="kicker">Under the hood</p>
            <h2>NaN-boxed values, bytecode VM.</h2>
            <p class="sub">
              Every value is a single 8-byte <code>struct Value(u64)</code> —
              encoded in IEEE 754 quiet-NaN payload space. The sole evaluator is
              a stack-based bytecode VM with intrinsic opcodes and NaN-boxed fast
              paths. No tree-walking interpreter.
            </p>
            <ul class="feature-list">
              <li><strong>Single-threaded.</strong> <code>Rc</code>-based values, no cross-thread sharing. Parallelism is at the LLM-call level, not the compute level.</li>
              <li><strong>No GC.</strong> Deterministic destruction via reference counting. Memory is freed the moment the last reference drops.</li>
              <li><strong>16 crates.</strong> Strict dependency ordering: <code>sema-core ← sema-reader ← sema-vm ← sema-eval ← sema</code>. Stdlib and LLM depend on core, not eval — dependency inversion via callbacks.</li>
              <li><strong>Bytecode format.</strong> <code>.semac</code> files with a 24-byte header, string table, function table, main chunk. <code>sema build</code> embeds the runtime + bytecode into a standalone binary.</li>
            </ul>
            <p class="sub" style="margin-top:18px">
              <a href="/docs/internals/architecture">Architecture reference &rarr;</a>
            </p>
          </div>
          <div class="feature-visual">
            <div class="bytecode-viz">
              <!-- Source → compiled pipeline strip -->
              <div class="bc-pipeline">
                <span class="bc-pipe-step">.sema</span>
                <span class="bc-pipe-arrow">&rarr;</span>
                <span class="bc-pipe-step">reader</span>
                <span class="bc-pipe-arrow">&rarr;</span>
                <span class="bc-pipe-step">lower</span>
                <span class="bc-pipe-arrow">&rarr;</span>
                <span class="bc-pipe-step bc-pipe-step-hot">VM</span>
              </div>

              <!-- .semac file layout — layered hex header + sections -->
              <div class="bc-file">
                <div class="bc-file-label">.semac file layout</div>

                <div class="bc-hex-header">
                  <div class="bc-hex-group">
                    <span class="bc-hex-byte bc-hex-magic">00</span>
                    <span class="bc-hex-byte bc-hex-magic">53</span>
                    <span class="bc-hex-byte bc-hex-magic">45</span>
                    <span class="bc-hex-byte bc-hex-magic">4D</span>
                    <span class="bc-hex-note">magic</span>
                  </div>
                  <div class="bc-hex-group">
                    <span class="bc-hex-byte">04</span><span class="bc-hex-byte">00</span>
                    <span class="bc-hex-note">v4</span>
                  </div>
                  <div class="bc-hex-group">
                    <span class="bc-hex-byte">00</span><span class="bc-hex-byte">00</span>
                    <span class="bc-hex-note">flags</span>
                  </div>
                  <div class="bc-hex-group bc-hex-dots">
                    <span class="bc-hex-byte bc-hex-faded">··</span><span class="bc-hex-byte bc-hex-faded">··</span><span class="bc-hex-byte bc-hex-faded">··</span><span class="bc-hex-byte bc-hex-faded">··</span>
                    <span class="bc-hex-note">version</span>
                  </div>
                  <div class="bc-hex-group">
                    <span class="bc-hex-byte">03</span><span class="bc-hex-byte">00</span>
                    <span class="bc-hex-note">3 sections</span>
                  </div>
                  <div class="bc-hex-group bc-hex-dots">
                    <span class="bc-hex-byte bc-hex-faded">··</span><span class="bc-hex-byte bc-hex-faded">··</span><span class="bc-hex-byte bc-hex-faded">··</span><span class="bc-hex-byte bc-hex-faded">··</span>
                    <span class="bc-hex-note">reserved</span>
                  </div>
                </div>
                <div class="bc-hex-size">24 bytes</div>

                <div class="bc-sections">
                  <div class="bc-section bc-section-required">
                    <div class="bc-sec-head"><span class="bc-sec-id">0x01</span><span class="bc-sec-name">String Table</span><span class="bc-sec-tag">required</span></div>
                    <div class="bc-sec-body">interned strings &middot; Spur remapping</div>
                  </div>
                  <div class="bc-section bc-section-required">
                    <div class="bc-sec-head"><span class="bc-sec-id">0x02</span><span class="bc-sec-name">Function Table</span><span class="bc-sec-tag">required</span></div>
                    <div class="bc-sec-body">compiled function templates</div>
                  </div>
                  <div class="bc-section bc-section-required bc-section-main">
                    <div class="bc-sec-head"><span class="bc-sec-id">0x03</span><span class="bc-sec-name">Main Chunk</span><span class="bc-sec-tag">bytecode</span></div>
                    <div class="bc-sec-body">
                      <span class="bc-op">Const</span>
                      <span class="bc-op">LoadLocal0</span>
                      <span class="bc-op">Call</span>
                      <span class="bc-op">Pop</span>
                      <span class="bc-op">Const</span>
                      <span class="bc-op">CallGlobal</span>
                      <span class="bc-op">Return</span>
                    </div>
                  </div>
                </div>
              </div>

              <!-- Value encoding -->
              <div class="bc-value-box">
                <div class="bc-value-label">NaN-boxed Value</div>
                <div class="bc-value-visual">
                  <div class="bc-value-part bc-value-tag"><span>tag</span><em>6 bits</em></div>
                  <div class="bc-value-part bc-value-payload"><span>payload</span><em>45 bits</em></div>
                </div>
                <div class="bc-value-note"><code>struct Value(u64)</code> — every type in 8 bytes</div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ NAMING ============ -->
    <section id="naming">
      <div class="wrap">
        <p class="kicker">Naming conventions</p>
        <h2>Slash-namespaced. Predicates with <code>?</code>. Arrows for conversions.</h2>
        <p class="sub">The conventions are the API contract — get these right and the stdlib falls into place.</p>

        <div class="naming-grid">
          <div class="naming-card">
            <div class="naming-pattern"><code>file/read</code></div>
            <div class="naming-desc">Slash-namespaced functions. <code>string/split</code>, <code>http/get</code>, <code>regex/match?</code>, <code>json/encode</code>. Never <code>read-file</code> or <code>split-string</code>.</div>
          </div>
          <div class="naming-card">
            <div class="naming-pattern"><code>empty?</code></div>
            <div class="naming-desc">Predicates end in <code>?</code>. <code>null?</code>, <code>list?</code>, <code>file/exists?</code>, <code>equal?</code>.</div>
          </div>
          <div class="naming-card">
            <div class="naming-pattern"><code>string-&gt;symbol</code></div>
            <div class="naming-desc">Conversions use <code>-&gt;</code>. <code>keyword-&gt;string</code>, <code>list-&gt;vector</code>, <code>string-&gt;number</code>.</div>
          </div>
          <div class="naming-card">
            <div class="naming-pattern"><code>string-append</code></div>
            <div class="naming-desc">Legacy Scheme names kept for a few string ops. <code>string-length</code>, <code>string-ref</code>, <code>substring</code> — no <code>string/</code> prefix on these.</div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ CTA ============ -->
    <section class="cta">
      <div class="wrap">
        <h2>Now go build something with it.</h2>
        <p class="sub">Install it — or skip the tutorial and hand the docs to your agent.</p>
        <div class="install-stack">
          <div class="install-row">
            <span class="badge">curl</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="i2">curl -fsSL https://sema-lang.com/install.sh | sh</span>
              </span>
              <button class="copy" @click="copyText('i2', $event)">copy</button>
            </span>
          </div>
          <div class="install-row">
            <span class="badge">cargo</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="i3">cargo install sema-lang</span>
              </span>
              <button class="copy" @click="copyText('i3', $event)">copy</button>
            </span>
          </div>
          <div class="install-row">
            <span class="badge agent">agent</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="i4">curl -fsSL https://sema-lang.com/docs/for-agents.md &gt;&gt; AGENTS.md</span>
              </span>
              <button class="copy" @click="copyText('i4', $event)">copy</button>
            </span>
          </div>
          <div class="hero-actions" style="justify-content:center; margin-top:24px">
            <a class="btn btn-gold" href="/docs/">Get started</a>
            <a class="btn btn-ghost" href="https://sema.run">Open the playground</a>
          </div>
        </div>
      </div>
    </section>

  </CustomPageLayout>
</template>

<style scoped>
/* ---------- hero ---------- */
.hero { padding: 104px 0 56px; }
.req code { font-family: var(--font-mono); color: var(--muted); }

/* ---------- language section ---------- */
.lang-split {
  display: grid;
  grid-template-columns: 1.1fr .9fr;
  gap: 32px;
  margin-top: 40px;
  align-items: start;
}

.lang-features {
  display: flex;
  flex-direction: column;
  gap: 18px;
}

.feature-bite {
  padding: 14px 18px;
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 10px;
}

.bite-label {
  font-family: var(--font-mono);
  font-size: 12px;
  font-weight: 500;
  color: var(--gold-bright);
  margin-bottom: 4px;
  display: block;
}

.feature-bite p {
  font-size: 13.5px;
  color: var(--muted);
  line-height: 1.55;
}

.feature-bite code {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
}

/* ---------- code card ---------- */
.code-card {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
  box-shadow: 0 0 0 1px rgba(200, 168, 85, .04), 0 20px 50px -30px rgba(0, 0, 0, .3);
}

.code-card-head {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  gap: 10px;
  padding: 13px 18px;
  border-bottom: 1px solid var(--border-lo);
  font-family: var(--font-mono);
  font-size: 12px;
}

.code-card-head .t { color: var(--gold-bright); }
.code-card-head .n { color: var(--dim); }

.code-card pre {
  font-family: var(--font-mono);
  font-size: 12.5px;
  line-height: 1.62;
  padding: 18px 20px;
  overflow-x: auto;
  color: #c9c2b4;
}

/* ---------- data types ---------- */
.types-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 18px;
  margin-top: 40px;
}

.type-group {
  padding: 20px 22px;
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
}

.type-group-llm {
  border-color: var(--gold-line);
  box-shadow: 0 0 0 1px rgba(200, 168, 85, .06);
}

.type-group-label {
  font-family: var(--font-mono);
  font-size: 12px;
  font-weight: 500;
  color: var(--gold);
  text-transform: uppercase;
  letter-spacing: 0.1em;
  margin-bottom: 14px;
  display: flex;
  align-items: center;
  gap: 8px;
}

.llm-badge {
  font-size: 9px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 2px 7px;
  border-radius: 4px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.type-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.type-tag {
  font-family: var(--font-mono);
  font-size: 12.5px;
  color: var(--text);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 5px 10px;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.type-tag-llm {
  color: var(--gold-bright);
  border-color: var(--gold-line);
  background: var(--gold-fade);
}

.tag-note {
  font-size: 10px;
  color: var(--dim);
  font-style: italic;
}

/* ---------- toolchain ---------- */
.toolchain-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 16px;
  margin-top: 40px;
}

.tool-card {
  padding: 18px 20px;
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 10px;
}

.tool-name {
  font-family: var(--font-mono);
  font-size: 13px;
  font-weight: 500;
  color: var(--gold-bright);
  margin-bottom: 8px;
}

.tool-desc {
  font-size: 13px;
  color: var(--muted);
  line-height: 1.55;
}

.tool-desc code {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
  white-space: nowrap;
}

/* ---------- feature rows ---------- */
.feature-row {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 56px;
  align-items: center;
  margin-top: 20px;
}

.feature-row.reverse .feature-text { order: 2; }
.feature-row.reverse .feature-visual { order: 1; }

.feature-list {
  margin-top: 24px;
}

.feature-list li {
  padding: 10px 0;
  font-size: 14.5px;
  color: var(--muted);
  line-height: 1.65;
  border-bottom: 1px solid var(--border-lo);
}

.feature-list li:last-child { border-bottom: none; }

.feature-list strong {
  color: var(--text);
  font-weight: 500;
  display: block;
  margin-bottom: 2px;
}

.feature-list code {
  font-family: var(--font-mono);
  font-size: 12.5px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
}

/* ---------- differences ---------- */
.diff-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 16px;
  margin-top: 40px;
}

.diff-card {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 10px;
  overflow: hidden;
}

.diff-from {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--dim);
  padding: 8px 14px;
  border-bottom: 1px solid var(--border-lo);
  background: var(--bg);
}

.diff-to {
  font-family: var(--font-mono);
  font-size: 12px;
  font-weight: 500;
  color: var(--gold-bright);
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-lo);
}

.diff-body {
  padding: 14px;
}

.diff-body p {
  font-size: 13px;
  color: var(--muted);
  line-height: 1.6;
}

.diff-body code {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
}

/* ---------- bytecode visualization ---------- */
.bytecode-viz {
  display: flex;
  flex-direction: column;
  gap: 20px;
  perspective: 800px;
}

/* pipeline strip */
.bc-pipeline {
  display: flex;
  align-items: center;
  gap: 6px;
  font-family: var(--font-mono);
  font-size: 11.5px;
  flex-wrap: wrap;
}

.bc-pipe-step {
  padding: 4px 10px;
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 5px;
  color: var(--muted);
}

.bc-pipe-step-hot {
  color: var(--gold-bright);
  border-color: var(--gold-line);
  background: var(--gold-fade);
}

.bc-pipe-arrow {
  color: var(--dim);
  font-size: 11px;
}

/* .semac file layout */
.bc-file {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 16px;
  transform: rotateX(4deg);
  transform-style: preserve-3d;
}

.bc-file-label {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--dim);
  text-transform: uppercase;
  letter-spacing: 0.1em;
  margin-bottom: 12px;
}

/* hex header */
.bc-hex-header {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  align-items: flex-start;
}

.bc-hex-group {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 3px;
}

.bc-hex-byte {
  display: inline-block;
  font-family: var(--font-mono);
  font-size: 10.5px;
  color: var(--text);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 3px;
  padding: 2px 5px;
  min-width: 26px;
  text-align: center;
  line-height: 1.3;
}

.bc-hex-magic {
  color: var(--gold-bright);
  border-color: var(--gold-line);
  background: var(--gold-fade);
}

.bc-hex-faded {
  color: var(--dim);
  border-color: var(--border-lo);
  background: transparent;
}

.bc-hex-note {
  font-family: var(--font-mono);
  font-size: 8.5px;
  color: var(--dim);
  margin-top: 1px;
}

.bc-hex-size {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--dim);
  margin-top: 6px;
  text-align: right;
}

/* sections */
.bc-sections {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 14px;
}

.bc-section {
  background: var(--bg);
  border: 1px solid var(--border-lo);
  border-radius: 6px;
  overflow: hidden;
  transition: transform .15s;
}

.bc-section:hover {
  transform: translateX(4px);
}

.bc-section-main {
  border-color: var(--gold-line);
  box-shadow: 0 0 0 1px rgba(200, 168, 85, .06);
}

.bc-sec-head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border-lo);
  font-family: var(--font-mono);
  font-size: 11.5px;
}

.bc-sec-id {
  color: var(--dim);
  font-size: 10px;
}

.bc-sec-name {
  color: var(--text);
  font-weight: 500;
}

.bc-section-main .bc-sec-name {
  color: var(--gold-bright);
}

.bc-sec-tag {
  margin-left: auto;
  font-size: 9px;
  color: var(--dim);
  background: var(--surface);
  padding: 2px 6px;
  border-radius: 3px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.bc-sec-body {
  padding: 8px 12px;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--muted);
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.bc-op {
  padding: 2px 6px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 3px;
  color: var(--gold-bright);
  font-size: 10.5px;
}

/* value encoding box */
.bc-value-box {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 14px 16px;
}

.bc-value-label {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--dim);
  text-transform: uppercase;
  letter-spacing: 0.1em;
  margin-bottom: 10px;
}

.bc-value-visual {
  display: flex;
  gap: 0;
  border-radius: 6px;
  overflow: hidden;
}

.bc-value-part {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  padding: 10px;
  font-family: var(--font-mono);
  font-size: 11px;
}

.bc-value-part span {
  font-size: 10px;
  color: var(--dim);
  text-transform: uppercase;
  letter-spacing: 0.08em;
}

.bc-value-part em {
  font-style: normal;
  font-size: 12px;
  font-weight: 500;
}

.bc-value-tag {
  background: var(--gold-fade);
  border: 1px solid var(--gold-line);
  color: var(--gold-bright);
  flex: 0 0 25%;
}

.bc-value-payload {
  background: var(--surface);
  border: 1px solid var(--border);
  border-left: none;
  color: var(--text);
  flex: 1;
}

.bc-value-note {
  font-family: var(--font-mono);
  font-size: 10.5px;
  color: var(--dim);
  margin-top: 8px;
}

.bc-value-note code {
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 3px;
}

/* ---------- naming ---------- */
.naming-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 16px;
  margin-top: 40px;
}

.naming-card {
  padding: 18px 20px;
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 10px;
}

.naming-pattern {
  font-family: var(--font-mono);
  font-size: 14px;
  font-weight: 500;
  color: var(--gold-bright);
  margin-bottom: 8px;
}

.naming-pattern code {
  background: var(--gold-fade);
  padding: 2px 8px;
  border-radius: 5px;
}

.naming-desc {
  font-size: 13px;
  color: var(--muted);
  line-height: 1.55;
}

.naming-desc code {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
}

/* ---------- responsive ---------- */
@media (max-width: 880px) {
  .hero { padding: 72px 0 48px; }

  .glance-grid { grid-template-columns: 1fr; }

  .lang-split { grid-template-columns: 1fr; }

  .types-grid { grid-template-columns: 1fr; }

  .toolchain-grid { grid-template-columns: 1fr; }

  .diff-grid { grid-template-columns: 1fr; }

  .naming-grid { grid-template-columns: 1fr; }

  .feature-row, .feature-row.reverse {
    grid-template-columns: 1fr;
  }
  .feature-row.reverse .feature-text { order: unset; }
  .feature-row.reverse .feature-visual { order: unset; }
}
</style>
