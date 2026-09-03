<script setup>
import CustomPageLayout from './CustomPageLayout.vue'

// __SEMA_VERSION__ is inlined from the workspace Cargo.toml by vite.define in
// .vitepress/config.ts, so a release does not leave a stale number in the hero.
// Omitted if the file could not be read.
const versionLabel = __SEMA_VERSION__ ? `v${__SEMA_VERSION__} · ` : ''
</script>

<template>
  <CustomPageLayout v-slot="{ copyText }">

    <header class="hero home-hero">
      <span class="hero-paren l" aria-hidden="true">(</span>
      <span class="hero-paren r" aria-hidden="true">)</span>
      <div class="wrap">
        <p class="eyebrow">
          Lisp<span class="sep">·</span>LLM primitives<span class="sep">·</span>Rust runtime
        </p>
        <h1>A Lisp with <em>LLM primitives.</em></h1>
        <p class="lede">
          Sema is a <strong>Scheme-like Lisp</strong> with built-in agents,
          typed tools, budgets, replay, and tracing. It runs on a Rust bytecode
          VM and builds to one standalone executable.
        </p>
        <div class="hero-actions">
          <a class="btn btn-gold" href="/docs/quickstart">Get started</a>
          <a class="btn btn-ghost" href="https://sema.run">Try the playground</a>
        </div>
        <div class="hero-install">
          <div class="hero-actions">
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="home-install">curl -fsSL https://sema-lang.com/install.sh | sh</span>
              </span>
              <button
                type="button"
                class="copy"
                aria-live="polite"
                @click="copyText('home-install', $event)"
              >
                copy
              </button>
            </span>
          </div>
          <a class="install-alternatives" href="/docs/#installation">
            Windows, Homebrew, Cargo, and other install methods
            <span aria-hidden="true">&rarr;</span>
          </a>
        </div>
        <p class="req">{{ versionLabel }}MIT · macOS, Linux, and Windows</p>
      </div>
    </header>

    <section id="agent" aria-labelledby="agent-title">
      <div class="wrap">
        <div class="feature-row">
          <div class="feature-text">
            <p class="kicker">Agents and tools</p>
            <h2 id="agent-title">The whole agent fits on one screen.</h2>
            <p class="sub">
              Model calls, tools, conversations, and agents have distinct runtime
              types. Sema handles provider requests, tool-result correlation, and
              turn limits so the program can state the work and its controls.
            </p>
            <ul class="feature-list">
              <li>
                <strong>Tools have schemas.</strong>
                <code>deftool</code> connects a description and input schema to
                an ordinary Sema function.
              </li>
              <li>
                <strong>The agent loop is built in.</strong>
                <code>defagent</code> declares the prompt, tools, and turn limit;
                <code>agent/run</code> handles calls and tool results.
              </li>
              <li>
                <strong>Limits are scoped.</strong>
                Token caps and known-price cost limits apply to every model call
                inside <code>llm/with-budget</code>.
              </li>
              <li>
                <strong>Runs can be replayed.</strong>
                A cassette records provider traffic once, then reuses the exact
                responses in local development and tests.
              </li>
            </ul>
            <a class="section-link" href="/feature/agents">
              Explore agents and tools <span aria-hidden="true">&rarr;</span>
            </a>
          </div>

          <figure class="feature-visual code-card">
            <figcaption class="code-card-head">
              <span class="t">weather.sema</span>
              <span class="n">tool · agent · controls</span>
            </figcaption>
            <pre tabindex="0"><code>(<span class="c-kw">deftool</span> <span class="c-fn">get-weather</span>
  <span class="c-str">"Get weather for a city"</span>
  {<span class="c-kwd">:city</span> {<span class="c-kwd">:type</span> <span class="c-kwd">:string</span>}}
  (<span class="c-kw">lambda</span> (city)
    (http/get f<span class="c-str">"https://weather.test/${city}"</span>)))

(<span class="c-kw">defagent</span> <span class="c-fn">guide</span>
  {<span class="c-kwd">:system</span> <span class="c-str">"Give a short forecast."</span>
   <span class="c-kwd">:tools</span> [get-weather]
   <span class="c-kwd">:max-turns</span> 3})

(llm/with-cassette
  <span class="c-str">"weather.jsonl"</span> {<span class="c-kwd">:mode</span> <span class="c-kwd">:auto</span>}
  (<span class="c-kw">lambda</span> ()
    (llm/with-budget
      {<span class="c-kwd">:max-cost-usd</span> 0.10}
      (<span class="c-kw">lambda</span> ()
        (agent/run guide
          <span class="c-str">"Weather in Oslo?"</span>)))))</code></pre>
          </figure>
        </div>
      </div>
    </section>

    <section id="language" class="raised" aria-labelledby="language-title">
      <div class="wrap">
        <p class="kicker">The language</p>
        <h2 id="language-title">Small enough to learn. Complete enough to use.</h2>
        <p class="sub">
          Sema keeps Scheme's lexical scope, functions, macros, and tail calls,
          then adds the data forms people expect in current programs.
        </p>

        <div class="language-split">
          <figure class="code-card">
            <figcaption class="code-card-head">
              <span class="t">surface.sema</span>
              <span class="n">one syntax rule</span>
            </figcaption>
            <pre tabindex="0"><code>(<span class="c-kw">define</span> tickets
  [{<span class="c-kwd">:id</span> 101 <span class="c-kwd">:priority</span> <span class="c-kwd">:high</span>}
   {<span class="c-kwd">:id</span> 102 <span class="c-kwd">:priority</span> <span class="c-kwd">:low</span>}])

(<span class="c-kw">define</span> urgent-ids
  (-&gt;&gt; tickets
    (filter
      #(<span class="c-kw">equal?</span> (<span class="c-kwd">:priority</span> %) <span class="c-kwd">:high</span>))
    (map <span class="c-kwd">:id</span>)))

(<span class="c-kw">match</span> urgent-ids
  ([]       <span class="c-str">"nothing urgent"</span>)
  ([id &amp; _] f<span class="c-str">"start with ticket ${id}"</span>))</code></pre>
          </figure>

          <dl class="language-notes">
            <div>
              <dt>Scheme core</dt>
              <dd>
                Lexical scope, first-class functions, tail-call optimization,
                quasiquote macros, and structured exceptions.
              </dd>
            </div>
            <div>
              <dt>Clojure-flavored data</dt>
              <dd>
                Keywords, maps, vectors, short lambdas, threading macros,
                destructuring, pattern matching, and f-strings.
              </dd>
            </div>
            <div>
              <dt>One runtime</dt>
              <dd>
                REPL, formatter, LSP, debugger, notebook, MCP server, compiler,
                and executable builder ship in the <code>sema</code> binary.
              </dd>
            </div>
            <div>
              <dt>Designed for generated code</dt>
              <dd>
                A uniform syntax and a concise agent guide give coding agents a
                small, stable target.
                <a href="/docs/for-agents">Read the guide &rarr;</a>
              </dd>
            </div>
          </dl>
        </div>
      </div>
    </section>

    <section id="runtime" aria-labelledby="runtime-title">
      <div class="wrap">
        <p class="kicker">The runtime, in one screen</p>
        <h2 id="runtime-title">Control is part of the program.</h2>
        <p class="sub">
          These forms wrap ordinary functions. They compose, nest, and clean up
          when the scope exits.
        </p>

        <div class="forms">
          <a class="form-row" href="/docs/llm/cost">
            <code>(llm/with-budget limits f)</code>
            <span>cap tokens or known-price cost for a block</span>
          </a>
          <a class="form-row" href="/docs/llm/caching">
            <code>(llm/with-cache {:ttl 3600} f)</code>
            <span>reuse responses during development</span>
          </a>
          <a class="form-row" href="/feature/cassettes">
            <code>(llm/with-cassette path opts f)</code>
            <span>record and replay provider traffic</span>
          </a>
          <a class="form-row" href="/docs/llm/resilience">
            <code>(llm/with-fallback providers f)</code>
            <span>try an ordered provider chain</span>
          </a>
          <a class="form-row" href="/feature/observability">
            <code>(with-span "ingest" attrs body...)</code>
            <span>emit OpenTelemetry spans around a run</span>
          </a>
          <a class="form-row" href="/feature/workflows">
            <code>(checkpoint :result value)</code>
            <span>journal work and resume completed phases</span>
          </a>
        </div>
      </div>
    </section>

    <section id="uses" class="raised" aria-labelledby="uses-title">
      <div class="wrap">
        <div class="section-heading">
          <div>
            <p class="kicker">What it is for</p>
            <h2 id="uses-title">From one model call to a complete workflow.</h2>
          </div>
          <p class="sub">
            Start with a script. Add tools, structure, persistence, or an
            interactive notebook without changing the language or runtime.
          </p>
        </div>

        <div class="work-list">
          <article class="work-row">
            <span class="work-index">01</span>
            <div>
              <h3>Agents and tools</h3>
              <p>Run bounded tool loops with typed inputs, callbacks, and recoverable tool errors.</p>
            </div>
            <a href="/feature/agents">Agents <span aria-hidden="true">&rarr;</span></a>
          </article>
          <article class="work-row">
            <span class="work-index">02</span>
            <div>
              <h3>Structured extraction</h3>
              <p>Turn unstructured model output into validated maps, lists, and scalar values.</p>
            </div>
            <a href="/feature/extraction">Extraction <span aria-hidden="true">&rarr;</span></a>
          </article>
          <article class="work-row">
            <span class="work-index">03</span>
            <div>
              <h3>Journaled workflows</h3>
              <p>Split long work into phases, record checkpoints, bound fan-out, and resume a run.</p>
            </div>
            <a href="/feature/workflows">Workflows <span aria-hidden="true">&rarr;</span></a>
          </article>
          <article class="work-row">
            <span class="work-index">04</span>
            <div>
              <h3>Notebooks</h3>
              <p>Develop in shared-state cells, inspect rich output, then run the same code as a script.</p>
            </div>
            <a href="/feature/notebook">Notebook <span aria-hidden="true">&rarr;</span></a>
          </article>
        </div>
      </div>
    </section>

    <section id="ship" aria-labelledby="ship-title">
      <div class="wrap">
        <div class="feature-row reverse">
          <div class="feature-text">
            <p class="kicker">Then ship it</p>
            <h2 id="ship-title">A script in. One executable out.</h2>
            <p class="sub">
              <code>sema build</code> traces imports, bundles declared assets,
              and writes a standalone executable. The target machine does not
              need Sema, Cargo, or another language runtime.
            </p>
            <ul class="feature-list">
              <li>
                <strong>Cross-platform builds.</strong>
                Target macOS, Linux, and Windows from the same source.
              </li>
              <li>
                <strong>Capability sandbox.</strong>
                Restrict filesystem, network, shell, process, environment, and
                LLM access at the CLI boundary.
              </li>
              <li>
                <strong>Embeddable when needed.</strong>
                Run Sema inside Rust, JavaScript, or a browser through the same VM.
              </li>
            </ul>
            <a class="section-link" href="/feature/build">
              See standalone executables <span aria-hidden="true">&rarr;</span>
            </a>
          </div>

          <figure class="feature-visual term build-term">
            <figcaption class="head">terminal</figcaption>
            <pre tabindex="0"><code><span><span class="dollar">$</span> sema build weather.sema -o weather</span>
<span class="out">  traced imports and bundled assets</span>
<span class="ok">  built ./weather</span>

<span><span class="dollar">$</span> scp weather server:</span>
<span><span class="dollar">$</span> ssh server ./weather Oslo</span>
<span class="out">  runs without a Sema installation</span></code></pre>
          </figure>
        </div>
      </div>
    </section>

    <section id="providers" class="raised" aria-labelledby="providers-title">
      <div class="wrap provider-layout">
        <div class="provider-copy">
          <p class="kicker">Provider choice</p>
          <h2 id="providers-title">Switch models without rewriting the program.</h2>
          <p class="sub">
            Sema auto-configures providers from environment variables. Your
            tools, agents, budgets, and traces stay the same whether a run uses
            a hosted model, a local model, or an ordered fallback chain.
          </p>
          <a class="section-link" href="/docs/llm/providers">
            See all providers <span aria-hidden="true">&rarr;</span>
          </a>
        </div>

        <div class="provider-details">
          <dl class="provider-list">
            <div>
              <dt>Built-in chat</dt>
              <dd>
                <ul class="provider-tags" aria-label="Built-in chat providers">
                  <li>Anthropic</li>
                  <li>OpenAI</li>
                  <li>Gemini</li>
                  <li>Ollama</li>
                </ul>
              </dd>
            </div>
            <div>
              <dt>Compatible chat</dt>
              <dd>
                <ul class="provider-tags" aria-label="OpenAI-compatible chat providers">
                  <li>Groq</li>
                  <li>xAI</li>
                  <li>Mistral</li>
                  <li>DeepSeek</li>
                  <li>OpenRouter</li>
                  <li>Together</li>
                  <li class="provider-more">and more</li>
                </ul>
              </dd>
            </div>
            <div>
              <dt>Embed &amp; rerank</dt>
              <dd>
                <ul class="provider-tags" aria-label="Embedding and reranking providers">
                  <li>OpenAI</li>
                  <li>Jina</li>
                  <li>Voyage</li>
                  <li>Cohere</li>
                  <li>Nomic</li>
                  <li>Fireworks</li>
                </ul>
              </dd>
            </div>
          </dl>

          <figure class="provider-code code-card">
            <figcaption class="code-card-head">
              <span class="t">provider.sema</span>
              <span class="n">ordered fallback</span>
            </figcaption>
            <pre tabindex="0"><code>(llm/with-fallback
  [<span class="c-kwd">:anthropic</span> <span class="c-kwd">:openai</span> <span class="c-kwd">:ollama</span>]
  (<span class="c-kw">lambda</span> ()
    (agent/run guide question)))</code></pre>
          </figure>
        </div>
      </div>
    </section>

    <section class="cta" aria-labelledby="cta-title">
      <div class="wrap">
        <p class="kicker">Start with one file</p>
        <h2 id="cta-title">Build your next LLM program in Sema.</h2>
        <p class="sub">
          Try it in the browser, follow the quickstart, or give the concise
          language guide to your coding agent.
        </p>

        <div class="install-stack">
          <div class="install-row">
            <span class="badge">curl</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="cta-curl">curl -fsSL https://sema-lang.com/install.sh | sh</span>
              </span>
              <button
                type="button"
                class="copy"
                aria-live="polite"
                @click="copyText('cta-curl', $event)"
              >
                copy
              </button>
            </span>
          </div>
          <div class="install-row">
            <span class="badge">brew</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="cta-brew">brew install helgesverre/tap/sema-lang</span>
              </span>
              <button
                type="button"
                class="copy"
                aria-live="polite"
                @click="copyText('cta-brew', $event)"
              >
                copy
              </button>
            </span>
          </div>
          <div class="install-row">
            <span class="badge agent">agent</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="cta-agent">curl -fsSL https://sema-lang.com/docs/for-agents.md &gt;&gt; AGENTS.md</span>
              </span>
              <button
                type="button"
                class="copy"
                aria-live="polite"
                @click="copyText('cta-agent', $event)"
              >
                copy
              </button>
            </span>
          </div>
          <div class="hero-actions cta-actions">
            <a class="btn btn-gold" href="https://sema.run">Open the playground</a>
            <a class="btn btn-ghost" href="/docs/quickstart">Read the quickstart</a>
          </div>
        </div>
      </div>
    </section>

  </CustomPageLayout>
</template>

<style scoped>
/* ---------- hero ---------- */
.home-hero {
  padding: 118px 0 92px;
  border-bottom: 1px solid var(--border-lo);
}

.home-hero h1 {
  max-width: 12ch;
  font-size: clamp(48px, 7.2vw, 82px);
  text-wrap: balance;
}

.home-hero .lede {
  max-width: 59ch;
  text-wrap: pretty;
}

.hero-install {
  max-width: fit-content;
  margin-bottom: 22px;
}

.hero-install .hero-actions {
  margin-bottom: 8px;
}

.install-alternatives {
  display: inline-block;
  max-width: 100%;
  color: var(--gold-bright);
  font-family: var(--font-mono);
  font-size: 12px;
  text-wrap: pretty;
}

/* ---------- shared editorial rows ---------- */
.raised {
  background: var(--bg-raise);
}

.feature-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 56px;
  align-items: center;
  margin-top: 20px;
}

.feature-row.reverse .feature-text {
  order: 2;
}

.feature-row.reverse .feature-visual {
  order: 1;
}

.feature-text h2,
.section-heading h2 {
  text-wrap: balance;
}

.feature-text .sub,
.section-heading .sub {
  text-wrap: pretty;
}

.feature-list {
  margin-top: 24px;
}

.feature-list li {
  padding: 10px 0;
  border-bottom: 1px solid var(--border-lo);
  color: var(--muted);
  font-size: 14.5px;
  line-height: 1.65;
}

.feature-list li:last-child {
  border-bottom: 0;
}

.feature-list strong {
  display: block;
  margin-bottom: 2px;
  color: var(--text);
  font-weight: 500;
}

.feature-list code,
.language-notes code {
  padding: 1px 5px;
  border-radius: 4px;
  background: var(--gold-fade);
  color: var(--gold-bright);
  font-family: var(--font-mono);
  font-size: 12.5px;
}

.section-link {
  display: inline-block;
  margin-top: 22px;
  font-size: 14px;
}

/* ---------- code ---------- */
.code-card {
  min-width: 0;
  margin: 0;
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--bg-raise);
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

.code-card-head .t {
  color: var(--gold-bright);
}

.code-card-head .n {
  color: var(--dim);
}

.code-card pre {
  overflow-x: auto;
  padding: 20px 22px;
  color: #c9c2b4;
  font-family: var(--font-mono);
  font-size: 12.5px;
  line-height: 1.62;
}

.code-card code {
  font-family: inherit;
}

/* ---------- language ---------- */
.language-split {
  display: grid;
  grid-template-columns: minmax(0, 1.08fr) minmax(0, .92fr);
  gap: 42px;
  align-items: start;
  margin-top: 42px;
}

.language-notes {
  margin: 0;
  border-top: 1px solid var(--border);
}

.language-notes > div {
  padding: 17px 0;
  border-bottom: 1px solid var(--border-lo);
}

.language-notes dt {
  margin-bottom: 5px;
  color: var(--text);
  font-family: var(--font-mono);
  font-size: 12.5px;
}

.language-notes dd {
  margin: 0;
  color: var(--muted);
  font-size: 13.5px;
  line-height: 1.6;
}

/* ---------- runtime forms ---------- */
.forms {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 1px;
  margin-top: 46px;
  overflow: hidden;
  border: 1px solid var(--border-lo);
  border-radius: 12px;
  background: var(--border-lo);
}

.form-row {
  display: grid;
  gap: 6px;
  padding: 18px 22px;
  background: var(--bg-raise);
  transition: background .15s var(--ease);
}

.form-row:hover {
  background: var(--surface);
  text-decoration: none;
}

.form-row code {
  color: var(--gold-bright);
  font-family: var(--font-mono);
  font-size: 13px;
  white-space: nowrap;
}

.form-row span {
  color: var(--muted);
  font-size: 13.5px;
}

/* ---------- work list ---------- */
.section-heading {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(18rem, .72fr);
  gap: 64px;
  align-items: end;
  margin-bottom: 42px;
}

.work-list {
  border-top: 1px solid var(--border);
}

.work-row {
  display: grid;
  grid-template-columns: 52px minmax(0, 1fr) 110px;
  gap: 24px;
  align-items: center;
  padding: 24px 0;
  border-bottom: 1px solid var(--border-lo);
}

.work-index {
  color: var(--gold);
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: .1em;
}

.work-row h3 {
  margin-bottom: 4px !important;
  color: var(--text);
  font-family: var(--font-display);
  font-size: 24px;
  font-weight: 400;
  line-height: 1.2;
}

.work-row p {
  max-width: 60ch;
  color: var(--muted);
  font-size: 14px;
}

.work-row > a {
  justify-self: end;
  font-size: 13px;
  white-space: nowrap;
}

/* ---------- build terminal ---------- */
.build-term {
  margin: 0;
  padding: 0;
}

.build-term pre {
  overflow-x: auto;
  padding: 20px 22px;
  font-family: var(--font-mono);
  white-space: pre;
}

.build-term code {
  display: grid;
  font-family: inherit;
}

/* ---------- providers ---------- */
.provider-layout {
  display: grid;
  grid-template-columns: minmax(0, .82fr) minmax(0, 1.18fr);
  gap: 72px;
  align-items: start;
}

.provider-copy h2 {
  text-wrap: balance;
}

.provider-copy .sub {
  text-wrap: pretty;
}

.provider-list {
  margin: 0;
  border-top: 1px solid var(--border);
}

.provider-list > div {
  display: grid;
  grid-template-columns: 140px minmax(0, 1fr);
  gap: 18px;
  align-items: start;
  padding: 16px 0;
  border-bottom: 1px solid var(--border-lo);
}

.provider-list dt {
  padding-top: 5px;
  color: var(--gold);
  font-family: var(--font-mono);
  font-size: 12px;
}

.provider-list dd {
  margin: 0;
}

.provider-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.provider-tags li {
  display: inline-flex;
  align-items: center;
  padding: 5px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface);
  color: var(--text);
  font-family: var(--font-mono);
  font-size: 12px;
}

.provider-tags .provider-more {
  color: var(--muted);
}

.provider-code {
  margin-top: 24px;
}

/* ---------- CTA ---------- */
.cta .kicker {
  margin-bottom: 14px;
}

.cta-actions {
  justify-content: center;
  margin: 24px 0 0 !important;
}

/* ---------- responsive ---------- */
@media (max-width: 880px) {
  .home-hero {
    padding: 76px 0 64px;
  }

  .feature-row,
  .feature-row.reverse,
  .language-split,
  .section-heading,
  .provider-layout {
    grid-template-columns: 1fr;
  }

  .feature-row.reverse .feature-text,
  .feature-row.reverse .feature-visual {
    order: unset;
  }

  .section-heading {
    gap: 10px;
  }

  .forms {
    grid-template-columns: 1fr;
  }

}

@media (max-width: 640px) {
  .home-hero h1 {
    font-size: clamp(44px, 14vw, 62px);
  }

  .work-row {
    grid-template-columns: 36px minmax(0, 1fr);
    gap: 16px;
  }

  .work-row > a {
    grid-column: 2;
    justify-self: start;
  }

  .provider-list > div {
    grid-template-columns: 1fr;
    gap: 8px;
  }

  .provider-list dt {
    padding-top: 0;
  }

  .code-card pre {
    padding: 17px 18px;
    font-size: 11.5px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .form-row {
    transition: none;
  }
}
</style>
