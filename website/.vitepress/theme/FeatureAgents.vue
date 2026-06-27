<script setup>
import CustomPageLayout from './CustomPageLayout.vue'
</script>

<template>
  <CustomPageLayout active-nav="agents" v-slot="{ copyText }">

    <!-- ============ HERO ============ -->
    <header class="hero">
      <span class="hero-paren l" aria-hidden="true">(</span>
      <span class="hero-paren r" aria-hidden="true">)</span>
      <div class="wrap">
        <p class="eyebrow">Feature<span class="sep">·</span>Tools &amp; Agents<span class="sep">·</span>LLM Primitives</p>
        <h1>The loop is <em>the language.</em></h1>
        <p class="lede">
          Tools are lambdas with schemas. Agents are system prompts with a turn
          limit. The tool-dispatch loop, retries, cost caps, and provider
          fallback live in <strong>the runtime</strong> — your code stays the
          size of its idea.
        </p>
        <div class="hero-actions">
          <a class="btn btn-gold" href="/docs/llm/tools-agents">Read the docs</a>
          <a class="btn btn-ghost" href="https://sema.run">Try the playground</a>
        </div>
        <div class="hero-actions">
          <span class="install">
            <span class="cmd-text">
              <span class="dollar">$</span>
              <span id="i1">cargo install sema-lang</span>
            </span>
            <button class="copy" @click="copyText('i1', $event)">copy</button>
          </span>
        </div>
        <p class="req">Eight chat providers · retries with backoff · scoped budgets · single binary</p>
      </div>
    </header>

    <!-- ============ AGENT LOOP DIAGRAM ============ -->
    <section class="loop-showcase">
      <div class="wrap">
        <p class="kicker">The agent loop, visualized</p>
        <h2>What the runtime handles for you.</h2>
        <p class="sub">
          Every agent script grows the same loop. In Sema it's not code you
          write — it's what <code>agent/run</code> does when you call it.
        </p>

        <div class="filmstrip">
          <div class="film-sprockets film-sprockets-top">
            <span></span><span></span><span></span><span></span><span></span><span></span><span></span><span></span><span></span><span></span><span></span><span></span>
          </div>

          <div class="film-frames">
            <div class="film-frame">
              <div class="film-frame-num">1</div>
              <div class="film-frame-label">User message</div>
              <div class="film-frame-desc">"Find TODOs in src/"</div>
            </div>
            <div class="film-frame film-frame-hot">
              <div class="film-frame-num">2</div>
              <div class="film-frame-label">LLM call</div>
              <div class="film-frame-desc">Model decides: call a tool or answer</div>
            </div>
            <div class="film-frame film-frame-branch">
              <div class="film-frame-num">3</div>
              <div class="film-frame-label">Tool call?</div>
              <div class="film-frame-desc">Dispatch &amp; feed result back</div>
            </div>
            <div class="film-frame film-frame-done">
              <div class="film-frame-num">4</div>
              <div class="film-frame-label">Final answer</div>
              <div class="film-frame-desc">String returned to caller</div>
            </div>
          </div>

          <div class="film-sprockets film-sprockets-bottom">
            <span></span><span></span><span></span><span></span><span></span><span></span><span></span><span></span><span></span><span></span><span></span><span></span>
          </div>
        </div>

        <!-- Loop-back arrow: from frame 3 back to frame 2 -->
        <div class="film-loopback">
          <svg viewBox="0 0 200 40" fill="none" preserveAspectRatio="none" aria-hidden="true">
            <path d="M150 0 Q150 30 100 30 Q50 30 50 0" stroke="#c8a855" stroke-width="1.2" stroke-dasharray="3 3" fill="none" stroke-linecap="round"/>
            <path d="M48 4 L50 0 L53 4" stroke="#c8a855" stroke-width="1.2" fill="none" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
          <span class="film-loopback-label">loop &mdash; tool result fed back to LLM</span>
        </div>

        <div class="loop-guards">
          <div class="guard">
            <span class="guard-icon">&#x21bb;</span>
            <span><strong>Retry with backoff</strong> — 429s and 5xx retried automatically, up to 3 retries with exponential backoff and full jitter</span>
          </div>
          <div class="guard">
            <span class="guard-icon">$</span>
            <span><strong>Budget enforcement</strong> — <code>llm/with-budget</code> caps spend for the scope; calls that would exceed it fail</span>
          </div>
          <div class="guard">
            <span class="guard-icon">&#x21c4;</span>
            <span><strong>Provider fallback</strong> — <code>llm/with-fallback</code> tries the next provider if one fails</span>
          </div>
          <div class="guard">
            <span class="guard-icon">&#x26a0;</span>
            <span><strong>Error recovery</strong> — a tool that throws doesn't abort; the error is fed back so the model can self-correct</span>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ FEATURE: DEFTOOL ============ -->
    <section id="deftool">
      <div class="wrap">
        <div class="feature-row">
          <div class="feature-text">
            <p class="kicker">deftool</p>
            <h2>A tool is a lambda with a schema.</h2>
            <p class="sub">
              Define a function the LLM can call. The name, description, and
              parameter schema are sent to the model automatically. No JSON
              schema objects to build by hand, no dispatch table to maintain.
            </p>
            <ul class="feature-list">
              <li><strong>Typed parameters.</strong> Declare types inline with <code>:string</code>, <code>:number</code>, <code>:boolean</code> — the runtime builds the JSON schema for you.</li>
              <li><strong>Self-documenting.</strong> The description string is what the model sees. Write it like a doc comment.</li>
              <li><strong>Inspection.</strong> <code>tool/name</code>, <code>tool/description</code>, <code>tool/parameters</code>, <code>tool?</code> — tools are first-class values.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="code-card">
              <div class="code-card-head">
                <span class="t">tools.sema</span>
                <span class="n">define a tool</span>
              </div>
              <pre>(<span class="c-kw">deftool</span> <span class="c-fn">read-file</span>
  <span class="c-str">"Read a file's contents"</span>
  {<span class="c-kwd">:path</span> {<span class="c-kwd">:type</span> <span class="c-kwd">:string</span>
          <span class="c-kwd">:description</span> <span class="c-str">"File path"</span>}}
  (<span class="c-kw">lambda</span> (path)
    (file/read path)))

(<span class="c-kw">deftool</span> <span class="c-fn">run-command</span>
  <span class="c-str">"Run a shell command"</span>
  {<span class="c-kwd">:command</span> {<span class="c-kwd">:type</span> <span class="c-kwd">:string</span>}}
  (<span class="c-kw">lambda</span> (command)
    (<span class="c-kwd">:stdout</span>
      (shell <span class="c-str">"sh"</span> <span class="c-str">"-c"</span> command))))</pre>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ FEATURE: DEFAGENT ============ -->
    <section id="defagent">
      <div class="wrap">
        <div class="feature-row reverse">
          <div class="feature-text">
            <p class="kicker">defagent</p>
            <h2>An agent is a config, not a class.</h2>
            <p class="sub">
              System prompt, tools, model, and a turn limit (optional, defaults to 10). <code>agent/run</code>
              handles the loop — calling tools, feeding results back, stopping at
              the limit or when the model has a final answer.
            </p>
            <ul class="feature-list">
              <li><strong>One call to run.</strong> <code>(agent/run coder "Find TODOs")</code> returns the final answer string.</li>
              <li><strong>Observe tool calls.</strong> Pass <code>:on-tool-call</code> to watch each tool start and end — for logging, UIs, or debugging.</li>
              <li><strong>Full history.</strong> Pass an options map and get <code>:response</code> + <code>:messages</code> — the complete conversation for chaining or inspection.</li>
            </ul>
            <p class="sub" style="margin-top:18px">
              <a href="/docs/llm/tools-agents#agents">Agent reference &rarr;</a>
            </p>
          </div>
          <div class="feature-visual">
            <div class="code-card">
              <div class="code-card-head">
                <span class="t">agent.sema</span>
                <span class="n">define &amp; run</span>
              </div>
              <pre>(<span class="c-kw">defagent</span> <span class="c-fn">coder</span>
  {<span class="c-kwd">:system</span>    <span class="c-str">"You are a coding assistant."</span>
   <span class="c-kwd">:tools</span>     [read-file run-command]
   <span class="c-kwd">:model</span>     <span class="c-str">"claude-sonnet-4-6"</span>
   <span class="c-kwd">:max-turns</span> 10})

(<span class="c-kw">llm/with-budget</span>
  {<span class="c-kwd">:max-cost-usd</span> 0.50}
  (<span class="c-kw">lambda</span> ()
    (agent/run coder
      <span class="c-str">"Find TODOs in src/"</span>)))

<span class="c-com">;; =&gt; "Found 3 TODOs: src/main.rs:42, ..."</span>
<span class="c-com">;;    cost: $0.03 · 4 tool calls · 2 turns</span></pre>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ FEATURE: RUNTIME SCOPES ============ -->
    <section id="runtime-scopes">
      <div class="wrap">
        <p class="kicker">The runtime, in one screen</p>
        <h2>Everything you'd otherwise hand-roll.</h2>
        <p class="sub">These are forms — scoped expressions that wrap your code. They can't be forgotten on a late-night code path.</p>

        <div class="forms">
          <div class="form-row"><code>(llm/with-budget {:max-cost-usd 1.00} f)</code><span>hard spend cap, scoped to a block</span></div>
          <div class="form-row"><code>(llm/with-cache {:ttl 3600} f)</code><span>response cache — dev loops stop costing money</span></div>
          <div class="form-row"><code>(llm/with-fallback [:anthropic :openai] f)</code><span>provider failover, in order</span></div>
          <div class="form-row"><code>(llm/with-rate-limit 5 f)</code><span>token-bucket rate limiting, requests/sec</span></div>
          <div class="form-row"><code>(llm/extract {:amount {:type :number}} text)</code><span>typed maps back, not strings to re-parse</span></div>
          <div class="form-row"><code>(llm/with-cassette "tape.jsonl" f)</code><span>record &amp; replay — deterministic tests</span></div>
        </div>
        <p class="sub" style="margin-top:22px">
          Eight chat providers plus embedding providers, configured from environment variables — set the key and go.
          <a href="/docs/llm/">Browse the LLM reference &rarr;</a>
        </p>
      </div>
    </section>

    <!-- ============ FEATURE: CONVERSATIONS AS DATA ============ -->
    <section id="conversations">
      <div class="wrap">
        <div class="feature-row">
          <div class="feature-text">
            <p class="kicker">Conversations as data</p>
            <h2>Fork it. Diff it. Replay it.</h2>
            <p class="sub">
              A conversation is an immutable value. Every operation —
              <code>say</code>, <code>set-system</code>, <code>filter</code> —
              returns a new conversation. The original is never modified. Branch
              from any point and explore two directions simultaneously.
            </p>
            <ul class="feature-list">
              <li><strong>Immutable history.</strong> <code>conversation/say</code> returns a new value — the old one is untouched.</li>
              <li><strong>Branch with fork.</strong> <code>conversation/fork</code> creates an independent copy at any point. Explore alternatives without losing the thread.</li>
              <li><strong>Inspect everything.</strong> <code>conversation/messages</code>, <code>conversation/last-reply</code>, <code>conversation/token-count</code>, <code>conversation/cost</code> — all first-class.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="code-card">
              <div class="code-card-head">
                <span class="t">conversation.sema</span>
                <span class="n">fork &amp; explore</span>
              </div>
              <pre>(<span class="c-kw">define</span> conv
  (conversation/new
    {<span class="c-kwd">:model</span> <span class="c-str">"claude-sonnet-4-6"</span>}))

(<span class="c-kw">define</span> conv
  (conversation/say conv
    <span class="c-str">"Remember: the secret is 7"</span>))

<span class="c-com">;; Fork and explore two paths</span>
(<span class="c-kw">define</span> path-a
  (conversation/say
    (conversation/fork conv)
    <span class="c-str">"What about Python?"</span>))

(<span class="c-kw">define</span> path-b
  (conversation/say
    (conversation/fork conv)
    <span class="c-str">"What about Rust?"</span>))

<span class="c-com">;; conv, path-a, path-b — all independent</span></pre>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ FEATURE: ERROR RECOVERY ============ -->
    <section id="error-recovery">
      <div class="wrap">
        <div class="feature-row reverse">
          <div class="feature-text">
            <p class="kicker">Error recovery</p>
            <h2>Tools that throw don't abort.</h2>
            <p class="sub">
              A tool that raises an error, isn't found, or gets bad arguments
              doesn't crash the agent. The error is fed back to the model as the
              tool result — so it can correct itself and continue. The loop is
              bounded by <code>:max-turns</code> and aborts after 5 consecutive
              tool errors.
            </p>
            <ul class="feature-list">
              <li><strong>Self-correcting loop.</strong> The model sees the error message and adjusts its next call — wrong file path, bad command, missing argument.</li>
              <li><strong>Bounded by design.</strong> <code>:max-turns</code> caps the total loop; 5 consecutive tool errors abort. No infinite loops.</li>
              <li><strong>Observe everything.</strong> <code>:on-tool-call</code> fires on each tool start and end, with duration and result preview.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="term">
              <div class="head">agent run — tool error &amp; recovery</div>
              <div><span class="dollar">$</span> sema run agent.sema</div>
              <div><span class="dot">●</span> turn 1: model calls <span class="c-fn">read-file</span></div>
              <div class="err">&nbsp;&nbsp;✗ tool error: "No such file: src/mai.rs"</div>
              <div><span class="dot">●</span> turn 2: model retries with corrected path</div>
              <div class="out">&nbsp;&nbsp;read-file "src/main.rs" → 2.1 KB</div>
              <div><span class="dot">●</span> turn 3: model calls <span class="c-fn">run-command</span></div>
              <div class="out">&nbsp;&nbsp;run-command "grep -n TODO src/main.rs"</div>
              <div class="out">&nbsp;&nbsp;→ "42: // TODO: handle empty input"</div>
              <div><span class="dot">●</span> turn 4: final answer</div>
              <div class="ok">&nbsp;&nbsp;✓ "Found 1 TODO in src/main.rs:42"</div>
              <div class="out">&nbsp;&nbsp;4 turns · 2 tool calls · $0.03</div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ COMPARISON ============ -->
    <section id="compare">
      <div class="wrap">
        <p class="kicker">The argument</p>
        <h2>The same agent, twice.</h2>
        <p class="sub">A coding agent with a tool loop, retries, and a spend limit. Once with an SDK, once in Sema.</p>

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
    <span class="c-str">"description"</span>: <span class="c-str">"Read a file"</span>,
    <span class="c-str">"input_schema"</span>: {
        <span class="c-str">"type"</span>: <span class="c-str">"object"</span>,
        <span class="c-str">"properties"</span>: {<span class="c-str">"path"</span>: {<span class="c-str">"type"</span>: <span class="c-str">"string"</span>}},
        <span class="c-str">"required"</span>: [<span class="c-str">"path"</span>],
    },
}, {
    <span class="c-str">"name"</span>: <span class="c-str">"run_command"</span>,
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
    <span class="c-kw">if</span> resp.stop_reason != <span class="c-str">"tool_use"</span>: <span class="c-kw">break</span>
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
   <span class="c-kwd">:max-turns</span> 10})

(llm/with-budget {<span class="c-kwd">:max-cost-usd</span> 0.50}
  (<span class="c-kw">lambda</span> ()
    (agent/run coder <span class="c-str">"Find TODOs in src/"</span>)))</pre>
            <div class="pane-foot">
              The tool loop, retries with backoff, and cost tracking
              live in the runtime. The spend cap is a <em>scope</em> — it can't be
              forgotten on the late-night code path.
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ CTA ============ -->
    <section class="cta">
      <div class="wrap">
        <h2>Build an agent in ten lines.</h2>
        <p class="sub">Install Sema, define a tool, run an agent. The loop is already there.</p>
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
          <div class="hero-actions" style="justify-content:center; margin-top:24px">
            <a class="btn btn-gold" href="/docs/llm/tools-agents">Tools &amp; Agents docs</a>
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

/* ---------- filmstrip loop diagram ---------- */
.loop-showcase { padding: 0 0 88px; border-top: none; }

.filmstrip {
  margin-top: 48px;
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
}

.film-sprockets {
  display: flex;
  justify-content: space-between;
  padding: 6px 12px;
  background: var(--bg);
}

.film-sprockets-top { border-bottom: 1px solid var(--border-lo); }
.film-sprockets-bottom { border-top: 1px solid var(--border-lo); }

.film-sprockets span {
  display: block;
  width: 10px;
  height: 5px;
  background: var(--border);
  border-radius: 1px;
  flex-shrink: 0;
}

.film-frames {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 1px;
  background: var(--border-lo);
}

.film-frame {
  background: var(--bg-raise);
  padding: 18px 14px 20px;
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
  gap: 6px;
  position: relative;
  transition: background .15s;
}

.film-frame:hover {
  background: var(--surface);
}

.film-frame-num {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--gold);
  font-weight: 500;
  width: 24px;
  height: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--border);
  border-radius: 50%;
  background: var(--bg);
}

.film-frame-hot .film-frame-num {
  border-color: var(--gold-line);
  color: var(--gold-bright);
  background: var(--gold-fade);
}

.film-frame-branch .film-frame-num {
  border-style: dashed;
}

.film-frame-done .film-frame-num {
  border-color: rgba(155, 184, 122, 0.3);
  color: #9bb87a;
}

.film-frame-label {
  font-family: var(--font-body);
  font-size: 13.5px;
  font-weight: 500;
  color: var(--text);
}

.film-frame-hot .film-frame-label { color: var(--gold-bright); }

.film-frame-desc {
  font-family: var(--font-mono);
  font-size: 10.5px;
  color: var(--muted);
  line-height: 1.45;
  max-width: 130px;
}

/* loop-back arrow */
.film-loopback {
  position: relative;
  padding: 0 18%;
  margin-top: 4px;
  height: 36px;
}

.film-loopback svg {
  width: 100%;
  height: 36px;
  display: block;
}

.film-loopback-label {
  position: absolute;
  bottom: 0;
  left: 50%;
  transform: translateX(-50%);
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--dim);
  background: var(--bg);
  padding: 0 8px;
  white-space: nowrap;
}

.loop-guards {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 16px;
  margin-top: 48px;
}

.guard {
  display: flex;
  align-items: flex-start;
  gap: 14px;
  padding: 18px 20px;
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 10px;
}

.guard-icon {
  font-size: 20px;
  color: var(--gold);
  flex-shrink: 0;
  line-height: 1.2;
}

.guard span:last-child {
  font-size: 13.5px;
  color: var(--muted);
  line-height: 1.6;
}

.guard strong {
  color: var(--text);
  font-weight: 500;
}

.guard code {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
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

/* ---------- comparison panes ---------- */
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

.pane-head .t { color: var(--text); }
.pane.sema .pane-head .t { color: var(--gold-bright); }
.pane-head .n { color: var(--dim); }

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
  left: 0; right: 0; bottom: 0;
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

.pane.sema .pane-foot { color: var(--text); }

/* ---------- responsive ---------- */
@media (max-width: 880px) {
  .hero { padding: 72px 0 48px; }

  .feature-row, .feature-row.reverse {
    grid-template-columns: 1fr;
  }
  .feature-row.reverse .feature-text { order: unset; }
  .feature-row.reverse .feature-visual { order: unset; }

  .film-frames {
    grid-template-columns: 1fr 1fr;
  }

  .film-loopback { display: none; }

  .loop-guards { grid-template-columns: 1fr; }

  .forms { grid-template-columns: 1fr; }

  .compare { grid-template-columns: 1fr; }
}
</style>
