<script setup>
import { ref } from 'vue'

// Mobile nav menu (hamburger) state.
const menuOpen = ref(false)
const toggleMenu = () => { menuOpen.value = !menuOpen.value }
const closeMenu = () => { menuOpen.value = false }

// TODO: extract into util function and re-use everywhere thisis needed
const copyText = (id, event) => {
  const el = document.getElementById(id);
  if (el) {
    navigator.clipboard.writeText(el.textContent.trim()).then(() => {
      const btn = event.currentTarget;
      const originalText = btn.textContent;
      btn.textContent = 'copied';
      setTimeout(() => {
        btn.textContent = originalText;
      }, 1400);
    });
  }
};
</script>

<template>
  <div class="custom-home">

    <nav>
      <div class="wrap nav-in">
        <a href="/" class="logo-link">
          <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 366.00 132.00" class="logo-svg">
            <path
              d="M48.5000 104.3000L48.5000 114Q34 110.7000 26.0500 100.5000Q18.1000 90.3000 18.1000 75L18.1000 57Q18.1000 41.7000 26.0500 31.5000Q34 21.3000 48.5000 18L48.5000 27.6000Q42.2000 29.1000 37.6000 33.1500Q33 37.2000 30.5000 43.3000Q28 49.4000 28 57L28 75Q28 82.6000 30.5000 88.6500Q33 94.7000 37.6000 98.7500Q42.2000 102.8000 48.5000 104.3000"
              fill="#c8a855" />
            <path
              d="M93.2000 102.8000L87.8000 102.8000Q79.4000 102.8000 74.2000 98.6000Q69 94.4000 69 86.8000L78.8000 86.8000Q78.8000 90.4000 81.4500 92.4500Q84.1000 94.5000 88.8000 94.5000L93.2000 94.5000Q98.1000 94.5000 100.7500 92.4000Q103.4000 90.3000 103.4000 86.5000Q103.4000 79.8000 96.8000 79L82 76.9000Q76.1000 76 72.9000 72.0500Q69.7000 68.1000 69.7000 61.8000Q69.7000 54.4000 74.7000 50.3000Q79.7000 46.2000 88.7000 46.2000L93.1000 46.2000Q101.5000 46.2000 106.7000 50.2000Q111.9000 54.2000 112.2000 60.8000L102.2000 60.8000Q102 58 99.6000 56.1500Q97.2000 54.3000 93.1000 54.3000L88.7000 54.3000Q84.2000 54.3000 81.7500 56.3000Q79.3000 58.3000 79.3000 61.7000Q79.3000 67.2000 84.8000 67.9000L98.7000 69.9000Q113 71.8000 113 86.5000Q113 94.3000 107.8500 98.5500Q102.7000 102.8000 93.2000 102.8000 M152 103Q142.1000 103 136.0500 97.1000Q130 91.2000 130 81L130 68Q130 57.8000 136.0500 51.9000Q142.1000 46 152 46Q158.6000 46 163.5500 48.6500Q168.5000 51.3000 171.2500 56.0500Q174 60.8000 174 67.1000L174 77L139.7000 77L139.7000 81.8000Q139.7000 87.8000 143 91.2000Q146.3000 94.6000 152 94.6000Q156.8000 94.6000 159.9000 92.8000Q163 91 163.6000 87.8000L173.5000 87.8000Q172.5000 94.8000 166.6000 98.9000Q160.7000 103 152 103M139.7000 67.1000L139.7000 69.7000L164.3000 69.7000L164.3000 67.1000Q164.3000 60.8000 161.1000 57.4000Q157.9000 54 152 54Q146.1000 54 142.9000 57.4000Q139.7000 60.8000 139.7000 67.1000 M197.7000 102L188.7000 102L188.7000 47L197.1000 47L197.1000 54.5000L197.4000 54.5000Q197.8000 50.7000 200.2500 48.3500Q202.7000 46 206.5000 46Q210.2000 46 212.7000 48.2000Q215.2000 50.4000 216.3000 54.1000Q216.9000 50.3000 219.4000 48.1500Q221.9000 46 225.7000 46Q230.9000 46 234.1000 49.9500Q237.3000 53.9000 237.3000 60.2000L237.3000 102L228.3000 102L228.3000 60.3000Q228.3000 57.2000 226.7500 55.3500Q225.2000 53.5000 222.6000 53.5000Q220 53.5000 218.5000 55.3000Q217 57.1000 217 60.3000L217 102L209 102L209 60.3000Q209 57.2000 207.5000 55.3500Q206 53.5000 203.4000 53.5000Q200.8000 53.5000 199.2500 55.3000Q197.7000 57.1000 197.7000 60.3000 M268.9000 103Q260.4000 103 255.4500 98.2500Q250.5000 93.5000 250.5000 85.8000Q250.5000 78.1000 255.6500 73.4000Q260.8000 68.7000 269.2000 68.7000L285.3000 68.7000L285.3000 64.5000Q285.3000 54.4000 274.1000 54.4000Q269.1000 54.4000 266.0500 56.2500Q263 58.1000 262.8000 61.4000L253 61.4000Q253.5000 54.7000 259.1000 50.3500Q264.7000 46 274.1000 46Q284.2000 46 289.7000 50.8000Q295.2000 55.6000 295.2000 64.3000L295.2000 102L285.5000 102L285.5000 91.9000L285.3000 91.9000Q284.6000 97 280.2500 100Q275.9000 103 268.9000 103M271.5000 94.7000Q277.8000 94.7000 281.5500 91.6500Q285.3000 88.6000 285.3000 83.3000L285.3000 76L270.1000 76Q265.8000 76 263.1500 78.5500Q260.5000 81.1000 260.5000 85.3000Q260.5000 89.6000 263.4000 92.1500Q266.3000 94.7000 271.5000 94.7000"
              fill="#ffffff" />
            <path
              d="M316.5000 114L316.5000 104.3000Q322.8000 102.8000 327.4000 98.7500Q332 94.7000 334.5500 88.6500Q337.1000 82.6000 337.1000 75L337.1000 57Q337.1000 49.4000 334.5500 43.3000Q332 37.2000 327.4000 33.1500Q322.8000 29.1000 316.5000 27.6000L316.5000 18Q331 21.3000 339 31.5000Q347 41.7000 347 57L347 75Q347 90.3000 339 100.5000Q331 110.7000 316.5000 114"
              fill="#c8a855" />
          </svg>
        </a>
        <button
          class="nav-toggle"
          :class="{ open: menuOpen }"
          :aria-expanded="menuOpen"
          aria-label="Toggle navigation menu"
          @click="toggleMenu"
        >
          <span></span><span></span><span></span>
        </button>
        <div class="nav-links" :class="{ open: menuOpen }" @click="closeMenu">
          <a href="/docs/">Guide</a>
          <a href="/docs/stdlib/">Stdlib</a>
          <a href="/docs/llm/">LLM</a>
          <a href="/docs/internals/architecture">Internals</a>
          <a href="/brand">Brand</a>
          <a href="https://sema.run" target="_blank" rel="noopener" class="vp-external-link-icon ">
            Playground
          </a>
          <a class="nav-gh" href="https://github.com/HelgeSverre/sema" aria-label="GitHub" target="_blank"
             rel="noopener">
            <svg viewBox="0 0 16 16" class="gh-svg" aria-hidden="true">
              <path fill="currentColor"
                    d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" />
            </svg>
          </a>
        </div>
      </div>
    </nav>

    <!-- ============ HERO ============ -->
    <header class="hero">
      <span class="hero-paren l" aria-hidden="true">(</span>
      <span class="hero-paren r" aria-hidden="true">)</span>
      <div class="wrap">
        <p class="eyebrow">A Lisp with LLM primitives<span class="sep">·</span>Rust<span class="sep">·</span>MIT</p>
        <h1>Stop rewriting <em>the agent loop.</em></h1>
        <p class="lede">
          Every LLM script grows the same scaffolding: retries, caching, cost caps,
          rate limits, tool dispatch, conversation state. <strong>Sema makes that
          scaffolding the runtime</strong> — your script stays the size of its idea,
          ships as a single binary, and your coding agent already speaks the language.
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

    <!-- ============ AGENT / LISP OBJECTION ============ -->
    <section id="agents">
      <div class="wrap">
        <p class="kicker">“Wait — a Lisp?”</p>
        <h2>You won't write most of it anyway.</h2>
        <p class="sub">
          Your coding agent will. And a Lisp is the language with the least surface
          for an agent to be wrong about.
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
              Point your agent at <a href="/llms.txt"><mark>llms.txt</mark></a> —
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
            <div class="out">&nbsp;&nbsp;(llm/classify {:labels [:low :medium :urgent]} (:body ticket))</div>
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
        <h2>Everything you'd otherwise hand-roll.</h2>

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
          Eleven providers, configured from environment variables — set the key and go.
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
          <li><strong>No JIT.</strong> A bytecode VM and a tree-walker. If your bottleneck is number crunching, use Rust
            — or embed Sema in it.
          </li>
          <li><strong>Not a full Scheme.</strong> No numeric tower, no call/cc, auto-gensym instead of syntax-rules.
          </li>
          <li><strong>Young.</strong> Solid and tested, not battle-hardened at scale. Pin a version; read the <a
            href="https://github.com/HelgeSverre/sema/blob/main/CHANGELOG.md">changelog</a>.
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
              <span id="i5">curl -fsSL https://sema-lang.com/llms.txt >> CLAUDE.md</span>
            </span>
            <button class="copy" @click="copyText('i5', $event)">copy</button>
          </span>
          </div>
          <div class="hero-actions" style="justify-content:center; margin-top:24px">
            <a class="btn btn-gold" href="https://sema.run">Open the playground</a>
            <a class="btn btn-ghost" href="https://github.com/HelgeSverre/sema">View source</a>
          </div>
        </div>
      </div>
    </section>

    <footer>
      <div class="wrap foot-in">
        <span><span style="color:var(--gold)">(</span>sema<span style="color:var(--gold)">)</span></span>
        <span>
        <a href="/docs/">Docs</a> ·
        <a href="/docs/internals/lisp-comparison.html">Benchmarks</a> ·
        <a href="https://github.com/HelgeSverre/sema/blob/main/CHANGELOG.md">Changelog</a> ·
        <a href="https://github.com/HelgeSverre/sema">GitHub</a>
      </span>
      </div>
    </footer>

  </div>
</template>

<style scoped>
.custom-home {
  /* brand */
  --gold: #c8a855;
  --gold-bright: #e3c878;
  --gold-fade: rgba(200, 168, 85, .09);
  --gold-line: rgba(200, 168, 85, .28);

  /* warm neutrals, tinted toward the gold */
  --bg: #131110;
  --bg-raise: #181512;
  --surface: #1c1916;
  --border: #2b2620;
  --border-lo: #221e19;
  --text: #e9e3d6;
  --muted: #968c79;
  --dim: #6b6354;

  /* On-brand thin scrollbars for any scroll region on the page (install
     snippets, code blocks). scrollbar-* are inherited, matching the playground. */
  scrollbar-width: thin;
  scrollbar-color: var(--border) transparent;

  --font-display: "Cormorant", Georgia, serif;
  --font-body: "Inter", system-ui, sans-serif;
  --font-mono: "JetBrains Mono", ui-monospace, monospace;

  --w: 1080px;
  --ease: cubic-bezier(.2, .7, .2, 1);

  background: var(--bg);
  color: var(--text);
  font-family: var(--font-body);
  font-size: 16px;
  line-height: 1.6;
  -webkit-font-smoothing: antialiased;
  min-height: 100vh;
}

/* Reset VitePress defaults inside custom home */
.custom-home * {
  box-sizing: border-box;
}

.custom-home a {
  color: var(--gold-bright);
  text-decoration: none;
}

.custom-home a:hover {
  text-decoration: underline;
  text-underline-offset: 3px;
}

.custom-home h1, .custom-home h2, .custom-home h3 {
  border: none;
  margin: 0;
}

.custom-home p {
  margin: 0;
}

.custom-home pre {
  margin: 0;
  padding: 0;
  background: none;
}

.custom-home code {
  background: none;
  border-radius: 0;
  padding: 0;
  font-size: inherit;
  color: inherit;
}

.custom-home ul {
  list-style: none;
  margin: 0;
  padding: 0;
}

/* ---------- nav ---------- */
nav {
  position: sticky;
  top: 0;
  z-index: 50;
  background: rgba(19, 17, 16, 0.86);
  backdrop-filter: blur(10px);
  border-bottom: 1px solid var(--border-lo);
}

.nav-in {
  display: flex;
  align-items: center;
  gap: 26px;
  height: 58px;
}

.logo-link {
  display: flex;
  align-items: center;
  text-decoration: none !important;
}

.logo-svg {
  height: 20px;
  transition: transform 0.2s ease;
  color: var(--text);
}

.logo-link:hover .logo-svg {
  transform: scale(1.04);
}

.nav-links {
  display: flex;
  align-items: center;
  gap: 22px;
  margin-left: auto;
  font-size: 13.5px;
}

.custom-home .nav-links a {
  color: var(--muted);
}

.custom-home .nav-links a:hover {
  color: var(--text);
  text-decoration: none;
}

.custom-home .nav-gh {
  display: flex;
  align-items: center;
  color: var(--muted) !important;
  transition: color 0.18s var(--ease);
}

.custom-home .nav-gh:hover {
  color: var(--gold-bright) !important;
  text-decoration: none;
}

.custom-home .nav-gh .gh-svg {
  width: 18px;
  height: 18px;
}

/* Hamburger toggle — hidden on desktop, shown on narrow screens. */
.nav-toggle {
  display: none;
  margin-left: auto;
  flex-direction: column;
  justify-content: center;
  gap: 4px;
  width: 36px;
  height: 34px;
  padding: 0;
  background: none;
  border: 1px solid var(--border);
  border-radius: 6px;
  cursor: pointer;
}

.nav-toggle span {
  display: block;
  width: 17px;
  height: 1.5px;
  margin: 0 auto;
  background: var(--text);
  transition: transform .2s var(--ease), opacity .2s var(--ease);
}

.nav-toggle.open span:nth-child(1) { transform: translateY(5.5px) rotate(45deg); }
.nav-toggle.open span:nth-child(2) { opacity: 0; }
.nav-toggle.open span:nth-child(3) { transform: translateY(-5.5px) rotate(-45deg); }

.wrap {
  max-width: var(--w);
  margin: 0 auto;
  padding: 0 28px;
}

.custom-home :focus-visible {
  outline: 2px solid var(--gold);
  outline-offset: 3px;
  border-radius: 2px;
}

/* ---------- hero ---------- */
.hero {
  position: relative;
  padding: 104px 0 84px;
  overflow: hidden;
}

.hero-paren {
  position: absolute;
  top: -60px;
  font-family: var(--font-display);
  font-weight: 300;
  font-style: italic;
  font-size: 560px;
  line-height: 1;
  color: var(--gold);
  opacity: .05;
  user-select: none;
  pointer-events: none;
}

.hero-paren.l {
  left: -70px;
}

.hero-paren.r {
  right: -70px;
  top: auto;
  bottom: -180px;
}

.custom-home .eyebrow {
  font-family: var(--font-mono);
  font-size: 12px;
  letter-spacing: .14em;
  text-transform: uppercase;
  color: var(--gold);
  margin-bottom: 26px;
}

.custom-home .eyebrow .sep {
  color: var(--dim);
  margin: 0 8px;
}

.custom-home h1 {
  font-family: var(--font-display);
  font-weight: 400;
  font-size: clamp(42px, 6.4vw, 76px);
  line-height: 1.04;
  letter-spacing: 0;
  max-width: 13ch;
  margin-bottom: 28px;
}

.custom-home h1 em {
  font-style: italic;
  color: var(--gold-bright);
}

.custom-home .lede {
  font-size: 18.5px;
  line-height: 1.65;
  color: var(--muted);
  max-width: 58ch;
  margin-bottom: 40px;
}

.custom-home .lede strong {
  color: var(--text);
  font-weight: 500;
}

.hero-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 14px;
  margin-bottom: 22px;
}

.custom-home .btn {
  display: inline-block;
  font-size: 14.5px;
  font-weight: 500;
  padding: 11px 22px;
  border-radius: 8px;
  transition: all .18s var(--ease);
}

.custom-home .btn-gold {
  background: var(--gold);
  color: #171410;
}

.custom-home .btn-gold:hover {
  background: var(--gold-bright);
  text-decoration: none;
}

.custom-home .btn-ghost {
  color: var(--text);
  border: 1px solid var(--border);
}

.custom-home .btn-ghost:hover {
  border-color: var(--gold-line);
  text-decoration: none;
}

.install {
  display: inline-flex;
  align-items: center;
  gap: 14px;
  font-family: var(--font-mono);
  font-size: 13.5px;
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 11px 16px;
  color: var(--text);
  max-width: 100%; /* never exceed the viewport — command text scrolls within */
}

.install .dollar {
  color: var(--gold);
  user-select: none;
}

.install .cm {
  color: var(--dim);
}

.copy {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--dim);
  background: none;
  border: 1px solid var(--border);
  border-radius: 5px;
  padding: 3px 9px;
  cursor: pointer;
  transition: all .15s;
  flex-shrink: 0; /* stay visible while the command text scrolls */
}

.copy:hover {
  color: var(--gold-bright);
  border-color: var(--gold-line);
}

.custom-home .req {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--dim);
}

/* ---------- section scaffolding ---------- */
section {
  padding: 88px 0;
  border-top: 1px solid var(--border-lo);
}

.custom-home .kicker {
  font-family: var(--font-mono);
  font-size: 12px;
  letter-spacing: .14em;
  text-transform: uppercase;
  color: var(--gold);
  margin-bottom: 14px;
}

.custom-home h2 {
  font-family: var(--font-display);
  font-weight: 400;
  font-size: clamp(30px, 3.6vw, 42px);
  line-height: 1.12;
  letter-spacing: 0;
  margin-bottom: 16px;
  max-width: 24ch;
}

.custom-home .sub {
  color: var(--muted);
  max-width: 62ch;
  font-size: 16.5px;
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

/* token colors */
.c-kw {
  color: var(--gold-bright);
}

.c-str {
  color: #a8b88a;
}

.c-kwd {
  color: #b8a3d6;
}

.c-com {
  color: #665e50;
  font-style: italic;
}

.c-fn {
  color: #d6c8a8;
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

.term {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  font-family: var(--font-mono);
  font-size: 12.5px;
  line-height: 1.85;
  padding: 20px 22px;
  color: #c9c2b4;
  overflow-x: auto;
}

.term .head {
  color: var(--dim);
  padding-bottom: 10px;
  margin-bottom: 12px;
  border-bottom: 1px solid var(--border-lo);
  font-size: 11.5px;
}

.term .dollar {
  color: var(--gold);
}

.term .dot {
  color: var(--gold);
}

.term .out {
  color: var(--dim);
}

.term .err {
  color: #c97b6a;
}

.term .ok {
  color: #9bb87a;
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

.custom-home .obj h3 {
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

/* ---------- cta / footer ---------- */
.cta {
  text-align: center;
  padding: 110px 0;
}

.custom-home .cta h2 {
  margin: 0 auto 18px;
}

.custom-home .cta .sub {
  margin: 0 auto 36px;
}

.install-stack {
  max-width: 660px;
  margin: 0 auto;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.install-row {
  display: flex;
  align-items: center;
  gap: 16px;
}

.install-row .badge {
  width: 54px;
  text-align: right;
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--gold-bright);
  font-weight: 500;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  flex-shrink: 0;
}

.install-row .badge.agent {
  color: var(--muted);
}

.install-row .install {
  flex-grow: 1;
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.custom-home .cmd-text {
  display: flex;
  align-items: center;
  gap: 14px;
  text-align: left;
  white-space: nowrap;
  /* Long install commands scroll horizontally inside the box instead of
     widening the page (min-width:0 lets this flex child actually shrink). */
  overflow-x: auto;
  min-width: 0;
  flex: 1 1 auto;
  scrollbar-width: thin;
}

/* WebKit/Safari scrollbar for the install snippets (scrollbar-color above
   covers Firefox + Chromium). */
.custom-home .cmd-text::-webkit-scrollbar {
  height: 8px;
}
.custom-home .cmd-text::-webkit-scrollbar-track {
  background: transparent;
}
.custom-home .cmd-text::-webkit-scrollbar-thumb {
  background: var(--border);
  border-radius: 8px;
}
.custom-home .cmd-text::-webkit-scrollbar-thumb:hover {
  background: var(--gold-line);
}

footer {
  border-top: 1px solid var(--border-lo);
  padding: 34px 0;
}

.foot-in {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 14px;
  font-size: 13px;
  color: var(--dim);
}

.custom-home .foot-in a {
  color: var(--muted);
}

@media (max-width: 880px) {
  /* Navbar collapses to a hamburger dropdown (was overflowing horizontally). */
  .nav-toggle {
    display: flex;
  }

  .nav-links {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    margin-left: 0;
    flex-direction: column;
    align-items: stretch;
    gap: 0;
    padding: 6px 0;
    background: rgba(19, 17, 16, 0.97);
    backdrop-filter: blur(10px);
    border-bottom: 1px solid var(--border-lo);
    display: none;
  }

  .nav-links.open {
    display: flex;
  }

  .custom-home .nav-links a {
    padding: 11px 22px;
  }

  .custom-home .nav-gh {
    padding: 11px 22px;
  }

  .compare, .objections, .ship-grid, .agent-grid {
    grid-template-columns: 1fr;
  }

  /* Stack the install label above its snippet so the command gets full width. */
  .install-row {
    flex-direction: column;
    align-items: stretch;
    gap: 7px;
  }

  .install-row .badge {
    width: auto;
    text-align: left;
  }

  .forms {
    grid-template-columns: 1fr;
  }

  .honest-list {
    columns: 1;
  }

  .hero {
    padding: 72px 0 60px;
  }

  section {
    padding: 64px 0;
  }

  .hero-paren {
    display: none;
  }
}

@media (prefers-reduced-motion: reduce) {
  html {
    scroll-behavior: auto;
  }
}
</style>
