<script setup>
import { ref } from 'vue'
import {
  sMarkTile, sMarkSquare, logotype,
  fileSemaLight, fileSemaDark, fileSemac, fileNotebookLight, fileNotebookDark,
} from './brandAssets'

// Which plugin-icon direction to preview inside the store replicas.
const candidate = ref('s') // 's' | 'logo'

// A square plugin tile for the chosen candidate at a given pixel size.
// `(s)` is already a self-backgrounded dark tile; the wordmark gets wrapped in
// one with padding so it survives a square slot.
function tile(kind, px) {
  if (kind === 's') return `<div class="tile-svg" style="width:${px}px;height:${px}px">${sMarkTile}</div>`
  const pad = Math.round(px * 0.16), r = Math.round(px * 0.19)
  return `<div class="logo-tile" style="width:${px}px;height:${px}px;border-radius:${r}px;padding:${pad}px 0">${logotype}</div>`
}

const sizes = [
  { px: 40, note: 'JetBrains in-IDE list · pluginIcon base' },
  { px: 72, note: 'JetBrains Marketplace card' },
  { px: 80, note: 'JetBrains Marketplace detail' },
  { px: 128, note: 'VS Code Marketplace detail' },
]

const fileRows = [
  { name: 'hello.sema', dark: fileSemaDark, light: fileSemaLight },
  { name: 'program.semac', dark: fileSemac, light: fileSemac },
  { name: 'notes.sema-nb', dark: fileNotebookDark, light: fileNotebookLight },
]
</script>

<template>
  <div class="icon-showcase">
    <div class="wrap">
      <header class="hero">
        <span class="eyebrow">Internal reference · unlinked</span>
        <h1>Icon Manifest</h1>
        <p class="hero-desc">
          Every plugin and file icon Sema ships, rendered at the exact sizes and on the real
          background colors each marketplace and IDE uses — measured live from the VS&nbsp;Code
          Marketplace, Open&nbsp;VSX, and JetBrains Marketplace, not assumed. The decision surface
          for the final plugin-icon direction.
        </p>
      </header>

      <!-- ─────────────  PLUGIN ICON DECISION  ───────────── -->
      <section class="sec">
        <div class="sec-head">
          <span class="sec-num">01</span>
          <h2 class="sec-title">Plugin-icon direction</h2>
          <p class="sec-desc">
            The two candidates at every real marketplace size, on light and dark. Open&nbsp;VSX
            renders on <code>#121212</code>; VS&nbsp;Code and JetBrains on white — so the icon has to
            hold up on both.
          </p>
        </div>

        <div class="cand-grid">
          <div class="cand">
            <span class="cand-label">A — <code>(s)</code> mark tile <em class="chosen">✓ chosen</em></span>
            <div class="swatch" v-for="bg in ['light','dark']" :key="'s'+bg" :class="bg">
              <div class="sample" v-for="s in sizes" :key="s.px">
                <div v-html="tile('s', s.px)"></div>
                <span class="px">{{ s.px }}</span>
              </div>
            </div>
          </div>

          <div class="cand">
            <span class="cand-label">B — <code>(sema)</code> logotype tile</span>
            <div class="swatch" v-for="bg in ['light','dark']" :key="'l'+bg" :class="bg">
              <div class="sample" v-for="s in sizes" :key="s.px">
                <div v-html="tile('logo', s.px)"></div>
                <span class="px">{{ s.px }}</span>
              </div>
            </div>
          </div>
        </div>
        <ul class="legend">
          <li v-for="s in sizes" :key="s.px"><b>{{ s.px }}px</b> — {{ s.note }}</li>
        </ul>

        <div class="variants">
          <span class="cand-label">Background shapes</span>
          <p class="variants-desc">
            Use the <b>rounded</b> tile as the marketplace / app icon; use the <b>full-bleed square</b>
            wherever the slot applies its own mask — GitHub / social avatars, org logos — so corners
            aren't double-rounded or clipped.
          </p>
          <div class="variant-row">
            <div class="variant">
              <div class="variant-box" v-html="sMarkTile"></div>
              <span class="v-name">Rounded</span>
              <span class="v-file"><code>favicon.svg</code></span>
            </div>
            <div class="variant">
              <div class="variant-box" v-html="sMarkSquare"></div>
              <span class="v-name">Full-bleed square</span>
              <span class="v-file"><code>avatar.svg</code> · <code>avatar.png</code></span>
            </div>
            <div class="variant">
              <div class="variant-box circle" v-html="sMarkSquare"></div>
              <span class="v-name">Square under a circle mask</span>
              <span class="v-file">avatar slots (GitHub, Slack…)</span>
            </div>
          </div>
        </div>
      </section>

      <!-- ─────────────  STORE REPLICAS  ───────────── -->
      <section class="sec">
        <div class="sec-head">
          <span class="sec-num">02</span>
          <h2 class="sec-title">In context — marketplace replicas</h2>
          <p class="sec-desc">Pixel-faithful to each store. Flip the icon to compare both directions where they actually appear.</p>
        </div>

        <div class="toggle">
          <span class="toggle-label">Preview</span>
          <button :class="{ on: candidate === 's' }" @click="candidate = 's'">A · (s) mark</button>
          <button :class="{ on: candidate === 'logo' }" @click="candidate = 'logo'">B · (sema) logotype</button>
        </div>

        <div class="panel">
          <span class="panel-label">VS Code Marketplace · detail</span>
          <div class="replica vsm">
            <div class="vsm-top"><span class="vsm-logo">▨ Visual Studio</span> <span class="vsm-sep">|</span> Marketplace</div>
            <div class="vsm-crumb">Visual Studio Code &nbsp;›&nbsp; Programming Languages &nbsp;›&nbsp; Sema</div>
            <div class="vsm-body">
              <div v-html="tile(candidate, 128)"></div>
              <div class="vsm-meta">
                <div class="vsm-name">Sema</div>
                <div class="vsm-pub">
                  <a class="pub">Helge Sverre</a> <span class="verified">✔</span>
                  <a class="link">sema-lang.com</a>
                  <span class="dim">| ⭳ 0 installs | ★★★★★ | Free</span>
                </div>
                <div class="vsm-desc">Language support for Sema, a Lisp dialect with first-class LLM primitives</div>
                <button class="vsm-install">Install</button>
              </div>
            </div>
          </div>
        </div>

        <div class="panel">
          <span class="panel-label">Open VSX Registry · detail <em>dark theme</em></span>
          <div class="replica ovsx">
            <div class="ovsx-top"><span class="ovsx-logo">◭ Open VSX Registry</span></div>
            <div class="ovsx-body">
              <div v-html="tile(candidate, 120)"></div>
              <div class="ovsx-meta">
                <div class="ovsx-name">Sema</div>
                <div class="ovsx-pub">helgesverre &nbsp;|&nbsp; Published by helgesverre &nbsp;|&nbsp; MIT</div>
                <div class="ovsx-desc">Language support for Sema, a Lisp dialect with first-class LLM primitives</div>
                <div class="ovsx-stats">⭳ 0 downloads &nbsp;|&nbsp; ★★★★★</div>
                <button class="ovsx-install">Download</button>
              </div>
            </div>
          </div>
        </div>

        <div class="panel">
          <span class="panel-label">JetBrains Marketplace · detail</span>
          <div class="replica jb">
            <div class="jb-top"><span class="jb-logo">▧ JETBRAINS</span> Marketplace</div>
            <div class="jb-body">
              <div v-html="tile(candidate, 80)"></div>
              <div class="jb-meta">
                <div class="jb-name">Sema</div>
                <div class="jb-sub"><span class="star">★</span> 0.0 &nbsp; Helge Sverre <span class="verified">✔</span></div>
              </div>
              <button class="jb-get">Get</button>
            </div>
            <div class="jb-tabs"><span class="on">Overview</span><span>Versions</span><span>Reviews</span></div>
          </div>
        </div>

        <div class="panel-row">
          <div class="panel">
            <span class="panel-label">JetBrains · search card · 72px</span>
            <div class="jb-card">
              <div class="jb-card-head">
                <div v-html="tile(candidate, 72)"></div>
                <div>
                  <div class="jb-card-name">Sema</div>
                  <div class="jb-card-stars">★★★★★</div>
                  <div class="jb-card-author">Helge Sverre</div>
                </div>
              </div>
              <div class="jb-card-desc">Language support for Sema, a Lisp dialect with first-class LLM primitives. LSP, DAP and notebooks.</div>
              <div class="jb-card-foot"><span>0 downloads</span><span>Free</span></div>
            </div>
          </div>

          <div class="panel">
            <span class="panel-label">In-IDE plugin list · 40px</span>
            <div class="ide-list dark">
              <div class="ide-row"><div v-html="tile(candidate, 40)"></div><div class="ide-txt"><div class="ide-name">Sema</div><div class="ide-desc">Lisp with LLM primitives · LSP, DAP, notebooks</div></div><button class="ide-btn">Install</button></div>
            </div>
            <div class="ide-list light">
              <div class="ide-row"><div v-html="tile(candidate, 40)"></div><div class="ide-txt"><div class="ide-name">Sema</div><div class="ide-desc">Lisp with LLM primitives · LSP, DAP, notebooks</div></div><button class="ide-btn">Install</button></div>
            </div>
          </div>
        </div>
      </section>

      <!-- ─────────────  FILE ICONS  ───────────── -->
      <section class="sec last">
        <div class="sec-head">
          <span class="sec-num">03</span>
          <h2 class="sec-title">File icons in the editor tree</h2>
          <p class="sec-desc">
            The transparent <code>.sema</code> / <code>.semac</code> / <code>.sema-nb</code> icons as
            they render in a file explorer, on both themes (~16px rows).
          </p>
        </div>
        <div class="tree-grid">
          <div class="panel">
            <span class="panel-label">Dark theme</span>
            <div class="tree dark">
              <div class="tree-row" v-for="r in fileRows" :key="r.name">
                <span class="tree-ic" v-html="r.dark"></span><span>{{ r.name }}</span>
              </div>
            </div>
          </div>
          <div class="panel">
            <span class="panel-label">Light theme</span>
            <div class="tree light">
              <div class="tree-row" v-for="r in fileRows" :key="r.name">
                <span class="tree-ic" v-html="r.light"></span><span>{{ r.name }}</span>
              </div>
            </div>
          </div>
        </div>
        <p class="footnote">Zed has no per-extension file-icon or marketplace-icon slot, so nothing ships there.</p>
      </section>
    </div>
  </div>
</template>

<style scoped>
.icon-showcase {
  --bg: #131110;
  --bg-elevated: #181512;
  --border: #2b2620;
  --gold: #c8a855;
  --text-primary: #e9e3d6;
  --text-secondary: #968c79;
  --text-tertiary: #6b6354;
  background: var(--bg);
  color: var(--text-secondary);
  font-family: 'Inter', system-ui, -apple-system, sans-serif;
  min-height: 100vh;
  -webkit-font-smoothing: antialiased;
}
.wrap { max-width: 1100px; margin: 0 auto; padding: 0 2rem 7rem; }

/* hero */
.hero { padding: 6rem 0 4rem; }
.eyebrow {
  font-family: 'JetBrains Mono', monospace; font-size: 0.75rem; letter-spacing: 0.08em;
  text-transform: uppercase; color: var(--gold); opacity: 0.85; display: block; margin-bottom: 1.1rem;
}
.hero h1 { font-family: 'Cormorant', Georgia, serif; font-weight: 300; font-size: clamp(2.6rem, 5vw, 4rem);
  color: var(--text-primary); line-height: 1.05; margin: 0 0 1.4rem; }
.hero-desc { font-size: 1.1rem; line-height: 1.7; color: var(--text-secondary); max-width: 46rem; }

/* sections */
.sec { padding-bottom: 6rem; margin-bottom: 4rem; border-bottom: 1px solid var(--border); }
.sec.last { border-bottom: none; margin-bottom: 0; }
.sec-head { margin-bottom: 2.75rem; }
.sec-num { font-family: 'JetBrains Mono', monospace; font-size: 0.8rem; color: var(--gold); opacity: 0.8;
  display: block; margin-bottom: 0.6rem; }
.sec-title { font-family: 'Cormorant', Georgia, serif; font-weight: 300; font-size: 2.2rem;
  color: var(--text-primary); margin: 0 0 0.9rem; }
.sec-desc { font-size: 1.05rem; line-height: 1.65; color: var(--text-secondary); max-width: 48rem; }
code { font-family: 'JetBrains Mono', monospace; background: rgba(200,168,85,0.08); color: var(--gold);
  padding: 0.1em 0.4em; border-radius: 4px; font-size: 0.85em; }

/* svg fill helpers */
:deep(.tile-svg svg) { width: 100%; height: 100%; display: block; }
:deep(.logo-tile) { background: #1a1a1a; display: flex; align-items: center; justify-content: center; box-sizing: border-box; }
:deep(.logo-tile svg) { width: 100%; height: auto; display: block; }

/* candidate grid */
.cand-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 1.75rem; }
.cand-label { font-family: 'JetBrains Mono', monospace; font-size: 0.8rem; color: var(--text-primary);
  display: block; margin-bottom: 1rem; }
.swatch { display: flex; align-items: flex-end; gap: 1.5rem; flex-wrap: wrap;
  padding: 1.5rem 1.75rem; border-radius: 10px; margin-bottom: 0.9rem; }
.swatch.light { background: #f3f3f3; }
.swatch.dark { background: #121212; border: 1px solid var(--border); }
.sample { display: flex; flex-direction: column; align-items: center; gap: 0.55rem; }
.px { font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: #8a8577; }
.swatch.light .px { color: #999; }
.legend { list-style: none; padding: 0; margin: 1.5rem 0 0; display: flex; flex-wrap: wrap; gap: 1.5rem;
  font-size: 0.8rem; color: var(--text-tertiary); }
.legend b { color: var(--text-secondary); font-weight: 600; }
.chosen { font-style: normal; font-family: 'JetBrains Mono', monospace; font-size: 0.68rem;
  color: #131110; background: var(--gold); padding: 0.1em 0.5em; border-radius: 20px; margin-left: 0.5rem; }

/* background-shape variants */
.variants { margin-top: 3rem; padding-top: 2rem; border-top: 1px dashed var(--border); }
.variants-desc { font-size: 0.95rem; line-height: 1.6; color: var(--text-secondary); max-width: 46rem; margin: 0.4rem 0 1.75rem; }
.variants-desc b { color: var(--text-primary); font-weight: 600; }
.variant-row { display: flex; gap: 2.5rem; flex-wrap: wrap; }
.variant { display: flex; flex-direction: column; align-items: center; gap: 0.6rem; }
.variant-box { width: 84px; height: 84px; }
:deep(.variant-box svg) { width: 100%; height: 100%; display: block; }
.variant-box.circle { border-radius: 50%; overflow: hidden; }
.v-name { font-size: 0.85rem; color: var(--text-primary); }
.v-file { font-size: 0.72rem; color: var(--text-tertiary); } .v-file code { font-size: 0.9em; }

/* toggle */
.toggle { display: flex; align-items: center; gap: 0.6rem; margin-bottom: 2rem; }
.toggle-label { font-family: 'JetBrains Mono', monospace; font-size: 0.7rem; text-transform: uppercase;
  letter-spacing: 0.08em; color: var(--text-tertiary); margin-right: 0.3rem; }
.toggle button { font-family: 'JetBrains Mono', monospace; font-size: 0.8rem; background: var(--bg-elevated);
  border: 1px solid var(--border); color: var(--text-secondary); padding: 0.45rem 0.9rem; border-radius: 7px; cursor: pointer; transition: all .15s; }
.toggle button:hover { border-color: var(--gold); color: var(--text-primary); }
.toggle button.on { background: var(--gold); color: #131110; border-color: var(--gold); font-weight: 600; }

/* panels wrapping each replica */
.panel { margin-bottom: 2.25rem; }
.panel-label { font-family: 'JetBrains Mono', monospace; font-size: 0.72rem; letter-spacing: 0.05em;
  text-transform: uppercase; color: var(--text-tertiary); display: block; margin-bottom: 0.85rem; }
.panel-label em { font-style: normal; color: var(--gold); opacity: 0.75; margin-left: 0.4rem; }
.panel-row { display: grid; grid-template-columns: 1fr 1fr; gap: 1.75rem; align-items: start; }
.replica { border-radius: 10px; overflow: hidden; border: 1px solid var(--border); }

/* VS Code Marketplace (light) */
.vsm { background: #fff; color: #000; }
.vsm-top { background: #1e1e1e; color: #fff; padding: 0.6rem 1.2rem; font-size: 0.85rem; }
.vsm-logo { font-weight: 600; } .vsm-sep { color: #888; margin: 0 0.4rem; }
.vsm-crumb { background: #2d2d2d; color: #ccc; padding: 0.4rem 1.2rem; font-size: 0.78rem; }
.vsm-body { display: flex; gap: 1.75rem; padding: 1.9rem 1.4rem; align-items: flex-start; }
.vsm-name { font-size: 1.7rem; font-weight: 600; }
.vsm-pub { font-size: 0.9rem; margin: 0.4rem 0; color: #000; }
.vsm-pub .pub { font-weight: 700; text-decoration: underline; color: #000; }
.vsm-pub .link { color: #006ab1; margin-left: 0.3rem; }
.vsm-pub .verified { color: #006ab1; } .vsm-pub .dim { color: #444; margin-left: 0.4rem; }
.vsm-desc { color: #333; margin: 0.55rem 0 1.1rem; font-size: 0.95rem; }
.vsm-install { background: #107C10; color: #fff; border: 0; padding: 0.5rem 2.2rem; font-size: 0.95rem; cursor: default; }

/* Open VSX (dark) */
.ovsx { background: #121212; color: #fff; }
.ovsx-top { background: #1c1c1c; padding: 0.75rem 1.2rem; font-weight: 600; }
.ovsx-body { display: flex; gap: 1.75rem; padding: 1.9rem 1.4rem; align-items: flex-start; }
.ovsx-name { font-size: 2rem; font-weight: 700; }
.ovsx-pub { color: #bbb; font-size: 0.9rem; margin: 0.35rem 0; }
.ovsx-desc { color: #ddd; margin: 0.35rem 0; }
.ovsx-stats { color: #999; font-size: 0.85rem; margin: 0.55rem 0 1.1rem; }
.ovsx-install { background: #C160EF; color: #edf5ea; border: 0; border-radius: 4px; padding: 0.55rem 1.7rem; cursor: default; }

/* JetBrains (light) */
.jb { background: #fff; color: #000; }
.jb-top { background: #27282c; color: #fff; padding: 0.75rem 1.2rem; font-size: 0.9rem; }
.jb-logo { font-weight: 700; margin-right: 0.3rem; }
.jb-body { display: flex; gap: 1.4rem; padding: 1.6rem 1.4rem 0.6rem; align-items: center; }
.jb-name { font-size: 2rem; font-weight: 600; }
.jb-sub { color: #444; font-size: 0.9rem; margin-top: 0.25rem; } .jb-sub .star { color: #f5b400; } .jb-sub .verified { color: #167DFF; }
.jb-get { margin-left: auto; background: #167DFF; color: #fff; border: 0; border-radius: 20px; height: 40px; padding: 0 2rem; cursor: default; }
.jb-tabs { display: flex; gap: 1.5rem; padding: 0.6rem 1.4rem; border-top: 1px solid #eee; color: #555; font-size: 0.9rem; }
.jb-tabs .on { color: #167DFF; border-bottom: 2px solid #167DFF; padding-bottom: 0.4rem; }

/* JetBrains search card (light) */
.jb-card { background: #fff; border: 1px solid #e0e0e0; border-radius: 8px; padding: 1.25rem; color: #000; }
.jb-card-head { display: flex; gap: 1rem; align-items: flex-start; }
.jb-card-name { font-size: 1.3rem; font-weight: 700; }
.jb-card-stars { color: #f5b400; font-size: 0.85rem; }
.jb-card-author { color: #777; font-size: 0.82rem; }
.jb-card-desc { color: #333; font-size: 0.9rem; margin: 0.9rem 0; line-height: 1.45; }
.jb-card-foot { display: flex; justify-content: space-between; color: #888; font-size: 0.82rem; }

/* in-IDE list rows */
.ide-list { border-radius: 8px; padding: 0.4rem; margin-bottom: 0.9rem; }
.ide-list.dark { background: #2b2b2b; } .ide-list.light { background: #f5f5f5; }
.ide-row { display: flex; align-items: center; gap: 0.9rem; padding: 0.55rem 0.6rem; }
.ide-list.dark .ide-name { color: #eee; } .ide-list.dark .ide-desc { color: #999; }
.ide-list.light .ide-name { color: #111; } .ide-list.light .ide-desc { color: #666; }
.ide-txt { min-width: 0; } .ide-name { font-weight: 600; font-size: 0.95rem; } .ide-desc { font-size: 0.8rem; }
.ide-btn { margin-left: auto; background: #167DFF; color: #fff; border: 0; border-radius: 4px; padding: 0.3rem 1rem; cursor: default; font-size: 0.82rem; }

/* file trees */
.tree-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 1.75rem; }
.tree { border-radius: 8px; padding: 0.85rem; font-family: 'JetBrains Mono', ui-monospace, monospace; font-size: 0.85rem; }
.tree.dark { background: #1e1e1e; color: #ccc; } .tree.light { background: #fff; color: #333; border: 1px solid #e0e0e0; }
.tree-row { display: flex; align-items: center; gap: 0.55rem; padding: 0.25rem 0.35rem; }
:deep(.tree-ic) { width: 16px; height: 16px; display: inline-flex; flex: none; }
:deep(.tree-ic svg) { width: 16px; height: 16px; display: block; }

.footnote { font-size: 0.8rem; color: var(--text-tertiary); margin-top: 1.75rem; }

@media (max-width: 820px) {
  .cand-grid, .panel-row, .tree-grid { grid-template-columns: 1fr; }
}
</style>
