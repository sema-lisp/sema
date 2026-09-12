<script setup>
import { onMounted } from 'vue'
import CustomPageLayout from './CustomPageLayout.vue'

// <sema-code-typer> ships in the published @sema-lang/ui package. Loaded lazily
// on the client, like the brand page does, so nothing reaches outside website/.
onMounted(() => { import('@sema-lang/ui/standalone') })

// Typed out by <sema-code-typer>. This is the shape of the file Sema Coder
// writes on first run, trimmed to the keys worth showing.
const configSource = `(configure!
  (coder-config
    {:model     "claude-opus-5"
     :max-turns 50

     ;; MCP servers — stdio or http
     :mcp-servers
     (list
       (mcp-server "sema"
         {:command "sema"
          :args ["mcp"]
          :autostart #t}))

     ;; a slash command, as a lambda
     :commands
     (list
       (command "hi"
         {:desc "greet"
          :do (lambda (state args)
                (emit :info "hi!")
                state)}))}))`

// Line counts are `wc -l` over sema-lisp/sema-coder at v0.3.0. Re-measure
// before changing them; the repo's own ROADMAP figure went stale once already.
const files = [
  { name: 'tui.sema', lines: 1637, what: 'Full-screen TUI — frame diffing, async turns' },
  { name: 'tools.sema', lines: 773, what: 'The ten tools the model can call' },
  { name: 'commands.sema', lines: 700, what: 'Slash commands, config apply, diagnostics' },
  { name: 'overlay.sema', lines: 573, what: 'Modal overlays — MCP manager, session picker' },
  { name: 'config.sema', lines: 440, what: 'Config language, validation, defaults' },
  { name: 'terminal_input.sema', lines: 413, what: 'Raw-mode input + terminal event decoding' },
  { name: 'transcript.sema', lines: 256, what: 'Transcript blocks → styled lines' },
  { name: 'selection.sema', lines: 228, what: 'Selection, highlighting, clipboard' },
  { name: 'markdown.sema', lines: 214, what: 'Markdown rendering for the transcript' },
  { name: 'session.sema', lines: 183, what: 'Sessions as JSONL, atomic writes' },
  { name: 'mcp.sema', lines: 176, what: 'MCP client — connect, merge tools, autostart' },
  { name: 'agent.sema', lines: 173, what: 'The agent loop and system prompt' },
]
</script>

<template>
  <CustomPageLayout active-nav="coder" v-slot="{ copyText }">

    <!-- ============ HERO ============ -->
    <header class="hero">
      <span class="hero-paren l" aria-hidden="true">(</span>
      <span class="hero-paren r" aria-hidden="true">)</span>
      <div class="wrap">
        <p class="eyebrow">Sema Coder<span class="sep">·</span>Terminal agent<span class="sep">·</span>Written in Sema</p>
        <h1>A coding agent <em>written in Sema.</em></h1>
        <p class="lede">
          It edits files, runs commands, and searches your code from the
          terminal. The agent loop, the tools, the TUI, the MCP client, and the
          config language are all <code>.sema</code> files. <strong>There is no
          build step, and nothing to install but the <code>sema</code>
          binary.</strong>
        </p>
        <div class="hero-actions">
          <a class="btn btn-gold" href="https://github.com/sema-lisp/sema-coder">View the source</a>
          <a class="btn btn-ghost" href="/docs/mcp#sema-coder">MCP setup</a>
        </div>
        <div class="hero-actions">
          <span class="install">
            <span class="cmd-text">
              <span class="dollar">$</span>
              <span id="i1">git clone https://github.com/sema-lisp/sema-coder &amp;&amp; cd sema-coder</span>
            </span>
            <button class="copy" @click="copyText('i1', $event)">copy</button>
          </span>
        </div>
        <p class="req">Needs sema ≥ 1.35 · ANTHROPIC_API_KEY or OPENAI_API_KEY · MIT · v0.3.0</p>
      </div>
    </header>

    <!-- ============ SESSION MOCK ============ -->
    <section id="session" class="shot-section">
      <div class="wrap">
        <div class="tui">
          <div class="tui-head">
            <span class="g">(sema)</span> coder — ~/code/sema · claude-opus-5
            <span class="tui-right">⌃O mcp · ⌃R resume · ⌃K palette · ⌃D quit</span>
          </div>
          <div class="tui-body">
            <div class="ep">; sixty years of parentheses; the good ideas kept their shape.</div>
            <div class="sp"></div>
            <div class="usr"><span class="bar">▌</span> read test.sema and explain how the test runner works</div>
            <div class="sp"></div>
            <div class="tool"><span class="ok">✓</span> <span class="tn">read-file</span> <span class="dim">test.sema · 0ms</span></div>
            <div class="dim">  1  #!/usr/bin/env sema</div>
            <div class="dim">  2  ;; test.sema — sema-coder's test runner, written in Sema</div>
            <div class="dim">  …  +141 lines</div>
            <div class="sp"></div>
            <div>Each file runs in a child <span class="g">sema</span> interpreter, so a crash</div>
            <div>in one test cannot take the run down and no state leaks between</div>
            <div>files. Pass or fail comes from the child's exit code.</div>
            <div class="sp"></div>
            <div class="tool"><span class="ok">✓</span> <span class="tn">bash</span> <span class="dim">./test.sema · 4.2s</span></div>
            <div class="dim">  312 checks, 0 failed</div>
            <div class="sp"></div>
            <div class="prompt"><span class="g">❯</span> <span class="cursor"></span></div>
          </div>
          <div class="tui-foot">tab queues · enter interrupts and sends · ⌃C cancels the turn</div>
        </div>
        <p class="req shot-cap">The TUI rebuilds every frame as a list of rows and rewrites only the rows that changed, so there is no clear and no flicker.</p>
      </div>
    </section>

    <!-- ============ WHAT RUNS WHERE ============ -->
    <section id="all-the-way-down">
      <div class="wrap">
        <p class="kicker">What runs where</p>
        <h2>What is Sema, and <em>what is Rust.</em></h2>
        <p class="sub">
          Sema makes LLM calls, tools, and agents part of the language instead
          of a library on top of it. Sema Coder is the test of that: the kind of
          program most people write in TypeScript or Go, written in Sema
          instead. The repository contains no Rust at all.
        </p>

        <div class="split">
          <div class="pane sema">
            <div class="pane-head"><span class="t">Sema</span><span class="n">6,703 lines, 23 files</span></div>
            <div class="pane-body">
              <p>Agent loop and system prompt. The ten tools. Slash commands and
              the key map. The TUI — frame diffing, scrollback, selection,
              markdown. Modal overlays. The MCP client. Sessions. The config
              language. The plugin system.</p>
            </div>
          </div>
          <div class="pane">
            <div class="pane-head"><span class="t">Rust</span><span class="n">0 lines in this repo</span></div>
            <div class="pane-body">
              <p>Host primitives inside the <code>sema</code> binary that Sema
              Coder calls: <code>agent/run</code>, <code>async/*</code>,
              <code>term/*</code>, <code>proc/*</code>, <code>file/*</code>,
              <code>mcp/connect</code>, and <code>path/within?</code>. There is
              no Rust source, and no <code>Cargo.toml</code>, in the repository.</p>
            </div>
          </div>
        </div>

        <div class="file-table">
          <div class="ft-head"><span>Largest source files</span><span>lines</span></div>
          <div class="ft-row" v-for="f in files" :key="f.name">
            <span class="ft-name">src/{{ f.name }}</span>
            <span class="ft-what">{{ f.what }}</span>
            <span class="ft-lines">{{ f.lines }}</span>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ WHAT IT DOES ============ -->
    <section id="tools">
      <div class="wrap">
        <div class="feature-row">
          <div class="feature-text">
            <p class="kicker">Ten tools</p>
            <h2>Ten tools.</h2>
            <p class="sub">
              Read, write, edit, search, shell, and background processes. Each
              one caps its output, so a large file or a noisy command cannot
              fill the context window.
            </p>
            <ul class="feature-list">
              <li><strong>Files.</strong> <code>read-file</code> (line-numbered, refuses binaries), <code>write-file</code> (creates parent dirs), <code>edit-file</code> (one exact match, returns a diff).</li>
              <li><strong>Search.</strong> <code>grep</code> prefers <code>rg</code> and falls back to <code>grep</code>; <code>find-files</code> skips <code>.git</code>, <code>node_modules</code>, and <code>target</code>; <code>list-dir</code>.</li>
              <li><strong>Shell.</strong> <code>bash</code> with a 120-second default timeout, or <code>background=true</code> for a task id — then <code>task-output</code>, <code>task-list</code>, <code>task-stop</code>.</li>
              <li><strong>Path checks.</strong> Every file and search path goes through <code>path/within?</code>, which resolves symlinks, so <code>../</code> and symlink escapes both fail. The <code>bash</code> tool is not covered: it runs real commands with your privileges. This is accident prevention, not a sandbox.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="code-card">
              <div class="code-card-head">
                <span class="t">one-shot mode</span>
                <span class="n">pipeable</span>
              </div>
              <pre><span class="c-com"># prose to stdout</span>
$ ./coder.sema -- -p <span class="c-str">"explain this codebase"</span>

<span class="c-com"># exactly one JSON document</span>
$ ./coder.sema -- --json -p <span class="c-str">"..."</span>

<span class="c-com"># stdin is appended in a &lt;stdin&gt; block</span>
$ git diff | ./coder.sema -- \
    -p <span class="c-str">"review this change"</span>

<span class="c-com"># pick a model</span>
$ ./coder.sema -- -m claude-haiku-4-5</pre>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ CONFIG ============ -->
    <section id="config">
      <div class="wrap">
        <div class="feature-row reverse">
          <div class="feature-text">
            <p class="kicker">Config</p>
            <h2>The config is <em>Lisp data.</em></h2>
            <p class="sub">
              <code>init.sema</code> calls <code>(configure! (coder-config {…}))</code>.
              The config is code, so a command can be a lambda, a hook can close
              over state, and a model entry can carry its own defaults. No
              template syntax had to be invented for any of it.
            </p>
            <ul class="feature-list">
              <li><strong>Validated, then applied.</strong> Loading is pure: a broken file reconciles nothing and keeps the last good commands, servers, hooks, and tools. An unknown key is a warning, not a failure.</li>
              <li><strong>Hot reload.</strong> The TUI watches the config dir and applies a valid save; <code>/reload</code> does the same by hand. Both go through one <code>apply-config!</code> path.</li>
              <li><strong>Written on first run.</strong> The annotated template is generated from the provider table, so the model list in your config cannot drift from the one in the code.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <sema-code-typer frame logo status line-numbers loop
                             filename="init.sema" lang="sema" rows="20" cps="45">{{ configSource }}</sema-code-typer>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ MCP ============ -->
    <section id="mcp">
      <div class="wrap">
        <div class="feature-row">
          <div class="feature-text">
            <p class="kicker">MCP client</p>
            <h2>It is an <em>MCP client.</em></h2>
            <p class="sub">
              Sema Coder connects to MCP servers over stdio or HTTP and merges
              their tools into the agent. The server configured by default is
              Sema's own, so the agent can evaluate Sema and read Sema docs
              while it edits Sema.
            </p>
            <ul class="feature-list">
              <li><strong>Autostart or on demand.</strong> <code>:autostart #t</code> connects at boot, before the alt-screen opens, so a first-time OAuth prompt is visible. Everything else connects from the modal.</li>
              <li><strong>The <code>/mcp</code> modal (⌃O).</strong> <code>↑↓</code> move, <code>c</code> connect, <code>d</code> disconnect, <code>⏎</code> tools, <code>e</code> edit config. A server needing auth shows <code>▲</code>.</li>
              <li><strong>Name collisions are reported, not silent.</strong> Built-ins win over MCP tools, the first server to connect wins between servers, and <code>/tools</code> annotates each tool with the server it came from.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="term">
              <div class="head">/mcp — MCP servers</div>
              <div><span class="ok">●</span> sema        connected · 3 tools</div>
              <div><span class="dot">▲</span> asana       needs auth — press c to sign in</div>
              <div><span class="out">◌</span> filesystem  idle — press c to connect</div>
              <div class="sp"></div>
              <div class="out">↑↓ move · c connect · d disconnect · ⏎ tools · e edit · esc close</div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ EXTENDING ============ -->
    <section id="extending">
      <div class="wrap">
        <p class="kicker">Extension API</p>
        <h2>Extending it.</h2>
        <p class="sub">
          Put a <code>.sema</code> file in <code>plugins/</code>, either globally
          or in a project's <code>.sema-coder/plugins/</code>. It is loaded at
          boot with the full API available. A broken plugin prints an error and
          the agent still starts.
        </p>

        <div class="cards">
          <div class="card">
            <h3>Tools</h3>
            <p><code>deftool</code> then <code>register-tool!</code>. Files in
            <code>tools/</code> are reconciled on every config apply: delete one
            and the tool goes away, edit one and the tool is replaced instead of
            duplicated.</p>
          </div>
          <div class="card">
            <h3>Commands</h3>
            <p><code>register-command!</code> with a <code>(state args)</code>
            handler, plus <code>register-completions!</code> for its arguments.
            Return the next state, or <code>'quit</code>.</p>
          </div>
          <div class="card">
            <h3>Hooks</h3>
            <p>Seven events: <code>:session-start</code>, <code>:pre-turn</code>,
            <code>:pre-tool-call</code>, <code>:post-turn</code>,
            <code>:on-error</code>, <code>:turn-queued</code>,
            <code>:turn-interrupted</code>. A handler that throws is caught, so
            it cannot break the turn.</p>
          </div>
          <div class="card">
            <h3>Keys</h3>
            <p><code>bind-key!</code> over four merge layers: defaults, the
            command's own <code>:key</code>, the config <code>:keys</code> map,
            then runtime binds. Conflicts are reported at boot.</p>
          </div>
          <div class="card">
            <h3>Overlays</h3>
            <p><code>register-overlay!</code> with <code>:render</code> and
            <code>:on-key</code>, or reuse <code>list-modal</code>, the widget
            the MCP manager and the session picker are built from.</p>
          </div>
          <div class="card">
            <h3>Prompt</h3>
            <p><code>prompt-text</code>, <code>prompt-insert!</code>,
            <code>prompt-set!</code>, <code>prompt-clear!</code> read and rewrite
            what the user is typing, from inside a command.</p>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ INTERRUPTION / SESSIONS ============ -->
    <section id="turns">
      <div class="wrap">
        <div class="feature-row reverse">
          <div class="feature-text">
            <p class="kicker">Turns and sessions</p>
            <h2>Interrupting a turn <em>keeps the history valid.</em></h2>
            <p class="sub">
              Cancelling a turn during a tool call is how an agent ends up with
              a history the provider rejects. Sema Coder repairs it: every
              orphaned tool call gets a synthesized, correlated result, the
              assistant text written so far is kept, and a typed control message
              records what happened.
            </p>
            <ul class="feature-list">
              <li><strong>The turn is an async task.</strong> A second task keeps drawing the UI for the whole turn, so keys stay live and <code>Ctrl-C</code> cancels the turn cleanly.</li>
              <li><strong>Sessions are JSONL.</strong> The TUI writes every turn: one meta line, then one message per line in the exact shape <code>agent/run</code> takes, to a temp file that is renamed — a crash mid-write cannot truncate the old one. One-shot and the plain REPL do not persist.</li>
              <li><strong>Queue a follow-up mid-turn.</strong> <code>Tab</code> queues, <code>/queue</code> lists, edits, drops, or resumes. <code>/resume</code> reopens a past session by id.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="code-card">
              <div class="code-card-head">
                <span class="t">session.jsonl</span>
                <span class="n">line 1 is meta</span>
              </div>
              <pre>{<span class="c-kwd">"kind"</span>:<span class="c-str">"meta"</span>,<span class="c-kwd">"id"</span>:<span class="c-str">"01J…"</span>,
 <span class="c-kwd">"model"</span>:<span class="c-str">"claude-opus-5"</span>,
 <span class="c-kwd">"cwd"</span>:<span class="c-str">"/work/sema"</span>}
{<span class="c-kwd">"role"</span>:<span class="c-str">"user"</span>,…}
{<span class="c-kwd">"role"</span>:<span class="c-str">"assistant"</span>,…}
{<span class="c-kwd">"role"</span>:<span class="c-str">"tool"</span>,…}

<span class="c-com">;; restored verbatim by /resume</span></pre>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ TESTS ============ -->
    <section id="tests">
      <div class="wrap">
        <p class="kicker">Tests</p>
        <h2>The tests are <em>Sema too.</em></h2>
        <p class="sub">
          <code>test.sema</code> runs each <code>tests/*_test.sema</code> in a
          child <code>sema</code> interpreter, so a crash stays in that file and
          no state leaks between files. A passing child must also print the
          harness summary, so forgetting <code>(done)</code> cannot report a
          false pass.
        </p>
        <div class="stats">
          <div class="stat"><span class="v">3,400</span><span class="k">lines of tests</span></div>
          <div class="stat"><span class="v">36</span><span class="k">test files</span></div>
          <div class="stat"><span class="v">2</span><span class="k">sema versions in CI</span></div>
          <div class="stat"><span class="v">0</span><span class="k">network calls needed</span></div>
        </div>
        <p class="sub afterstats">
          The turn controller takes a scripted runner and a no-op renderer, so
          queueing, steering, cancellation, late deltas, and event ordering are
          tested without a network or a terminal. Writing the agent also found
          gaps in the language itself, filed as issues #82–#94 on the Sema
          repository. Eight are fixed and used here through released APIs; five
          are still open and worked around in the source.
        </p>
      </div>
    </section>

    <!-- ============ CTA ============ -->
    <section class="cta">
      <div class="wrap">
        <h2>Try it.</h2>
        <p class="sub">
          Install <code>sema</code>, clone the repository, export an API key.
          Every part of the agent you then run is a file you can edit.
        </p>
        <div class="install-stack">
          <div class="install-row">
            <span class="badge">1</span>
            <span class="install">
              <span class="cmd-text"><span class="dollar">$</span><span id="c1">curl -fsSL https://sema-lang.com/install.sh | sh</span></span>
              <button class="copy" @click="copyText('c1', $event)">copy</button>
            </span>
          </div>
          <div class="install-row">
            <span class="badge">2</span>
            <span class="install">
              <span class="cmd-text"><span class="dollar">$</span><span id="c2">git clone https://github.com/sema-lisp/sema-coder</span></span>
              <button class="copy" @click="copyText('c2', $event)">copy</button>
            </span>
          </div>
          <div class="install-row">
            <span class="badge">3</span>
            <span class="install">
              <span class="cmd-text"><span class="dollar">$</span><span id="c3">export ANTHROPIC_API_KEY=… &amp;&amp; ./coder.sema</span></span>
              <button class="copy" @click="copyText('c3', $event)">copy</button>
            </span>
          </div>
        </div>
        <p class="req cta-req">
          Optional: install <code>rg</code> and the grep tool uses it instead of <code>grep</code>.
        </p>
      </div>
    </section>

  </CustomPageLayout>
</template>

<style scoped>
.hero { padding: 104px 0 76px; }

/* ---------- session mock (HTML, not a screenshot) ---------- */
.shot-section { padding-top: 0; border-top: none; }
.tui {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
  font-family: var(--font-mono);
  font-size: 13px;
  line-height: 1.75;
  color: #c9c2b4;
  box-shadow: 0 30px 70px -40px rgba(0, 0, 0, .6);
}
.tui-head, .tui-foot {
  padding: 11px 18px;
  color: var(--dim);
  font-size: 11.5px;
}
.tui-head { border-bottom: 1px solid var(--border-lo); display: flex; gap: 16px; flex-wrap: wrap; }
.tui-foot { border-top: 1px solid var(--border-lo); }
.tui-right { margin-left: auto; }
.tui-body { padding: 16px 18px; overflow-x: auto; }
.tui-body > div { white-space: pre; }
.tui .g { color: var(--gold-bright); }
.tui .dim { color: var(--dim); }
.tui .ep { color: #665e50; font-style: italic; }
.tui .sp { height: 12px; }
.tui .usr { color: var(--text); }
.tui .usr .bar { color: var(--gold); margin-right: 6px; }
.tui .tool .ok { color: #9bb87a; }
.tui .tool .tn { color: var(--gold-bright); }
.tui .prompt .cursor {
  display: inline-block;
  width: 7px;
  height: 15px;
  margin-left: 4px;
  background: var(--gold);
  vertical-align: -2px;
  animation: tui-blink 1.1s steps(1) infinite;
}
@keyframes tui-blink { 50% { opacity: 0; } }
@media (prefers-reduced-motion: reduce) {
  .tui .prompt .cursor { animation: none; }
}
.shot-cap { margin-top: 14px; max-width: 74ch; line-height: 1.6; }

/* ---------- typed config ---------- */
sema-code-typer { display: block; width: 100%; }

/* ---------- shared feature scaffolding (same shape as /feature/*) ---------- */
.feature-row {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 56px;
  align-items: center;
  margin-top: 20px;
}
.feature-row.reverse .feature-text { order: 2; }
.feature-row.reverse .feature-visual { order: 1; }

.feature-list { margin-top: 24px; }
.feature-list li {
  padding: 10px 0;
  font-size: 14.5px;
  color: var(--muted);
  line-height: 1.65;
  border-bottom: 1px solid var(--border-lo);
}
.feature-list li:last-child { border-bottom: none; }
.feature-list strong { color: var(--text); font-weight: 500; display: block; margin-bottom: 2px; }
.feature-list code {
  font-family: var(--font-mono);
  font-size: 12.5px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
}

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

/* ---------- sema / rust split ---------- */
.split {
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
.pane-body { padding: 18px 20px; font-size: 14px; line-height: 1.65; color: var(--muted); }
.pane-body code {
  font-family: var(--font-mono);
  font-size: 12.5px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
}

/* ---------- file table ---------- */
.file-table {
  margin-top: 30px;
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
  font-family: var(--font-mono);
  font-size: 12.5px;
}
.ft-head {
  display: flex;
  justify-content: space-between;
  padding: 11px 18px;
  border-bottom: 1px solid var(--border-lo);
  color: var(--dim);
  font-size: 11.5px;
  letter-spacing: .08em;
  text-transform: uppercase;
}
.ft-row {
  display: grid;
  grid-template-columns: 200px 1fr 60px;
  gap: 16px;
  padding: 9px 18px;
  border-bottom: 1px solid var(--border-lo);
}
.ft-row:last-child { border-bottom: none; }
.ft-name { color: var(--gold-bright); }
.ft-what { color: var(--muted); font-family: var(--font-base); font-size: 13.5px; }
.ft-lines { color: var(--dim); text-align: right; }

/* ---------- terminal mockup extras ---------- */
.term .sp { height: 10px; }

/* ---------- extension cards ---------- */
.cards {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 20px;
  margin-top: 46px;
}
.card {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 22px 22px 24px;
}
.card h3 {
  font-family: var(--font-mono);
  font-size: 12px;
  letter-spacing: .1em;
  text-transform: uppercase;
  color: var(--gold-bright);
  margin-bottom: 12px;
}
.card p { font-size: 14px; line-height: 1.65; color: var(--muted); }
.card code {
  font-family: var(--font-mono);
  font-size: 12.5px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 4px;
}

/* ---------- stats ---------- */
.stats {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 20px;
  margin: 46px 0 34px;
}
.stat {
  border-left: 1px solid var(--gold-line);
  padding-left: 18px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.stat .v { font-family: var(--font-display); font-size: 38px; line-height: 1; color: var(--gold-bright); }
.stat .k { font-family: var(--font-mono); font-size: 11.5px; letter-spacing: .06em; text-transform: uppercase; color: var(--dim); }
.afterstats { margin-top: 0; }

.cta-req { margin-top: 26px; }

/* ---------- responsive ---------- */
@media (max-width: 880px) {
  .hero { padding: 72px 0 48px; }
  .feature-row, .feature-row.reverse { grid-template-columns: 1fr; }
  .feature-row.reverse .feature-text { order: unset; }
  .feature-row.reverse .feature-visual { order: unset; }
  .split { grid-template-columns: 1fr; }
  .cards { grid-template-columns: 1fr; }
  .stats { grid-template-columns: 1fr 1fr; }
  .ft-row { grid-template-columns: 1fr 52px; }
  .ft-what { display: none; }
}
</style>
