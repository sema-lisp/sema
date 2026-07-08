<script setup>
import CustomPageLayout from './CustomPageLayout.vue'
</script>

<template>
  <CustomPageLayout v-slot="{ copyText }">

    <!-- ============ HERO (homepage-only: distinct, wide, editorial) ============ -->
    <header class="hero home-hero">
      <div class="wrap">
        <div class="hero-head">
          <p class="eyebrow">Agent-native Lisp<span class="sep">·</span>LLM workflows<span class="sep">·</span>Rust<span class="sep">·</span>MIT</p>
          <h1>Agent-native language.<br><em>Runtime you trust.</em></h1>
        </div>
        <div class="hero-body">
          <p class="lede">
            Sema builds the agent plumbing into the language itself.
            <strong>Model calls, typed tools, budgets, deterministic replay,
            journaled runs, OpenTelemetry traces, and single-binary deploys</strong>
            are primitives, not scaffolding — so the workflows your coding agent
            writes stay small, inspectable, and easy to constrain, replay, and ship.
          </p>
          <div class="hero-actions">
            <a class="btn btn-gold" href="/docs/">Get started</a>
            <a class="btn btn-ghost" href="https://sema.run">Try it in the browser</a>
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
          <p class="req">macOS · Linux · Windows · single static binary, no toolchain required</p>
        </div>
      </div>
    </header>

    <!-- ============ COMPARISON ============ -->
    <section id="compare">
      <div class="wrap">
        <p class="kicker">The argument</p>
        <h2>The same agent, twice.</h2>
        <p class="sub">
          A coding agent that reads files and runs commands, with a tool loop,
          retries, and a spend limit. Once with an SDK, once in Sema.
        </p>

        <div class="compare">
          <div class="pane python">
            <div class="pane-head">
              <span class="t">agent.py — Python + SDK</span>
              <span class="n">you write the machinery</span>
            </div>
            <pre><span class="c-kw">import</span> anthropic, time

client = anthropic.Anthropic()

TOOLS = [{
    <span class="c-str">"name"</span>: <span class="c-str">"read_file"</span>,
    <span class="c-str">"description"</span>: <span class="c-str">"Read a file's contents"</span>,
    <span class="c-str">"input_schema"</span>: {
        <span class="c-str">"type"</span>: <span class="c-str">"object"</span>,
        <span class="c-str">"properties"</span>: {<span class="c-str">"path"</span>: {<span class="c-str">"type"</span>: <span
                class="c-str">"string"</span>}},
        <span class="c-str">"required"</span>: [<span class="c-str">"path"</span>],
    },
}, {
    <span class="c-str">"name"</span>: <span class="c-str">"run_command"</span>,
    <span class="c-str">"description"</span>: <span class="c-str">"Run a shell command"</span>,
    <span class="c-str">"input_schema"</span>: { <span class="c-com"># ...same again</span> },
}]

<span class="c-kw">def</span> <span class="c-fn">call_with_retry</span>(messages, attempt=0):
    <span class="c-kw">try</span>:
        <span class="c-kw">return</span> client.messages.create(
            model=MODEL, max_tokens=4096,
            tools=TOOLS, messages=messages)
    <span class="c-kw">except</span> anthropic.RateLimitError:
        <span class="c-kw">if</span> attempt > 5: <span class="c-kw">raise</span>
        time.sleep(2 ** attempt)
        <span class="c-kw">return</span> call_with_retry(messages, attempt + 1)

<span class="c-kw">def</span> <span class="c-fn">dispatch</span>(name, args):
    <span class="c-kw">if</span> name == <span class="c-str">"read_file"</span>:
        <span class="c-kw">return</span> open(args[<span class="c-str">"path"</span>]).read()
    <span class="c-kw">if</span> name == <span class="c-str">"run_command"</span>:
        <span class="c-com"># subprocess, capture stdout+stderr...</span>

messages = [{<span class="c-str">"role"</span>: <span class="c-str">"user"</span>, <span class="c-str">"content"</span>: task}]
<span class="c-kw">for</span> turn <span class="c-kw">in</span> range(10):
    resp = call_with_retry(messages)
    track_cost(resp.usage)  <span class="c-com"># you wrote this too</span>
    <span class="c-kw">if</span> resp.stop_reason != <span class="c-str">"tool_use"</span>:
        <span class="c-kw">break</span>
    results = []
    <span class="c-kw">for</span> block <span class="c-kw">in</span> resp.content:
        <span class="c-kw">if</span> block.type == <span class="c-str">"tool_use"</span>:
            results.append({
                <span class="c-str">"type"</span>: <span class="c-str">"tool_result"</span>,
                <span class="c-str">"tool_use_id"</span>: block.id,
                <span class="c-str">"content"</span>: dispatch(block.name, block.input),
            })
    messages.append(...)
<div class="fade"></div></pre>
            <div class="pane-foot">
              And there's still no response cache, no hard spend cap, no fallback
              provider. That's another dependency — or another hundred lines.
            </div>
          </div>

          <div class="pane sema">
            <div class="pane-head">
              <span class="t">agent.sema — Sema</span>
              <span class="n">the machinery is the language</span>
            </div>
            <pre>(<span class="c-kw">deftool</span> <span class="c-fn">read-file</span>
  <span class="c-str">"Read a file's contents"</span>
  {<span class="c-kwd">:path</span> {<span class="c-kwd">:type</span> <span class="c-kwd">:string</span>}}
  (<span class="c-kw">lambda</span> (path) (file/read path)))

(<span class="c-kw">deftool</span> <span class="c-fn">run-command</span>
  <span class="c-str">"Run a shell command"</span>
  {<span class="c-kwd">:command</span> {<span class="c-kwd">:type</span> <span class="c-kwd">:string</span>}}
  (<span class="c-kw">lambda</span> (command)
    (<span class="c-kwd">:stdout</span> (shell <span class="c-str">"sh"</span> <span class="c-str">"-c"</span> command))))

(<span class="c-kw">defagent</span> <span class="c-fn">coder</span>
  {<span class="c-kwd">:system</span>    <span class="c-str">"You are a coding assistant."</span>
   <span class="c-kwd">:tools</span>     [read-file run-command]
   <span class="c-kwd">:max-turns 10</span>})

(llm/with-budget {<span class="c-kwd">:max-cost-usd</span> 0.50}
  (<span class="c-kw">lambda</span> ()
    (agent/run coder <span class="c-str">"Find TODOs in src/"</span>)))</pre>
            <div class="pane-foot">
              The tool loop, retries with backoff, rate limiting, and cost tracking
              live in the runtime. The spend cap is a <em>scope</em> — it can't be
              forgotten on the late-night code path.
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ AGENT-NATIVE MEANS CHECKABLE ============ -->
    <section id="checkable">
      <div class="wrap">
        <p class="kicker">Why agent-native matters</p>
        <h2>Agent-native means checkable.</h2>
        <p class="sub">
          Generated code is only useful if you can constrain it. Sema workflows are
          ordinary code, but the runtime sees the boundaries that matter:
        </p>

        <div class="checks">
          <div class="check">
            <span class="n">01</span>
            <code>(agent/run coder task)</code>
            <h3>Every call passes through the runtime</h3>
            <p>Model calls, tool dispatches, results, and retries are all things the
              runtime can observe — not logic buried inside an SDK.</p>
          </div>
          <div class="check">
            <span class="n">02</span>
            <code>(llm/with-budget {:max-cost-usd 1.00} f)</code>
            <h3>Budgets and checkpoints are scopes</h3>
            <p>A spend cap or a resume point is part of the run — not a comment the
              late-night code path can forget.</p>
          </div>
          <div class="check">
            <span class="n">03</span>
            <code>(llm/extract {:amount :number} text)</code>
            <h3>Outputs are values, not free text</h3>
            <p>Schema-backed tools and extraction hand back typed data. Nothing
              downstream has to re-parse a blob of prose.</p>
          </div>
        </div>

        <p class="sub" style="margin-top:30px">
          So a run can be replayed from cassettes, traced with OpenTelemetry, resumed
          from a journal, and shipped as one binary — and, <em>coming soon</em>, guarded
          by executable policies instead of “please be safe” prompt vibes.
        </p>
      </div>
    </section>

    <!-- ============ AGENT / LISP OBJECTION ============ -->
    <section id="agents">
      <div class="wrap">
        <p class="kicker">Why Lisp?</p>
        <h2>Because the agent has to write it.</h2>
        <p class="sub">
          Sema is built as a small, stable target for generated programs — the
          language with the least surface for an agent to be wrong about. The code is
          already data, so the runtime can inspect it, check it, journal it, and replay it.
        </p>

        <div class="agent-grid">
          <ul class="claims">
            <li>
              <strong>Sixty years of training data.</strong>
              Lisp predates nearly everything else in the corpus. Scheme, Common Lisp,
              Clojure, Racket — your agent has read all of it, and a Lisp is a Lisp.
            </li>
            <li>
              <strong>Nothing to hallucinate.</strong>
              One syntax rule. No borrow checker, no venv, no lockfiles, no build
              config, no framework versions that drifted since training. The agent
              can't misremember machinery that doesn't exist.
            </li>
            <li>
              <strong>The whole language fits in context.</strong>
              Point your agent at <a href="/docs/for-agents"><mark>one short page</mark></a> —
              where Sema diverges from the dialects it already knows, and nothing else.
              Constraints, not a textbook.
            </li>
            <li>
              <strong>Errors self-correct.</strong>
              Dialect drift is the shallow kind of wrong: <em>“oh, it's
              <code>equal?</code> here, not <code>string=?</code>”</em> — one check,
              one fix, moving on.
            </li>
          </ul>

          <div class="term">
            <div class="head">claude — working in pipeline repo</div>
            <div><span class="dollar">$</span> claude "add urgency classification to the ticket pipeline"</div>
            <div><span class="dot">●</span> Read pipeline.sema, llms.txt</div>
            <div><span class="dot">●</span> Edit pipeline.sema</div>
            <div class="out">&nbsp;&nbsp;(llm/classify [:low :medium :urgent] (:body ticket))</div>
            <div><span class="dot">●</span> Run sema check pipeline.sema</div>
            <div class="err">&nbsp;&nbsp;✗ unbound symbol: string=?</div>
            <div><span class="dot">●</span> llms.txt → "use equal? for all equality" — fixed, re-ran</div>
            <div class="ok">&nbsp;&nbsp;✓ pipeline.sema ok</div>
            <div class="out">one self-correction. zero questions for you.</div>
          </div>
        </div>

        <p class="symmetry">
          Sema is LLM-native in <span class="hl">both directions</span>: LLMs are
          primitives in the language — and the language is a target LLMs write
          without special training.
        </p>
      </div>
    </section>

    <!-- ============ OBJECTIONS ============ -->
    <section id="why">
      <div class="wrap">
        <p class="kicker">The other fair questions</p>
        <h2>“Why not just—”</h2>

        <div class="objections">
          <div class="obj">
            <h3><span class="q">…a Python script</span> with the SDK?</h3>
            <p>
              That's where everyone starts, and it's fine — until the script matters.
              Then you bolt on retries, then a cache so dev runs stop costing money,
              then cost tracking, then the second provider. The scaffolding ends up
              bigger than the idea.
            </p>
            <p>
              In Sema those are forms, not code you maintain:
              <code>llm/with-cache</code>, <code>llm/with-budget</code>,
              <code>llm/with-fallback</code>, <code>defagent</code>.
            </p>
          </div>

          <div class="obj">
            <h3><span class="q">…a framework</span> like LangChain?</h3>
            <p>
              Frameworks stack abstractions on a language that wasn't built for them —
              so a "chain" is a class, a prompt is a template object, a conversation
              is hidden inside an opaque memory wrapper.
            </p>
            <p>
              Sema makes them language constructs instead. A conversation is an
              immutable value you can fork, diff, and inspect. A prompt is an
              s-expression. A tool is a lambda with a schema. There's nothing to
              wrap, because nothing is foreign.
            </p>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ RUNTIME STRIP ============ -->
    <section id="runtime">
      <div class="wrap">
        <p class="kicker">The runtime, in one screen</p>
        <h2>The agent runtime, not another framework.</h2>

        <div class="forms">
          <div class="form-row"><code>(llm/with-budget {:max-cost-usd 1.00} f)</code><span>hard spend cap, scoped to a block</span>
          </div>
          <div class="form-row"><code>(llm/with-cache {:ttl 3600} f)</code><span>response cache — dev loops stop costing money</span>
          </div>
          <div class="form-row"><code>(llm/with-fallback [:anthropic :openai]
            f)</code><span>provider failover, in order</span></div>
          <div class="form-row"><code>(llm/extract {:amount {:type :number}} text)</code><span>typed maps back, not strings to re-parse</span>
          </div>
          <div class="form-row"><code>(conversation/say conv "...")</code><span>immutable history — fork it, replay it, inspect it</span>
          </div>
          <div class="form-row"><code>(llm/pmap prompt-fn items)</code><span>parallel batch over a collection</span>
          </div>
        </div>
        <p class="sub" style="margin-top:22px">
          Eight chat providers plus embedding providers, configured from environment variables — set the key and go.
          <a href="/docs/llm/">Browse the LLM reference →</a>
        </p>
      </div>
    </section>

    <!-- ============ SHIP ============ -->
    <section id="ship">
      <div class="wrap">
        <p class="kicker">Then ship it</p>
        <div class="ship-grid">
          <div>
            <h2>One file out the other end.</h2>
            <p class="sub">
              The part Python never solved. No virtualenv on the server, no
              dependency pinning, no container just to run a script.
            </p>
            <ul class="ship-list">
              <li><span><strong>Standalone executables.</strong> <code
                style="font-family:var(--font-mono);font-size:13px">sema build</code> traces your imports, bundles assets, and emits a self-contained binary.</span>
              </li>
              <li><span><strong>Capability sandbox.</strong> <code style="font-family:var(--font-mono);font-size:13px">--sandbox</code> fences shell, filesystem, network, and LLM access per group.</span>
              </li>
              <li><span><strong>Starts in milliseconds.</strong> Fast enough for a git hook, a cron job, or a CI step — no JVM tax, no import dance.</span>
              </li>
            </ul>
          </div>
          <div class="ship-term">
            <div><span class="dollar">$</span> sema build agent.sema -o agent</div>
            <div class="out">→ traced 3 imports, bundled 1 asset</div>
            <div class="out">→ agent (self-contained, 12 MB)</div>
            <div>&nbsp;</div>
            <div><span class="dollar">$</span> scp agent prod: &amp;&amp; ssh prod ./agent</div>
            <div class="out">→ runs. that's it.</div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ HONEST ============ -->
    <section class="honest" id="honest">
      <div class="wrap">
        <p class="kicker">Read this before adopting</p>
        <h2>Where Sema won't fit.</h2>
        <p class="sub">Knowing the boundaries up front beats discovering them in production.</p>
        <ul class="honest-list">
          <li><strong>Single-threaded.</strong> Rc-based values, no cross-thread sharing. Parallelism is at the LLM-call
            level, not the compute level.
          </li>
          <li><strong>No JIT.</strong> A bytecode compiler and a stack-based VM. If your bottleneck is number crunching, use Rust
            — or embed Sema in it.
          </li>
          <li><strong>Not a full Scheme.</strong> No call/cc, auto-gensym instead of syntax-rules.
          </li>
          <li><strong>Young.</strong> Solid and tested, not battle-hardened at scale. Pin a version; read the <a
            href="https://github.com/sema-lisp/sema/blob/main/CHANGELOG.md">changelog</a>.
          </li>
        </ul>
      </div>
    </section>

    <!-- ============ CTA ============ -->
    <section class="cta">
      <div class="wrap">
        <h2>Your next LLM script, without the scaffolding.</h2>
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
            <span class="badge">brew</span>
            <span class="install">
            <span class="cmd-text">
              <span class="dollar">$</span>
              <span id="i3">brew install helgesverre/tap/sema-lang</span>
            </span>
            <button class="copy" @click="copyText('i3', $event)">copy</button>
          </span>
          </div>
          <div class="install-row">
            <span class="badge">cargo</span>
            <span class="install">
            <span class="cmd-text">
              <span class="dollar">$</span>
              <span id="i4">cargo install sema-lang</span>
            </span>
            <button class="copy" @click="copyText('i4', $event)">copy</button>
          </span>
          </div>
          <div class="install-row">
            <span class="badge agent">agent</span>
            <span class="install">
            <span class="cmd-text">
              <span class="dollar">$</span>
              <span id="i5">curl -fsSL https://sema-lang.com/docs/for-agents.md >> AGENTS.md</span>
            </span>
            <button class="copy" @click="copyText('i5', $event)">copy</button>
          </span>
          </div>
          <div class="hero-actions" style="justify-content:center; margin-top:24px">
            <a class="btn btn-gold" href="https://sema.run">Open the playground</a>
            <a class="btn btn-ghost" href="https://github.com/sema-lisp/sema">View source</a>
          </div>
        </div>
      </div>
    </section>

  </CustomPageLayout>
</template>

<style scoped>
/* ---------- hero (homepage-only: distinct raised band, wide, two-column) ---------- */
.home-hero {
  background: var(--bg-raise);
  border-bottom: 1px solid var(--border);
  padding: clamp(76px, 9vw, 124px) 0;
}
.home-hero .wrap {
  max-width: 1280px;
  display: grid;
  grid-template-columns: 1.12fr 0.88fr;
  gap: clamp(40px, 5.5vw, 92px);
  align-items: center;
}
.home-hero .hero-head { min-width: 0; }
.home-hero .hero-head .eyebrow { margin-bottom: 24px; }
.home-hero .hero-head h1 {
  font-size: clamp(46px, 5.2vw, 88px);
  line-height: 1.03;
  max-width: 15ch;
  margin-bottom: 0;
}
.home-hero .hero-body { min-width: 0; max-width: 52ch; }
.home-hero .hero-body .lede { font-size: 18px; max-width: none; margin-bottom: 30px; }
.home-hero .hero-body .hero-actions { margin-bottom: 16px; }
.home-hero .hero-body .req { margin-top: 8px; }

@media (max-width: 900px) {
  .home-hero { padding: 64px 0 56px; }
  .home-hero .wrap { grid-template-columns: 1fr; gap: 34px; align-items: start; }
  .home-hero .hero-head h1 { max-width: 20ch; }
  .home-hero .hero-body { max-width: 60ch; }
}

/* ---------- checkable cards ---------- */
.checks {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 20px;
  margin-top: 44px;
}
.check {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 24px 22px 26px;
  transition: border-color .18s var(--ease);
}
.check:hover { border-color: var(--gold-line); }
.check .n {
  font-family: var(--font-mono); font-size: 12px;
  letter-spacing: .12em; color: var(--gold);
}
.check code {
  display: block; margin: 12px 0 18px;
  font-family: var(--font-mono); font-size: 12.5px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  border: 1px solid var(--gold-line);
  border-radius: 7px;
  padding: 9px 11px;
  overflow-x: auto; white-space: nowrap;
}
.check h3 {
  font-family: var(--font-display); font-weight: 400;
  font-size: 21px; line-height: 1.2; margin: 0 0 9px;
}
.check p {
  color: var(--muted); font-size: 14.5px; line-height: 1.55; margin: 0;
}
@media (max-width: 860px) {
  .checks { grid-template-columns: 1fr; }
}

/* ---------- comparison ---------- */
.compare {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 22px;
  margin-top: 46px;
  align-items: start;
}

.pane {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
}

.pane.sema {
  border-color: var(--gold-line);
  box-shadow: 0 0 0 1px rgba(200, 168, 85, .08), 0 24px 60px -30px rgba(200, 168, 85, .12);
}

.pane-head {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  gap: 10px;
  padding: 13px 18px;
  border-bottom: 1px solid var(--border-lo);
  font-family: var(--font-mono);
  font-size: 12px;
}

.pane-head .t {
  color: var(--text);
}

.pane.sema .pane-head .t {
  color: var(--gold-bright);
}

.pane-head .n {
  color: var(--dim);
}

.pane pre {
  font-family: var(--font-mono);
  font-size: 12.5px;
  line-height: 1.62;
  padding: 18px 20px;
  overflow-x: auto;
  color: #c9c2b4;
}

.pane.python pre {
  position: relative;
  max-height: 560px;
  overflow-y: hidden;
  color: #9b9486;
}

.pane.python .fade {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 140px;
  background: linear-gradient(transparent, var(--bg-raise));
  pointer-events: none;
}

.pane-foot {
  padding: 13px 18px;
  border-top: 1px solid var(--border-lo);
  font-size: 13px;
  color: var(--muted);
  line-height: 1.55;
}

.pane.sema .pane-foot {
  color: var(--text);
}

/* ---------- agent section ---------- */
.agent-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 48px;
  margin-top: 46px;
  align-items: start;
}

.claims {
  list-style: none;
}

.claims li {
  padding: 16px 0;
  border-bottom: 1px solid var(--border-lo);
  font-size: 15px;
  color: var(--muted);
  line-height: 1.65;
}

.claims li:first-child {
  padding-top: 4px;
}

.claims strong {
  color: var(--text);
  font-weight: 500;
  display: block;
  margin-bottom: 3px;
}

.claims code, .claims mark {
  font-family: var(--font-mono);
  font-size: 12.5px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
}

.symmetry {
  margin-top: 54px;
  padding: 30px 34px;
  border: 1px solid var(--gold-line);
  border-radius: 12px;
  background: var(--gold-fade);
  font-family: var(--font-display);
  font-style: italic;
  font-size: clamp(19px, 2.3vw, 24px);
  line-height: 1.45;
  color: var(--text);
  max-width: 880px;
}

.symmetry .hl {
  color: var(--gold-bright);
}

/* ---------- objections ---------- */
.objections {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 22px;
  margin-top: 46px;
}

.obj {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 26px 24px;
}

.obj h3 {
  font-family: var(--font-display);
  font-style: italic;
  font-weight: 400;
  font-size: 22px;
  line-height: 1.25;
  margin-bottom: 14px;
  color: var(--text);
}

.obj h3 .q {
  color: var(--gold);
}

.obj p {
  font-size: 14.5px;
  color: var(--muted);
  line-height: 1.65;
}

.obj p + p {
  margin-top: 10px;
}

.obj code {
  font-family: var(--font-mono);
  font-size: 12.5px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
}

/* ---------- runtime strip ---------- */
.forms {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 1px;
  background: var(--border-lo);
  border: 1px solid var(--border-lo);
  border-radius: 12px;
  overflow: hidden;
  margin-top: 46px;
}

.form-row {
  background: var(--bg-raise);
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 6px 18px;
  padding: 18px 22px;
}

.form-row code {
  font-family: var(--font-mono);
  font-size: 13px;
  color: var(--gold-bright);
  white-space: nowrap;
}

.form-row span {
  font-size: 13.5px;
  color: var(--muted);
}

/* ---------- ship ---------- */
.ship-grid {
  display: grid;
  grid-template-columns: 1.1fr .9fr;
  gap: 48px;
  align-items: center;
  margin-top: 8px;
}

.ship-term {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  font-family: var(--font-mono);
  font-size: 13px;
  line-height: 1.9;
  padding: 22px 24px;
  color: #c9c2b4;
}

.ship-term .dollar {
  color: var(--gold);
}

.ship-term .out {
  color: var(--dim);
}

.ship-list {
  list-style: none;
  margin-top: 28px;
}

.ship-list li {
  display: flex;
  gap: 14px;
  padding: 10px 0;
  font-size: 15px;
  color: var(--muted);
}

.ship-list li::before {
  content: "›";
  color: var(--gold);
  font-family: var(--font-mono);
}

.ship-list strong {
  color: var(--text);
  font-weight: 500;
}

/* ---------- honest ---------- */
.honest {
  background: var(--bg-raise);
}

.honest-list {
  columns: 2;
  column-gap: 60px;
  margin-top: 30px;
  max-width: 880px;
}

.honest-list li {
  break-inside: avoid;
  list-style: none;
  padding: 8px 0;
  font-size: 15px;
  color: var(--muted);
  border-bottom: 1px solid var(--border-lo);
}

.honest-list li strong {
  color: var(--text);
  font-weight: 500;
}

/* ---------- responsive ---------- */
@media (max-width: 880px) {
  .hero { padding: 72px 0 60px; }

  .compare, .objections, .ship-grid, .agent-grid {
    grid-template-columns: 1fr;
  }

  .forms {
    grid-template-columns: 1fr;
  }

  .honest-list {
    columns: 1;
  }
}
</style>
