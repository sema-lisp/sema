<script setup>
import CustomPageLayout from './CustomPageLayout.vue'
</script>

<template>
  <CustomPageLayout active-nav="workflows" v-slot="{ copyText }">

    <!-- ============ HERO ============ -->
    <header class="hero">
      <span class="hero-paren l" aria-hidden="true">(</span>
      <span class="hero-paren r" aria-hidden="true">)</span>
      <div class="wrap">
        <p class="eyebrow">Feature<span class="sep">·</span>Workflows<span class="sep">·</span>Journaled<span class="sep">·</span>Resumable</p>
        <h1>Agent orchestration, <em>built in.</em></h1>
        <p class="lede">
          Define multi-phase agentic workflows as ordinary Sema code.
          <strong>Phases</strong> are markers, <strong>agents</strong> are LLM
          leaves, and every step is <strong>journalled</strong> to a frozen
          JSONL run directory — resume, replay, or fork without losing state.
        </p>
        <div class="hero-actions">
          <a class="btn btn-gold" href="/docs/llm/workflows">Read the docs</a>
          <a class="btn btn-ghost" href="https://sema.run">Try the playground</a>
        </div>
        <div class="hero-actions">
          <span class="install">
            <span class="cmd-text">
              <span class="dollar">$</span>
              <span id="wf-install">cargo install sema-lang</span>
            </span>
            <button class="copy" @click="copyText('wf-install', $event)">copy</button>
          </span>
        </div>
        <p class="req">Frozen JSONL journal · content-keyed resume · budget caps · parallel &amp; pipeline fan-out</p>
      </div>
    </header>

    <!-- ============ WHAT IS A WORKFLOW ============ -->
    <section id="what">
      <div class="wrap">
        <p class="kicker">A workflow is just a function</p>
        <h2>Define, run, journal — no daemon, no YAML.</h2>
        <p class="sub">
          <code>defworkflow</code> is a prelude macro that expands to
          <code>workflow/run</code>. The body is ordinary Sema: phases are
          markers, agents are LLM calls, and the runtime journals everything
          to a run directory under <code>.sema/runs/</code>.
        </p>

        <div class="feature-row" style="margin-top: 46px">
          <div class="feature-visual">
            <div class="code-card">
              <div class="code-card-head">
                <span class="t">content-pipeline.sema</span>
                <span class="n">a real workflow</span>
              </div>
              <pre><span class="c-com">;; (helpers: article schema, slug, prompt, good-article? — omitted)</span>
(<span class="c-kw">defworkflow</span> <span class="c-fn">content-pipeline</span>
  <span class="c-str">"Generate + verify explainer articles."</span>
  {<span class="c-kwd">:phases</span> [<span class="c-str">"Topics"</span> <span class="c-str">"Write"</span> <span class="c-str">"Verify"</span> <span class="c-str">"Publish"</span>]
   <span class="c-kwd">:budget</span> {<span class="c-kwd">:tokens</span> 50000 <span class="c-kwd">:usd</span> 1.0}}

  (<span class="c-kw">phase</span> <span class="c-str">"Topics"</span>)
  (<span class="c-kw">def</span> chosen topics)

  (<span class="c-kw">phase</span> <span class="c-str">"Write"</span>)
  <span class="c-com">;; Fan out: one typed step per topic,</span>
  <span class="c-com">;; at most 4 model calls in flight.</span>
  (<span class="c-kw">def</span> articles
    (<span class="c-kw">pipeline</span> chosen
      (<span class="c-kw">fn</span> (t)
        (<span class="c-kw">step</span> (article-prompt t)
               {<span class="c-kwd">:name</span> <span class="c-str">"writer"</span>
                <span class="c-kwd">:schema</span> article}))))

  (<span class="c-kw">phase</span> <span class="c-str">"Verify"</span>)
  (<span class="c-kw">def</span> verified
    (<span class="c-kw">filter</span> good-article? articles))

  (<span class="c-kw">phase</span> <span class="c-str">"Publish"</span>)
  (<span class="c-kw">for-each</span> write-file verified)

  {<span class="c-kwd">:status</span> <span class="c-kwd">:success</span>
   <span class="c-kwd">:published</span> (<span class="c-kw">count</span> verified)})</pre>
            </div>
          </div>
          <div class="feature-text">
            <ul class="feature-list">
              <li><strong>defworkflow</strong> — a macro that expands to <code>workflow/run</code>. The body is a thunk; the form <em>is</em> the run.</li>
              <li><strong>phase</strong> — a marker, not a wrapper. <code>(phase "Write")</code> opens a phase; forms after it belong to that phase until the next marker or run end.</li>
              <li><strong>step</strong> — a journaled LLM leaf. With <code>:schema</code> it returns typed data (validated via <code>llm/extract</code>); without, it returns the completion text. With <code>:tools</code> it runs the real tool loop. With <code>:agent</code> it runs a configured <code>defagent</code>.</li>
              <li><strong>checkpoint</strong> — <code>(checkpoint :k v)</code> records and returns <code>v</code>; <code>(checkpoint :k)</code> reads it back. Threads state between phases.</li>
              <li><strong>parallel / pipeline</strong> — bounded-concurrency fan-out. <code>parallel</code> runs thunks concurrently (barrier); <code>pipeline</code> flows each item through stages independently (no barrier between stages).</li>
              <li><strong>{:status …}</strong> — the return envelope. <code>:success</code> or <code>:failed</code>; the runtime forces <code>:failed</code> if a budget cap trips.</li>
            </ul>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ RUN IT ============ -->
    <section id="run">
      <div class="wrap">
        <div class="feature-row reverse">
          <div class="feature-visual">
            <div class="term">
              <div class="head">sema workflow run + view</div>
              <div><span class="dollar">$</span> sema workflow run content-pipeline.sema \
                --args '{"repo":"my-app"}' --view</div>
              <div class="out">Sema workflow viewer:  http://127.0.0.1:8899</div>
              <div class="out">  runs: .sema/runs</div>
              <div><span class="dot">●</span> run.started  wf_1719494_12345</div>
              <div><span class="dot">●</span> phase.started  "Topics"</div>
              <div><span class="dot">●</span> phase.started  "Write"</div>
              <div><span class="dot">●</span> agent.started  writer_1</div>
              <div><span class="dot">●</span> agent.started  writer_2</div>
              <div class="out">  writer_1 → {:title "TCO" :body "…"}</div>
              <div class="out">  writer_2 → {:title "NaN-boxing" :body "…"}</div>
              <div><span class="dot">●</span> budget  3120 in / 880 out / $0.004</div>
              <div><span class="dot">●</span> phase.ended  "Write"  2100ms</div>
              <div><span class="dot">●</span> phase.started  "Verify"</div>
              <div class="out">  4 articles → 3 passed</div>
              <div><span class="dot">●</span> phase.ended  "Verify"  12ms</div>
              <div><span class="dot">●</span> phase.started  "Publish"</div>
              <div class="out">  wrote out/tco.md, out/nan-boxing.md, …</div>
              <div class="ok">  ✓ run.ended  success  5400ms</div>
              <div class="out">  result: {:status :success :published 3}</div>
            </div>
          </div>
          <div class="feature-text">
            <p class="kicker">Run it</p>
            <h2>One command. A run directory. A live viewer.</h2>
            <p class="sub">
              <code>sema workflow run</code> evaluates the file, journals every
              event to <code>.sema/runs/&lt;run-id&gt;/</code>, and writes
              <code>result.json</code>. The <code>--view</code> flag starts a
              live web viewer so you can watch the run progress in real time.
            </p>
            <ul class="feature-list">
              <li><strong>events.jsonl</strong> — the system of record. Append-only, one JSON event per line. Frozen vocabulary.</li>
              <li><strong>memo/</strong> — per-leaf resume cache. A file's existence means that leaf completed with this value.</li>
              <li><strong>metadata.json</strong> — workflow name, code version, budget, args.</li>
              <li><strong>result.json</strong> — the final <code>{:status …}</code> envelope.</li>
              <li><strong>sema workflow view</strong> — a read-only web viewer that polls the journal and renders the live tree.</li>
            </ul>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ VIEWER MOCKUP ============ -->
    <section id="viewer">
      <div class="wrap">
        <p class="kicker">The dashboard</p>
        <h2>Watch the run unfold, live.</h2>
        <p class="sub">
          <code>sema workflow view</code> (or <code>--view</code> on
          <code>run</code>) serves a read-only web viewer that polls the
          journal and renders the run as a tree: phases nest agents, budget
          events show per-leaf spend, checkpoints show their digests.
        </p>

        <div class="viewer-mock" style="margin-top: 46px">
          <div class="viewer-bar">
            <span class="viewer-dot"></span>
            <span class="viewer-dot"></span>
            <span class="viewer-dot"></span>
            <span class="viewer-url">127.0.0.1:8899 — Sema Workflow Viewer</span>
            <span class="viewer-status">● running</span>
          </div>
          <div class="viewer-body">
            <div class="vt-row vt-run">
              <span class="vt-icon">▶</span>
              <span class="vt-label">content-pipeline</span>
              <span class="vt-meta">wf_1719494_12345 · 5400ms · <span class="vt-ok">success</span></span>
            </div>

            <div class="vt-row vt-phase">
              <span class="vt-icon">◇</span>
              <span class="vt-label">Topics</span>
              <span class="vt-meta">0ms · <span class="vt-ok">success</span></span>
            </div>

            <div class="vt-row vt-phase">
              <span class="vt-icon">◇</span>
              <span class="vt-label">Write</span>
              <span class="vt-meta">2100ms · <span class="vt-ok">success</span></span>
            </div>

            <div class="vt-row vt-agent">
              <span class="vt-icon">●</span>
              <span class="vt-label">writer_1 <span class="vt-role">writer</span></span>
              <span class="vt-meta">1240ms · gpt-5.4-mini</span>
            </div>
            <div class="vt-row vt-budget">
              <span class="vt-icon">$</span>
              <span class="vt-label">budget</span>
              <span class="vt-meta">3120 in · 880 out · $0.004 / $1.00</span>
            </div>

            <div class="vt-row vt-agent">
              <span class="vt-icon">●</span>
              <span class="vt-label">writer_2 <span class="vt-role">writer</span></span>
              <span class="vt-meta">980ms · gpt-5.4-mini</span>
            </div>
            <div class="vt-row vt-budget">
              <span class="vt-icon">$</span>
              <span class="vt-label">budget</span>
              <span class="vt-meta">2900 in · 620 out · $0.003 / $1.00</span>
            </div>

            <div class="vt-row vt-phase">
              <span class="vt-icon">◇</span>
              <span class="vt-label">Verify</span>
              <span class="vt-meta">12ms · <span class="vt-ok">success</span></span>
            </div>
            <div class="vt-row vt-checkpoint">
              <span class="vt-icon">◆</span>
              <span class="vt-label">checkpoint <span class="vt-role">:files</span></span>
              <span class="vt-meta">ck_4d2f8a1c</span>
            </div>

            <div class="vt-row vt-phase">
              <span class="vt-icon">◇</span>
              <span class="vt-label">Publish</span>
              <span class="vt-meta">45ms · <span class="vt-ok">success</span></span>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ THE JOURNAL ============ -->
    <section id="journal">
      <div class="wrap">
        <p class="kicker">The run directory is the system of record</p>
        <h2>Every event, journaled. Frozen. Append-only.</h2>
        <p class="sub">
          The event vocabulary is a frozen public contract — add fields
          (append-only, all <code>Option</code>/skippable) but never change
          existing ones. Old runs stay readable forever. The journal is
          flushed per event, so a crash mid-run leaves a valid prefix.
        </p>

        <div class="code-card" style="margin-top: 46px">
          <div class="code-card-head">
            <span class="t">events.jsonl</span>
            <span class="n">one line per event</span>
          </div>
          <pre>{"event":"run.started","seq":0,"ts":"2026-06-27T…","workflow":"content-pipeline","run_id":"wf_…"}
{"event":"phase.started","seq":1,"ts":"…","phase":"Topics"}
{"event":"phase.ended","seq":2,"ts":"…","phase":"Topics","status":"success","dur_ms":0}
{"event":"phase.started","seq":3,"ts":"…","phase":"Write"}
{"event":"agent.started","seq":4,"ts":"…","agent_id":"writer_1","agent_name":"writer"}
{"event":"agent.result","seq":5,"ts":"…","agent_id":"writer_1","status":"ok","output":"…","dur_ms":1240,"model":"gpt-5.4-mini"}
{"event":"budget","seq":6,"ts":"…","agent_id":"writer_1","input_tokens":3120,"output_tokens":880,"cost_usd":0.0041,"budget_limit":50000}
{"event":"checkpoint","seq":7,"ts":"…","key":"files","content_key":"ck_4d2f8a1c","value_digest":"abc123"}
{"event":"phase.ended","seq":8,"ts":"…","phase":"Write","status":"success","dur_ms":2100}
{"event":"run.ended","seq":9,"ts":"…","status":"success","dur_ms":5400}</pre>
        </div>

        <div class="forms" style="margin-top: 46px">
          <div class="form-row"><code>run.started / run.ended</code><span>workflow name, run id, status, duration</span></div>
          <div class="form-row"><code>phase.started / phase.ended</code><span>label, status, duration — nest agents under phases</span></div>
          <div class="form-row"><code>agent.started / agent.result</code><span>agent_id, role, model, output, duration</span></div>
          <div class="form-row"><code>agent.tool_call</code><span>tool name, argument digest — journaled tool invocations</span></div>
          <div class="form-row"><code>checkpoint</code><span>key, content-key (resume hash), value digest</span></div>
          <div class="form-row"><code>budget</code><span>per-leaf token + cost attribution; never double-charges</span></div>
        </div>
      </div>
    </section>

    <!-- ============ RESUME ============ -->
    <section id="resume">
      <div class="wrap">
        <div class="feature-row">
          <div class="feature-text">
            <p class="kicker">Resume</p>
            <h2>Crash, edit, re-run — <em>pick up where you left off.</em></h2>
            <p class="sub">
              <code>--resume &lt;run-id&gt;</code> reuses the run directory and
              short-circuits any leaf whose content-key is in the prior run's
              <code>memo/</code> dir. The model is not called for memoized
              leaves — they replay for free. A fresh
              <code>events.resume-N.jsonl</code> segment is written so the
              frozen invariants hold in each file.
            </p>
            <ul class="feature-list">
              <li><strong>Content-keyed.</strong> Each leaf's key is a hash of (kind, code-version, phase, prompt, schema). Same inputs → same key → memo hit → no re-call.</li>
              <li><strong>Automatic invalidation.</strong> Edit the workflow → different code version → different keys → no memo hits → full re-run. No guard files to maintain.</li>
              <li><strong>Per-leaf granularity.</strong> Delete one memo file → that leaf re-runs while others still replay. Conservative: a missing memo always re-runs.</li>
              <li><strong>Resume doesn't double-charge.</strong> Spend starts at zero on resume. Memoized leaves don't re-call the model and don't recharge the budget.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="term">
              <div class="head">resume after a crash</div>
              <div><span class="c-com">;; First run crashed after "Write" phase.</span></div>
              <div><span class="c-com">;; The journal + memo/ dir are intact.</span></div>
              <div><span class="dollar">$</span> ls .sema/runs/wf_1719494_12345/</div>
              <div class="out">  events.jsonl    memo/    metadata.json</div>
              <div><span class="dollar">$</span> ls .sema/runs/wf_1719494_12345/memo/</div>
              <div class="out">  ck_a1b2c3d4.json   ck_e5f6g7h8.json</div>
              <div><span class="dollar">$</span> sema workflow run content-pipeline.sema \</div>
              <div>    --resume wf_1719494_12345</div>
              <div class="out">  → writer_1: memoized (skipped)</div>
              <div class="out">  → writer_2: memoized (skipped)</div>
              <div><span class="dot">●</span> phase.started  "Verify"</div>
              <div class="out">  3 articles → 3 passed</div>
              <div class="ok">  ✓ run.ended  success  180ms</div>
              <div class="out">  model calls: 0 (all memoized)</div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ BUDGET ============ -->
    <section id="budget">
      <div class="wrap">
        <div class="feature-row reverse">
          <div class="feature-visual">
            <div class="code-card">
              <div class="code-card-head">
                <span class="t">budget enforcement</span>
                <span class="n">:budget in metadata</span>
              </div>
              <pre>(<span class="c-kw">defworkflow</span> <span class="c-fn">audit</span>
  <span class="c-str">"Audit with a token cap."</span>
  {<span class="c-kwd">:phases</span> [<span class="c-str">"Scan"</span> <span class="c-str">"Report"</span>]
   <span class="c-kwd">:budget</span> {<span class="c-kwd">:tokens</span> 5000}}

  (<span class="c-kw">phase</span> <span class="c-str">"Scan"</span>)
  (<span class="c-kw">def</span> a (step <span class="c-str">"Find files."</span> {}))
  <span class="c-com">;; a burns 5200 tokens → cap trips</span>

  (<span class="c-kw">phase</span> <span class="c-str">"Report"</span>)
  (<span class="c-kw">def</span> b (step <span class="c-str">"Summarize."</span> {}))
  <span class="c-com">;; b refused: over_budget latch</span>

  {<span class="c-kwd">:status</span> <span class="c-kwd">:success</span>
   <span class="c-kwd">:a</span> a <span class="c-kwd">:b</span> b})
<span class="c-com">;; → {:status :failed</span>
<span class="c-com">;;     :reason "budget exceeded"}</span></pre>
            </div>
          </div>
          <div class="feature-text">
            <p class="kicker">Budget enforcement</p>
            <h2>Spend caps that actually trip.</h2>
            <p class="sub">
              Declare <code>:budget {:tokens N :usd M}</code> in the workflow
              metadata. The runtime charges each step leaf and latches a
              sticky <code>over_budget</code> flag when a cap is exceeded —
              further step leaves are refused and the run ends
              <code>{:status :failed :reason "budget exceeded"}</code>.
            </p>
            <ul class="feature-list">
              <li><strong>Per-leaf attribution.</strong> Each <code>budget</code> event records the <code>agent_id</code>, token counts, and cost — so the dashboard shows per-leaf spend, not just a total.</li>
              <li><strong>Sticky latch.</strong> Once a cap trips, the latch stays set. No step leaf launches after it — even under concurrent <code>parallel</code> fan-out.</li>
              <li><strong>Resume doesn't double-charge.</strong> A <code>--resume</code> run starts spend at zero. Memoized leaves replay for free.</li>
              <li><strong>USD is best-effort.</strong> Token caps are deterministic; USD caps depend on the pricing table being available.</li>
            </ul>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ MACRO COOKBOOK ============ -->
    <section id="cookbook">
      <div class="wrap">
        <p class="kicker">The papers are just control flow</p>
        <h2>Classic agent architectures as composable macros.</h2>
        <p class="sub">
          The workflow DSL is homoiconic — agent patterns from the literature
          are macros that expand into <code>parallel</code>, <code>pipeline</code>,
          and <code>step</code> forms. No framework, no runtime tax. These are
          from <code>examples/workflows/cookbook.sema</code> — load and use them.
        </p>

        <div class="cookbook-grid">
          <div class="cook-card">
            <h3>ReAct</h3>
            <p class="cook-tag">think → act → observe → repeat</p>
            <p>The step reasons about each tool result before deciding the next step. The loop is a <code>let loop</code> over an accumulator — bounded by <code>max-rounds</code>.</p>
            <pre>(<span class="c-kw">defmacro</span> <span class="c-fn">react</span> (question tools max-rounds)
  `(let loop ((round 1) (scratch ""))
     (let ((answer (step (str "Q: " ,question "\n"
                              scratch)
                        {:name "react"
                         :tools ,tools})))
       (if (or (>= round ,max-rounds)
               (not (contains? (lower answer) "next:")))
         answer
         (loop (+ round 1)
               (str scratch "\n" answer))))))</pre>
          </div>

          <div class="cook-card">
            <h3>Reflexion</h3>
            <p class="cook-tag">try → self-critique → retry</p>
            <p>The step critiques its own output and retries with the feedback. A critic reply starting with "OK" short-circuits. Bounded by <code>max-tries</code>.</p>
            <pre>(<span class="c-kw">defmacro</span> <span class="c-fn">reflexion</span> (task max-tries)
  `(let loop ((try 1) (note ""))
     (let ((attempt (step ,task {:name "actor"})))
       (if (>= try ,max-tries) attempt
         (let ((critique (step
           (str "Critique. Reply OK if good.\n\n"
                attempt) {:name "critic"})))
           (if (starts-with? (trim critique) "OK")
             attempt
             (loop (+ try 1) critique)))))))</pre>
          </div>

          <div class="cook-card">
            <h3>Tree-of-Thought</h3>
            <p class="cook-tag">branch → score → keep best</p>
            <p>Fork N candidate solutions in parallel, score each, keep the best. <code>parallel</code> handles the fan-out; the workflow journals every branch.</p>
            <pre>(<span class="c-kw">defmacro</span> <span class="c-fn">tree-of-thought</span> (prompt n scorer)
  `(let ((cands (filter (fn (c) (not (nil? c)))
                  (parallel
                    (map (fn (i)
                           (fn () (step
                             (str ,prompt "\n(candidate #" i ")")
                             {:name "thought"})))
                         (range ,n))))))
     (foldl (fn (best c)
              (if (> (,scorer c) (,scorer best))
                c best))
            (first cands) (rest cands))))</pre>
          </div>

          <div class="cook-card">
            <h3>Debate</h3>
            <p class="cook-tag">N agents → judge</p>
            <p>Two personas argue R rounds; a judge reads the transcript and decides. Each round is two <code>step</code> leaves; the judge is a third.</p>
            <pre>(<span class="c-kw">defmacro</span> <span class="c-fn">debate</span> (topic a b rounds)
  `(let loop ((r 1) (transcript topic))
     (let* ((arg-a (step (str "You are " ,a ".\n"
                              transcript) {:name ,a}))
            (t1 (str transcript "\n\n" ,a ": " arg-a))
            (arg-b (step (str "You are " ,b ".\n"
                              t1) {:name ,b}))
            (t2 (str t1 "\n\n" ,b ": " arg-b)))
       (if (>= r ,rounds)
         (step (str "Judge the debate:\n" t2)
           {:name "judge"})
         (loop (+ r 1) t2)))))</pre>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ STATIC CHECK ============ -->
    <section id="check">
      <div class="wrap">
        <div class="feature-row">
          <div class="feature-text">
            <p class="kicker">Catch errors before a run</p>
            <h2><code style="font-family: var(--font-mono); font-size: 0.7em; color: var(--gold-bright); background: var(--gold-fade); padding: 2px 8px; border-radius: 5px; white-space: nowrap;">sema workflow check</code> — instant, no LLM.</h2>
            <p class="sub">
              Statically validate a workflow file without evaluating it or
              calling any model. Catches arity traps (e.g.
              <code>(phase "x" body)</code> — <code>phase</code> is a one-arg
              marker), bad step options, and layout issues before you spend a
              token.
            </p>
            <ul class="feature-list">
              <li><strong>Pure static analysis.</strong> Parses the AST and walks it — never evaluates, never configures a provider, never emits a journal event.</li>
              <li><strong>Human or JSON output.</strong> <code>--json</code> emits machine-readable diagnostics with source spans. <code>--strict</code> treats warnings as errors (CI gate).</li>
              <li><strong>Workflow-only checks.</strong> <code>phase</code>/<code>checkpoint</code>/<code>parallel</code>/<code>pipeline</code> arity is checked only inside a <code>defworkflow</code> body — a bare <code>(parallel …)</code> in a library file never trips.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="term">
              <div class="head">static validation</div>
              <div><span class="dollar">$</span> sema workflow check audit.sema</div>
              <div class="err">  error[WF-PHASE-ARITY]: phase expects exactly 1</div>
              <div class="err">    argument (a label), got 3</div>
              <div class="out">    at line 12, col 3</div>
              <div class="out">    hint: phase is a MARKER — use</div>
              <div class="out">      (phase "Audit") then body forms</div>
              <div><span class="dollar">$</span> sema workflow check audit.sema --json</div>
              <div class="out">  [{"severity":"error",</div>
              <div class="out">    "code":"WF-PHASE-ARITY",</div>
              <div class="out">    "line":12,...}]</div>
              <div><span class="dollar">$</span> echo $?</div>
              <div class="out">  1</div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ CTA ============ -->
    <section class="cta">
      <div class="wrap">
        <h2>Run your first workflow.</h2>
        <p class="sub">The workflow DSL is homoiconic — the plan, the program, and the trace are all s-expressions. No YAML, no daemon, no separate runtime.</p>
        <div class="install-stack">
          <div class="install-row">
            <span class="badge">curl</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="wf-curl">curl -fsSL https://sema-lang.com/install.sh | sh</span>
              </span>
              <button class="copy" @click="copyText('wf-curl', $event)">copy</button>
            </span>
          </div>
          <div class="install-row">
            <span class="badge">cargo</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="wf-cargo">cargo install sema-lang</span>
              </span>
              <button class="copy" @click="copyText('wf-cargo', $event)">copy</button>
            </span>
          </div>
          <div class="hero-actions" style="justify-content:center; margin-top:24px">
            <a class="btn btn-gold" href="/docs/llm/workflows">Workflow docs</a>
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

/* ---------- feature rows (reuse from agents page) ---------- */
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

/* ---------- runtime strip (reuse from agents page) ---------- */
.forms {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 1px;
  background: var(--border-lo);
  border: 1px solid var(--border-lo);
  border-radius: 12px;
  overflow: hidden;
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

/* ---------- cookbook grid ---------- */
.cookbook-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 22px;
  margin-top: 46px;
}

.cook-card {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.cook-card h3 {
  font-family: var(--font-display);
  font-size: 22px;
  font-weight: 400;
  color: var(--text);
  margin: 0;
}

.cook-tag {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--gold);
  margin: 0;
}

.cook-card > p:not(.cook-tag) {
  font-size: 13.5px;
  color: var(--muted);
  line-height: 1.55;
}

.cook-card pre {
  font-family: var(--font-mono);
  font-size: 11.5px;
  line-height: 1.6;
  color: #c9c2b4;
  background: var(--bg);
  border: 1px solid var(--border-lo);
  border-radius: 8px;
  padding: 14px 16px;
  overflow-x: auto;
  margin-top: 4px;
}

/* ---------- viewer mockup ---------- */
.viewer-mock {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
  box-shadow: 0 0 0 1px rgba(200, 168, 85, .04), 0 20px 50px -30px rgba(0, 0, 0, .3);
}

.viewer-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 11px 16px;
  background: var(--bg);
  border-bottom: 1px solid var(--border-lo);
  font-family: var(--font-mono);
  font-size: 12px;
}

.viewer-dot {
  width: 11px;
  height: 11px;
  border-radius: 50%;
  background: var(--border);
}

.viewer-url {
  color: var(--dim);
  margin-left: 8px;
}

.viewer-status {
  margin-left: auto;
  color: var(--gold-bright);
  font-size: 11px;
}

.viewer-body {
  padding: 16px 20px;
  font-family: var(--font-mono);
  font-size: 13px;
  line-height: 1.85;
}

.vt-row {
  display: flex;
  align-items: baseline;
  gap: 10px;
}

.vt-icon {
  width: 16px;
  text-align: center;
  flex-shrink: 0;
  color: var(--gold);
}

.vt-label {
  color: var(--text);
}

.vt-role {
  color: var(--dim);
  font-size: 11px;
}

.vt-meta {
  margin-left: auto;
  color: var(--muted);
  font-size: 12px;
}

.vt-ok { color: #9bb87a; }

.vt-run { padding-bottom: 8px; border-bottom: 1px solid var(--border-lo); margin-bottom: 8px; }
.vt-run .vt-label { font-weight: 500; }

.vt-phase { padding-left: 20px; }
.vt-phase .vt-icon { color: var(--gold-bright); }
.vt-phase .vt-label { color: var(--gold-bright); }

.vt-agent { padding-left: 40px; }
.vt-agent .vt-icon { color: var(--gold); font-size: 10px; }

.vt-budget { padding-left: 56px; }
.vt-budget .vt-icon { color: var(--dim); font-size: 11px; }
.vt-budget .vt-label { color: var(--dim); font-size: 12px; }

.vt-checkpoint { padding-left: 40px; }
.vt-checkpoint .vt-icon { color: #b8a3d6; font-size: 10px; }
.vt-checkpoint .vt-label { color: var(--muted); font-size: 12px; }

/* ---------- responsive ---------- */
@media (max-width: 880px) {
  .hero { padding: 72px 0 48px; }

  .feature-row, .feature-row.reverse {
    grid-template-columns: 1fr;
  }
  .feature-row.reverse .feature-text { order: unset; }
  .feature-row.reverse .feature-visual { order: unset; }

  .forms { grid-template-columns: 1fr; }

  .cookbook-grid { grid-template-columns: 1fr; }

  .vt-meta { display: none; }
}
</style>
