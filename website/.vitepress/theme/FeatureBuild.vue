<script setup>
import CustomPageLayout from './CustomPageLayout.vue'
</script>

<template>
  <CustomPageLayout active-nav="build" v-slot="{ copyText }">

    <!-- ============ HERO ============ -->
    <header class="hero">
      <span class="hero-paren l" aria-hidden="true">(</span>
      <span class="hero-paren r" aria-hidden="true">)</span>
      <div class="wrap">
        <p class="eyebrow">Feature<span class="sep">·</span>Standalone Executables<span class="sep">·</span>Cross-Compilation</p>
        <h1>One file out <em>the other end.</em></h1>
        <p class="lede">
          <code>sema build</code> traces your imports, bundles assets, and emits
          a self-contained binary. No venv on the server, no dependency pinning,
          no container just to run a script. <strong>The part Python never
          solved.</strong>
        </p>
        <div class="hero-actions">
          <a class="btn btn-gold" href="/docs/internals/executable-format">Read the docs</a>
          <a class="btn btn-ghost" href="https://sema.run">Try the playground</a>
        </div>
        <div class="hero-actions">
          <span class="install">
            <span class="cmd-text">
              <span class="dollar">$</span>
              <span id="i1">sema build agent.sema -o agent</span>
            </span>
            <button class="copy" @click="copyText('i1', $event)">copy</button>
          </span>
        </div>
        <p class="req">12 MB binary · no runtime needed · cross-compile for 5 platforms</p>
      </div>
    </header>

    <!-- ============ BUILD PIPELINE ============ -->
    <section class="pipeline-showcase">
      <div class="wrap">
        <p class="kicker">The build pipeline</p>
        <h2>From source to binary, in one command.</h2>

        <div class="pipeline">
          <div class="pipe-stage">
            <div class="pipe-icon">&#x1F4C4;</div>
            <div class="pipe-name">.sema source</div>
            <div class="pipe-desc">Your script + imports</div>
          </div>
          <div class="pipe-arrow">&rarr;</div>
          <div class="pipe-stage">
            <div class="pipe-icon">&#x2699;</div>
            <div class="pipe-name">compile</div>
            <div class="pipe-desc">Lower to bytecode (.semac)</div>
          </div>
          <div class="pipe-arrow">&rarr;</div>
          <div class="pipe-stage">
            <div class="pipe-icon">&#x1F517;</div>
            <div class="pipe-name">trace imports</div>
            <div class="pipe-desc">Recursive dependency walk</div>
          </div>
          <div class="pipe-arrow">&rarr;</div>
          <div class="pipe-stage">
            <div class="pipe-icon">&#x1F4E6;</div>
            <div class="pipe-name">VFS archive</div>
            <div class="pipe-desc">Bundle files + checksum</div>
          </div>
          <div class="pipe-arrow">&rarr;</div>
          <div class="pipe-stage">
            <div class="pipe-icon">&#x1F48A;</div>
            <div class="pipe-name">inject</div>
            <div class="pipe-desc">Embed into runtime binary</div>
          </div>
          <div class="pipe-arrow">&rarr;</div>
          <div class="pipe-stage pipe-stage-done">
            <div class="pipe-icon">&#x1F5A5;</div>
            <div class="pipe-name">executable</div>
            <div class="pipe-desc">Self-contained, ship it</div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ PYTHON PROBLEM ============ -->
    <section id="python-problem">
      <div class="wrap">
        <p class="kicker">The problem it solves</p>
        <h2>Deploy without the ritual.</h2>

        <div class="compare">
          <div class="pane python">
            <div class="pane-head">
              <span class="t">Python deployment</span>
              <span class="n">the ritual</span>
            </div>
            <pre><span class="c-kw">def</span> <span class="c-fn">deploy</span>():
    <span class="c-com"># 1. SSH in</span>
    ssh prod

    <span class="c-com"># 2. Create virtualenv</span>
    python3 -m venv venv
    source venv/bin/activate

    <span class="c-com"># 3. Install dependencies</span>
    pip install -r requirements.txt
    <span class="c-com"># hope versions haven't drifted…</span>

    <span class="c-com"># 4. Copy source</span>
    scp -r src/ prod:~/app/

    <span class="c-com"># 5. Run it</span>
    python agent.py

    <span class="c-com"># 6. Pray the runtime matches</span>
    <span class="c-com"># 7. Containerize if it doesn't</span></pre>
            <div class="pane-foot">
              A venv, a requirements.txt, a container, a CI pipeline to build
              the container — just to run a script.
            </div>
          </div>

          <div class="pane sema">
            <div class="pane-head">
              <span class="t">Sema deployment</span>
              <span class="n">one file</span>
            </div>
            <pre><span class="c-com"># Build locally</span>
sema build agent.sema -o agent
<span class="c-com">→ traced 3 imports, bundled 1 asset</span>
<span class="c-com">→ agent (self-contained, 12 MB)</span>

<span class="c-com"># Ship it</span>
scp agent prod: &amp;&amp; ssh prod ./agent
<span class="c-com">→ runs. that's it.</span></pre>
            <div class="pane-foot">
              One binary. No venv, no pip, no container. The runtime, bytecode,
              and assets are all inside.
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ CROSS-COMPILATION ============ -->
    <section id="cross-compile">
      <div class="wrap">
        <div class="feature-row">
          <div class="feature-text">
            <p class="kicker">Cross-compilation</p>
            <h2>Build from anywhere, for everywhere.</h2>
            <p class="sub">
              <code>--target linux</code> on macOS. <code>--target windows</code>
              on Linux. <code>--target all</code> produces five binaries in one
              command. Runtime binaries are downloaded, SHA256-verified, and
              cached — injection is format-aware, not host-specific.
            </p>
            <ul class="feature-list">
              <li><strong>Five targets.</strong> macOS ARM + Intel, Linux x86_64 + ARM, Windows. Cover every mainstream deployment target.</li>
              <li><strong>Any host &rarr; any target.</strong> Mach-O section injection works in pure Rust — build macOS ARM64 binaries from Linux.</li>
              <li><strong>Cached runtimes.</strong> Downloaded once, SHA256-verified, stored in <code>~/.sema/cache/</code>. <code>--no-cache</code> re-downloads.</li>
              <li><strong>Air-gapped support.</strong> <code>SEMA_RUNTIME_BASE_URL</code> overrides the download location for mirrors or offline builds.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="targets-grid">
              <div class="target-card">
                <div class="target-os">macOS ARM</div>
                <div class="target-triple">aarch64-apple-darwin</div>
                <div class="target-inject">Mach-O section</div>
              </div>
              <div class="target-card">
                <div class="target-os">macOS Intel</div>
                <div class="target-triple">x86_64-apple-darwin</div>
                <div class="target-inject">Mach-O section</div>
              </div>
              <div class="target-card">
                <div class="target-os">Linux x86_64</div>
                <div class="target-triple">x86_64-unknown-linux-gnu</div>
                <div class="target-inject">ELF append</div>
              </div>
              <div class="target-card">
                <div class="target-os">Linux ARM</div>
                <div class="target-triple">aarch64-unknown-linux-gnu</div>
                <div class="target-inject">ELF append</div>
              </div>
              <div class="target-card">
                <div class="target-os">Windows</div>
                <div class="target-triple">x86_64-pc-windows-msvc</div>
                <div class="target-inject">PE resource</div>
              </div>
            </div>
            <div class="term" style="margin-top:16px">
              <div class="head">cross-compile — all targets</div>
              <div><span class="dollar">$</span> sema build agent.sema --target all</div>
              <div class="out">→ agent-aarch64-apple-darwin (12.1 MB)</div>
              <div class="out">→ agent-x86_64-apple-darwin (12.4 MB)</div>
              <div class="out">→ agent-x86_64-unknown-linux-gnu (11.8 MB)</div>
              <div class="out">→ agent-aarch64-unknown-linux-gnu (11.6 MB)</div>
              <div class="out">→ agent-x86_64-pc-windows-msvc.exe (11.9 MB)</div>
              <div class="ok">✓ 5 binaries built</div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ BINARY LAYOUT ============ -->
    <section id="binary-layout">
      <div class="wrap">
        <p class="kicker">Binary layout</p>
        <h2>How the archive gets injected.</h2>
        <p class="sub">
          The injection strategy varies by binary format — detected from the
          runtime binary's magic bytes, not the build host. Each method
          preserves binary integrity and OS loader compatibility.
        </p>

        <div class="binary-layouts">
          <div class="layout-card">
            <div class="layout-head">
              <span class="layout-os">Linux (ELF)</span>
              <span class="layout-method">Raw append + trailer</span>
            </div>
            <div class="layout-stack">
              <div class="layout-layer layout-layer-binary">Original Sema Binary (ELF)</div>
              <div class="layout-layer layout-layer-archive">VFS Archive</div>
              <div class="layout-layer layout-layer-trailer">
                <span class="layout-hex">archive_size: u64 LE</span>
                <span class="layout-hex layout-hex-magic">magic: SEMAEXEC</span>
              </div>
            </div>
            <div class="layout-note">ELF loaders ignore appended data — the binary stays valid.</div>
          </div>

          <div class="layout-card">
            <div class="layout-head">
              <span class="layout-os">macOS (Mach-O)</span>
              <span class="layout-method">Section injection</span>
            </div>
            <div class="layout-stack">
              <div class="layout-layer layout-layer-binary">Mach-O Header</div>
              <div class="layout-layer">Load Commands</div>
              <div class="layout-layer">Segments</div>
              <div class="layout-layer layout-layer-archive"><span class="arch-tag">semaexec</span> section &larr; VFS archive</div>
            </div>
            <div class="layout-note">Injected via <code>libsui</code>, ad-hoc re-signed for ARM64.</div>
          </div>

          <div class="layout-card">
            <div class="layout-head">
              <span class="layout-os">Windows (PE)</span>
              <span class="layout-method">Resource injection</span>
            </div>
            <div class="layout-stack">
              <div class="layout-layer layout-layer-binary">PE Header</div>
              <div class="layout-layer">.text, .data</div>
              <div class="layout-layer">.rsrc</div>
              <div class="layout-layer layout-layer-archive"><span class="arch-tag">semaexec</span> resource &larr; VFS archive</div>
            </div>
            <div class="layout-note">Injected via <code>libsui</code>. Authenticode signatures stripped.</div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ VFS — BUNDLED FILES ============ -->
    <section id="vfs">
      <div class="wrap">
        <div class="feature-row reverse">
          <div class="feature-text">
            <p class="kicker">VFS — bundled files</p>
            <h2>Your files travel with the binary.</h2>
            <p class="sub">
              <code>--include data.json</code> or <code>--include assets/</code>
              bundles files into a virtual filesystem inside the executable. At
              runtime, <code>file/read</code>, <code>import</code>, and
              <code>load</code> check the VFS first, then the real filesystem.
              Your code doesn't change between dev and production.
            </p>
            <ul class="feature-list">
              <li><strong>Transparent interception.</strong> <code>file/read</code>, <code>file/exists?</code>, <code>import</code>, <code>load</code> — all check VFS first.</li>
              <li><strong>Recursive directories.</strong> <code>--include assets/</code> bundles everything underneath.</li>
              <li><strong>Integrity checked.</strong> CRC32-IEEE checksum on the archive — corruption is detected at load.</li>
              <li><strong>Writes go to real FS.</strong> <code>file/write</code>, <code>file/append</code>, <code>file/delete</code> always target the real filesystem, never the VFS.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="vfs-card">
              <div class="vfs-header">
                <span class="vfs-title">VFS Archive</span>
                <span class="vfs-meta">v1 · CRC32 · 4 entries</span>
              </div>
              <div class="vfs-toc">
                <div class="vfs-entry">
                  <span class="vfs-path vfs-path-entry">__main__.semac</span>
                  <span class="vfs-size">4.2 KB</span>
                  <span class="vfs-tag">bytecode</span>
                </div>
                <div class="vfs-entry">
                  <span class="vfs-path">lib/utils.sema</span>
                  <span class="vfs-size">890 B</span>
                  <span class="vfs-tag vfs-tag-import">traced import</span>
                </div>
                <div class="vfs-entry">
                  <span class="vfs-path">data.json</span>
                  <span class="vfs-size">12.1 KB</span>
                  <span class="vfs-tag vfs-tag-asset">--include</span>
                </div>
                <div class="vfs-entry">
                  <span class="vfs-path">prompts/system.txt</span>
                  <span class="vfs-size">340 B</span>
                  <span class="vfs-tag vfs-tag-asset">--include</span>
                </div>
              </div>
              <div class="vfs-footer">
                <span>Total: 17.5 KB</span>
                <span>checksum: verified</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ CAPABILITY SANDBOX ============ -->
    <section id="sandbox">
      <div class="wrap">
        <div class="feature-row">
          <div class="feature-text">
            <p class="kicker">Capability sandbox</p>
            <h2>Fence off what's dangerous.</h2>
            <p class="sub">
              <code>--sandbox</code> restricts shell access, filesystem writes,
              network calls, and LLM access — per group. <code>--allowed-paths</code>
              whitelists specific directories. Run untrusted code without
              exposing the host.
            </p>
            <ul class="feature-list">
              <li><strong>Strict mode.</strong> <code>--sandbox strict</code> blocks shell, network, and filesystem writes. Only <code>--allowed-paths</code> are readable.</li>
              <li><strong>Allowed paths.</strong> <code>--sandbox strict --allowed-paths ./data,./output</code> — granular filesystem access.</li>
              <li><strong>Per-capability.</strong> <code>--sandbox shell,network</code> — block only specific capabilities, allow the rest.</li>
            </ul>
          </div>
          <div class="feature-visual">
            <div class="sandbox-grid">
              <div class="sandbox-row sandbox-row-head">
                <span></span>
                <span>strict</span>
                <span>default</span>
                <span>all</span>
              </div>
              <div class="sandbox-row">
                <span class="sandbox-cap">Shell</span>
                <span class="sandbox-cell sandbox-cell-blocked">&#x2717;</span>
                <span class="sandbox-cell sandbox-cell-blocked">&#x2717;</span>
                <span class="sandbox-cell sandbox-cell-allowed">&#x2713;</span>
              </div>
              <div class="sandbox-row">
                <span class="sandbox-cap">File write</span>
                <span class="sandbox-cell sandbox-cell-blocked">&#x2717;</span>
                <span class="sandbox-cell sandbox-cell-allowed">&#x2713;</span>
                <span class="sandbox-cell sandbox-cell-allowed">&#x2713;</span>
              </div>
              <div class="sandbox-row">
                <span class="sandbox-cap">Network</span>
                <span class="sandbox-cell sandbox-cell-blocked">&#x2717;</span>
                <span class="sandbox-cell sandbox-cell-allowed">&#x2713;</span>
                <span class="sandbox-cell sandbox-cell-allowed">&#x2713;</span>
              </div>
              <div class="sandbox-row">
                <span class="sandbox-cap">LLM</span>
                <span class="sandbox-cell sandbox-cell-blocked">&#x2717;</span>
                <span class="sandbox-cell sandbox-cell-allowed">&#x2713;</span>
                <span class="sandbox-cell sandbox-cell-allowed">&#x2713;</span>
              </div>
              <div class="sandbox-row">
                <span class="sandbox-cap">File read</span>
                <span class="sandbox-cell sandbox-cell-partial">whitelist</span>
                <span class="sandbox-cell sandbox-cell-allowed">&#x2713;</span>
                <span class="sandbox-cell sandbox-cell-allowed">&#x2713;</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ============ CTA ============ -->
    <section class="cta">
      <div class="wrap">
        <h2>Build your first binary.</h2>
        <p class="sub">One command. Trace, compile, bundle, inject.</p>
        <div class="install-stack">
          <div class="install-row">
            <span class="badge">build</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="i2">sema build agent.sema -o agent</span>
              </span>
              <button class="copy" @click="copyText('i2', $event)">copy</button>
            </span>
          </div>
          <div class="install-row">
            <span class="badge">cross</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="i3">sema build agent.sema --target all</span>
              </span>
              <button class="copy" @click="copyText('i3', $event)">copy</button>
            </span>
          </div>
          <div class="install-row">
            <span class="badge">bundle</span>
            <span class="install">
              <span class="cmd-text">
                <span class="dollar">$</span>
                <span id="i4">sema build agent.sema --include assets/ -o agent</span>
              </span>
              <button class="copy" @click="copyText('i4', $event)">copy</button>
            </span>
          </div>
          <div class="hero-actions" style="justify-content:center; margin-top:24px">
            <a class="btn btn-gold" href="/docs/internals/executable-format">Executable format docs</a>
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

/* ---------- build pipeline ---------- */
.pipeline-showcase { padding: 0 0 88px; border-top: none; }

.pipeline {
  display: flex;
  align-items: stretch;
  gap: 0;
  margin-top: 48px;
  flex-wrap: wrap;
}

.pipe-stage {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  flex: 1;
  min-width: 120px;
  padding: 18px 12px;
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 10px;
  text-align: center;
}

.pipe-stage-done {
  border-color: var(--gold-line);
  background: var(--gold-fade);
}

.pipe-icon { font-size: 22px; line-height: 1; }

.pipe-name {
  font-family: var(--font-mono);
  font-size: 12.5px;
  font-weight: 500;
  color: var(--text);
}

.pipe-stage-done .pipe-name { color: var(--gold-bright); }

.pipe-desc {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--dim);
  line-height: 1.4;
  max-width: 110px;
}

.pipe-arrow {
  display: flex;
  align-items: center;
  font-size: 18px;
  color: var(--dim);
  padding: 0 6px;
  align-self: center;
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

.pane.python pre { color: #9b9486; }

.pane-foot {
  padding: 13px 18px;
  border-top: 1px solid var(--border-lo);
  font-size: 13px;
  color: var(--muted);
  line-height: 1.55;
}

.pane.sema .pane-foot { color: var(--text); }

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
  white-space: nowrap;
}

/* ---------- targets grid ---------- */
.targets-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 10px;
}

.target-card {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 14px 16px;
}

.target-os {
  font-family: var(--font-body);
  font-size: 13.5px;
  font-weight: 500;
  color: var(--text);
  margin-bottom: 4px;
}

.target-triple {
  font-family: var(--font-mono);
  font-size: 10.5px;
  color: var(--dim);
  margin-bottom: 6px;
}

.target-inject {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 2px 8px;
  border-radius: 4px;
  display: inline-block;
}

/* ---------- binary layouts ---------- */
.binary-layouts {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 16px;
  margin-top: 40px;
}

.layout-card {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
}

.layout-head {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  gap: 8px;
  padding: 12px 16px;
  border-bottom: 1px solid var(--border-lo);
  font-family: var(--font-mono);
  font-size: 11.5px;
}

.layout-os { color: var(--gold-bright); font-weight: 500; }
.layout-method { color: var(--dim); font-size: 10.5px; }

.layout-stack {
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.layout-layer {
  padding: 10px 14px;
  background: var(--bg);
  border: 1px solid var(--border-lo);
  border-radius: 6px;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--muted);
  text-align: center;
}

.layout-layer-binary { color: var(--text); font-weight: 500; }

.layout-layer-archive {
  border-color: var(--gold-line);
  background: var(--gold-fade);
  color: var(--gold-bright);
  font-weight: 500;
}

.layout-layer-trailer {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  font-size: 9.5px;
}

.layout-hex { color: var(--dim); }
.layout-hex-magic { color: var(--gold); }

.arch-tag {
  display: inline-block;
  font-size: 9px;
  color: var(--gold);
  background: rgba(200, 168, 85, 0.14);
  padding: 1px 6px;
  border-radius: 3px;
  margin-right: 6px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.layout-note {
  padding: 10px 16px;
  border-top: 1px solid var(--border-lo);
  font-size: 11.5px;
  color: var(--dim);
  line-height: 1.5;
}

.layout-note code {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 5px;
  border-radius: 3px;
}

/* ---------- VFS card ---------- */
.vfs-card {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
  box-shadow: 0 0 0 1px rgba(200, 168, 85, .04), 0 20px 50px -30px rgba(0, 0, 0, .3);
}

.vfs-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 12px 16px;
  background: var(--surface);
  border-bottom: 1px solid var(--border-lo);
  font-family: var(--font-mono);
  font-size: 12px;
}

.vfs-title { color: var(--gold-bright); font-weight: 500; }
.vfs-meta { color: var(--dim); font-size: 10.5px; }

.vfs-toc {
  padding: 8px 16px;
}

.vfs-entry {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 0;
  border-bottom: 1px solid var(--border-lo);
  font-family: var(--font-mono);
  font-size: 12px;
}

.vfs-entry:last-child { border-bottom: none; }

.vfs-path {
  color: var(--text);
  flex-grow: 1;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.vfs-path-entry { color: var(--gold-bright); font-weight: 500; }

.vfs-size {
  color: var(--dim);
  font-size: 11px;
  flex-shrink: 0;
}

.vfs-tag {
  font-size: 9px;
  color: var(--dim);
  background: var(--surface);
  padding: 2px 7px;
  border-radius: 3px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  flex-shrink: 0;
}

.vfs-tag-import { color: var(--gold-bright); background: var(--gold-fade); }
.vfs-tag-asset { color: #9bb87a; background: rgba(155, 184, 122, 0.08); }

.vfs-footer {
  display: flex;
  justify-content: space-between;
  padding: 10px 16px;
  background: var(--surface);
  border-top: 1px solid var(--border-lo);
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--dim);
}

/* ---------- sandbox grid ---------- */
.sandbox-grid {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
}

.sandbox-row {
  display: grid;
  grid-template-columns: 1fr repeat(3, 60px);
  align-items: center;
  gap: 0;
}

.sandbox-row-head {
  background: var(--surface);
  border-bottom: 1px solid var(--border-lo);
}

.sandbox-row-head span {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--dim);
  text-align: center;
  padding: 10px 8px;
}

.sandbox-row-head span:first-child { text-align: left; padding-left: 16px; }

.sandbox-row:not(.sandbox-row-head) {
  border-bottom: 1px solid var(--border-lo);
}

.sandbox-row:last-child { border-bottom: none; }

.sandbox-cap {
  font-family: var(--font-mono);
  font-size: 12.5px;
  color: var(--text);
  padding: 12px 16px;
}

.sandbox-cell {
  display: flex;
  align-items: center;
  justify-content: center;
  font-family: var(--font-mono);
  font-size: 13px;
  padding: 12px 8px;
}

.sandbox-cell-allowed { color: #9bb87a; }
.sandbox-cell-blocked { color: #c97b6a; }
.sandbox-cell-partial {
  color: var(--gold-bright);
  font-size: 10px;
  background: var(--gold-fade);
  border-radius: 4px;
  margin: 4px 8px;
  padding: 4px 6px;
}

/* ---------- responsive ---------- */
@media (max-width: 880px) {
  .hero { padding: 72px 0 48px; }

  .pipeline { flex-direction: column; gap: 8px; }
  .pipe-arrow { transform: rotate(90deg); padding: 4px 0; }

  .compare { grid-template-columns: 1fr; }

  .feature-row, .feature-row.reverse {
    grid-template-columns: 1fr;
  }
  .feature-row.reverse .feature-text { order: unset; }
  .feature-row.reverse .feature-visual { order: unset; }

  .binary-layouts { grid-template-columns: 1fr; }
  .targets-grid { grid-template-columns: 1fr; }
}
</style>
