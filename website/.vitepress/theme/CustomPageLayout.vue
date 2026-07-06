<script setup>
import { ref, computed } from 'vue'
import HomeSearch from './HomeSearch.vue'
import SemaLogo from './SemaLogo.vue'

const props = defineProps({
  activeNav: { type: String, default: '' }
})

const menuOpen = ref(false)
const toggleMenu = () => { menuOpen.value = !menuOpen.value }
const closeMenu = () => { menuOpen.value = false }

const docsItems = [
  { label: 'Guide', link: '/docs/', key: 'guide' },
  { label: 'Quickstart', link: '/docs/quickstart', key: 'quickstart' },
  { label: 'Standard Library', link: '/docs/stdlib/', key: 'stdlib' },
  { label: 'LLM & Agents', link: '/docs/llm/', key: 'llm' },
  { label: 'Internals', link: '/docs/internals/architecture', key: 'internals' },
]

const featureItems = [
  { label: 'Notebook', link: '/feature/notebook', key: 'notebook' },
  { label: 'Agents & Tools', link: '/feature/agents', key: 'agents' },
  { label: 'Cassettes', link: '/feature/cassettes', key: 'cassettes' },
  { label: 'Observability', link: '/feature/observability', key: 'observability' },
  { label: 'Standalone Executables', link: '/feature/build', key: 'build' },
  { label: 'Structured Extraction', link: '/feature/extraction', key: 'extraction' },
  { label: 'Embedding', link: '/feature/embed', key: 'embed' },
  { label: 'RAG', link: '/feature/rag', key: 'rag' },
  { label: 'Workflows', link: '/feature/workflows', key: 'workflows' },
]

const docsActive = computed(() => docsItems.some(i => i.key === props.activeNav))
const featuresActive = computed(() => featureItems.some(i => i.key === props.activeNav))

const copyText = (id, event) => {
  const el = document.getElementById(id);
  if (el) {
    navigator.clipboard.writeText(el.textContent.trim()).then(() => {
      const btn = event.currentTarget;
      const originalText = btn.textContent;
      btn.textContent = 'copied';
      setTimeout(() => { btn.textContent = originalText; }, 1400);
    });
  }
};
</script>

<template>
  <div class="custom-home">

    <nav>
      <div class="wrap nav-in">
        <a href="/" class="logo-link" aria-label="Sema home">
          <SemaLogo class="logo-svg" height="20px" />
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
          <a href="/what-is-sema" :class="{ 'nav-active': activeNav === 'what-is-sema' }">What is Sema?</a>

          <div class="nav-dropdown" :class="{ 'dd-active': featuresActive }">
            <a href="/feature/notebook" class="dd-label">Features <svg class="dd-caret" width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true"><path d="M2 3.5L5 6.5L8 3.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg></a>
            <div class="dd-menu">
              <a v-for="item in featureItems" :key="item.key" :href="item.link"
                 :class="{ 'nav-active': activeNav === item.key }">{{ item.label }}</a>
            </div>
          </div>

          <div class="nav-dropdown" :class="{ 'dd-active': docsActive }">
            <a href="/docs/" class="dd-label">Docs <svg class="dd-caret" width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true"><path d="M2 3.5L5 6.5L8 3.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg></a>
            <div class="dd-menu">
              <a v-for="item in docsItems" :key="item.key" :href="item.link"
                 :class="{ 'nav-active': activeNav === item.key }">{{ item.label }}</a>
            </div>
          </div>

          <a href="https://sema.run" target="_blank" rel="noopener" class="vp-external-link-icon">
            Playground
          </a>
          <a class="nav-gh" href="https://github.com/sema-lisp/sema" aria-label="GitHub" target="_blank"
             rel="noopener">
            <svg viewBox="0 0 16 16" class="gh-svg" aria-hidden="true">
              <path fill="currentColor"
                    d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" />
            </svg>
          </a>
        </div>
      </div>
    </nav>

    <main>
      <slot :copy-text="copyText" />
    </main>

    <footer>
      <div class="wrap foot-in">
        <span><span style="color:var(--gold)">(</span>sema<span style="color:var(--gold)">)</span></span>
        <span>
          <a href="/docs/">Docs</a> ·
          <a href="/docs/internals/lisp-comparison">Benchmarks</a> ·
          <a href="https://github.com/sema-lisp/sema/blob/main/CHANGELOG.md">Changelog</a> ·
          <a href="https://github.com/sema-lisp/sema">GitHub</a> ·
          <a href="/brand">Brand</a> ·
          <a href="/llms.txt">llms.txt</a>
        </span>
      </div>
    </footer>

    <HomeSearch />

  </div>
</template>

<style>
/* ============================================================
   CustomPageLayout — shared CSS for all custom (layout: false)
   pages.  Non-scoped so styles cascade into <slot/> content.
   Every selector is prefixed with .custom-home (which only
   exists on these pages), so there is no leak into the docs.
   ============================================================ */

.custom-home {
  --gold: #c8a855;
  --gold-bright: #e3c878;
  --gold-fade: rgba(200, 168, 85, .09);
  --gold-line: rgba(200, 168, 85, .28);

  --bg: #131110;
  --bg-raise: #181512;
  --surface: #1c1916;
  --bg-editor: #1c1916;
  --bg-output: #0f0d0c;
  --border: #2b2620;
  --border-lo: #221e19;
  --text: #e9e3d6;
  --muted: #968c79;
  --dim: #6b6354;

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

/* ---------- resets ---------- */
.custom-home * { box-sizing: border-box; }
.custom-home a { color: var(--gold-bright); text-decoration: none; }
.custom-home a:hover { text-decoration: underline; text-underline-offset: 3px; }
.custom-home h1, .custom-home h2, .custom-home h3 { border: none; margin: 0; }
.custom-home p { margin: 0; }
.custom-home pre { margin: 0; padding: 0; background: none; }
.custom-home code { background: none; border-radius: 0; padding: 0; font-size: inherit; color: inherit; }
.custom-home ul { list-style: none; margin: 0; padding: 0; }

/* ---------- layout primitives ---------- */
.custom-home .wrap { max-width: var(--w); margin: 0 auto; padding: 0 28px; }
.custom-home :focus-visible { outline: 2px solid var(--gold); outline-offset: 3px; border-radius: 2px; }

/* ---------- nav ---------- */
.custom-home nav {
  position: sticky; top: 0; z-index: 50;
  background: rgba(19, 17, 16, 0.86);
  backdrop-filter: blur(10px);
  border-bottom: 1px solid var(--border-lo);
}
.custom-home .nav-in { display: flex; align-items: center; gap: 26px; height: 58px; }
.custom-home .logo-link { display: flex; align-items: center; text-decoration: none !important; }
.custom-home .logo-svg { height: 20px; transition: transform 0.2s ease; color: var(--text); }
.custom-home .logo-link:hover .logo-svg { transform: scale(1.04); }
.custom-home .nav-links { display: flex; align-items: center; gap: 22px; margin-left: auto; font-size: 13.5px; }
.custom-home .nav-links > a { color: var(--muted); }
.custom-home .nav-links > a:hover { color: var(--text); text-decoration: none; }
.custom-home .nav-links a.nav-active { color: var(--gold-bright); }

/* dropdowns */
.custom-home .nav-dropdown { position: relative; }
.custom-home .dd-label {
  color: var(--muted) !important;
  cursor: default;
  display: flex;
  align-items: center;
  gap: 4px;
  text-decoration: none !important;
}
.custom-home .dd-caret { font-size: 9px; opacity: 0.5; margin-left: 2px; }
.custom-home .nav-dropdown:hover .dd-label,
.custom-home .nav-dropdown.dd-active .dd-label { color: var(--text) !important; }
.custom-home .nav-dropdown.dd-active .dd-label { color: var(--gold-bright) !important; }

.custom-home .dd-menu {
  position: absolute;
  top: 100%;
  left: -12px;
  min-width: 200px;
  padding: 6px 0;
  background: rgba(24, 21, 18, 0.98);
  backdrop-filter: blur(14px);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: 0 12px 40px -12px rgba(0, 0, 0, .5);
  opacity: 0;
  visibility: hidden;
  transform: translateY(-6px);
  transition: opacity .15s var(--ease), transform .15s var(--ease), visibility .15s;
  z-index: 60;
}
.custom-home .nav-dropdown:hover .dd-menu {
  opacity: 1;
  visibility: visible;
  transform: translateY(0);
}
.custom-home .dd-menu a {
  display: block;
  padding: 8px 16px;
  color: var(--muted) !important;
  font-size: 13px;
  transition: color .12s, background .12s;
}
.custom-home .dd-menu a:hover {
  color: var(--text) !important;
  background: rgba(200, 168, 85, .06);
  text-decoration: none !important;
}
.custom-home .dd-menu a.nav-active { color: var(--gold-bright) !important; }
.custom-home .nav-gh { display: flex; align-items: center; color: var(--muted) !important; transition: color 0.18s var(--ease); }
.custom-home .nav-gh:hover { color: var(--gold-bright) !important; text-decoration: none; }
.custom-home .nav-gh .gh-svg { width: 18px; height: 18px; }

.custom-home .nav-toggle {
  display: none; margin-left: auto;
  flex-direction: column; justify-content: center; gap: 4px;
  width: 36px; height: 34px; padding: 0;
  background: none; border: 1px solid var(--border); border-radius: 6px; cursor: pointer;
}
.custom-home .nav-toggle span { display: block; width: 17px; height: 1.5px; margin: 0 auto; background: var(--text); transition: transform .2s var(--ease), opacity .2s var(--ease); }
.custom-home .nav-toggle.open span:nth-child(1) { transform: translateY(5.5px) rotate(45deg); }
.custom-home .nav-toggle.open span:nth-child(2) { opacity: 0; }
.custom-home .nav-toggle.open span:nth-child(3) { transform: translateY(-5.5px) rotate(-45deg); }

/* ---------- section scaffolding ---------- */
.custom-home section { padding: 88px 0; border-top: 1px solid var(--border-lo); }

/* ---------- typography ---------- */
.custom-home .eyebrow { font-family: var(--font-mono); font-size: 12px; letter-spacing: .14em; text-transform: uppercase; color: var(--gold); margin-bottom: 26px; }
.custom-home .eyebrow .sep { color: var(--dim); margin: 0 8px; }
.custom-home h1 { font-family: var(--font-display); font-weight: 400; font-size: clamp(42px, 6.4vw, 76px); line-height: 1.04; letter-spacing: 0; max-width: 14ch; margin-bottom: 28px; }
.custom-home h1 em { font-style: italic; color: var(--gold-bright); }
.custom-home .lede { font-size: 18.5px; line-height: 1.65; color: var(--muted); max-width: 62ch; margin-bottom: 40px; }
.custom-home .lede strong { color: var(--text); font-weight: 500; }
.custom-home .lede code, .custom-home .req code, .custom-home .sub code {
  font-family: var(--font-mono);
  font-size: 0.88em;
  color: var(--gold-bright);
  background: var(--gold-fade);
  padding: 1px 6px;
  border-radius: 4px;
  white-space: nowrap;
}
.custom-home .kicker { font-family: var(--font-mono); font-size: 12px; letter-spacing: .14em; text-transform: uppercase; color: var(--gold); margin-bottom: 14px; }
.custom-home h2 { font-family: var(--font-display); font-weight: 400; font-size: clamp(30px, 3.6vw, 42px); line-height: 1.12; letter-spacing: 0; margin-bottom: 16px; max-width: 24ch; }
.custom-home .sub { color: var(--muted); max-width: 62ch; font-size: 16.5px; }
.custom-home .req { font-family: var(--font-mono); font-size: 12px; color: var(--dim); }

/* ---------- hero (base — pages override padding) ---------- */
.custom-home .hero { position: relative; overflow: hidden; }
.custom-home .hero-paren {
  position: absolute; top: -60px;
  font-family: var(--font-display); font-weight: 300; font-style: italic;
  font-size: 560px; line-height: 1; color: var(--gold);
  opacity: .05; user-select: none; pointer-events: none;
}
.custom-home .hero-paren.l { left: -70px; }
.custom-home .hero-paren.r { right: -70px; top: auto; bottom: -180px; }

/* ---------- buttons & install ---------- */
.custom-home .hero-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 14px; margin-bottom: 22px; }
.custom-home .btn { display: inline-block; font-size: 14.5px; font-weight: 500; padding: 11px 22px; border-radius: 8px; transition: all .18s var(--ease); }
.custom-home .btn-gold { background: var(--gold); color: #171410; }
.custom-home .btn-gold:hover { background: var(--gold-bright); text-decoration: none; }
.custom-home .btn-ghost { color: var(--text); border: 1px solid var(--border); }
.custom-home .btn-ghost:hover { border-color: var(--gold-line); text-decoration: none; }

.custom-home .install { display: inline-flex; align-items: center; gap: 14px; font-family: var(--font-mono); font-size: 13.5px; background: var(--bg-raise); border: 1px solid var(--border); border-radius: 8px; padding: 11px 16px; color: var(--text); max-width: 100%; }
.custom-home .install .dollar { color: var(--gold); user-select: none; }
.custom-home .install .cm { color: var(--dim); }
.custom-home .copy { font-family: var(--font-mono); font-size: 11px; color: var(--dim); background: none; border: 1px solid var(--border); border-radius: 5px; padding: 3px 9px; cursor: pointer; transition: all .15s; flex-shrink: 0; }
.custom-home .copy:hover { color: var(--gold-bright); border-color: var(--gold-line); }

/* ---------- CTA ---------- */
.custom-home .cta { text-align: center; padding: 110px 0; }
.custom-home .cta h2 { margin: 0 auto 18px; }
.custom-home .cta .sub { margin: 0 auto 36px; }
.custom-home .install-stack { max-width: 660px; margin: 0 auto; display: flex; flex-direction: column; gap: 12px; }
.custom-home .install-row { display: flex; align-items: center; gap: 16px; }
.custom-home .install-row .badge { width: 54px; text-align: right; font-family: var(--font-mono); font-size: 12px; color: var(--gold-bright); font-weight: 500; text-transform: uppercase; letter-spacing: 0.05em; flex-shrink: 0; }
.custom-home .install-row .badge.agent { color: var(--muted); }
.custom-home .install-row .install { flex-grow: 1; display: flex; justify-content: space-between; align-items: center; }

.custom-home .cmd-text { display: flex; align-items: center; gap: 14px; text-align: left; white-space: nowrap; overflow-x: auto; min-width: 0; flex: 1 1 auto; scrollbar-width: thin; }
.custom-home .cmd-text::-webkit-scrollbar { height: 8px; }
.custom-home .cmd-text::-webkit-scrollbar-track { background: transparent; }
.custom-home .cmd-text::-webkit-scrollbar-thumb { background: var(--border); border-radius: 8px; }
.custom-home .cmd-text::-webkit-scrollbar-thumb:hover { background: var(--gold-line); }

/* ---------- terminal mockup ---------- */
.custom-home .term { background: var(--bg-raise); border: 1px solid var(--border); border-radius: 12px; font-family: var(--font-mono); font-size: 12.5px; line-height: 1.85; padding: 20px 22px; color: #c9c2b4; overflow-x: auto; }
.custom-home .term .head { color: var(--dim); padding-bottom: 10px; margin-bottom: 12px; border-bottom: 1px solid var(--border-lo); font-size: 11.5px; }
.custom-home .term .dollar { color: var(--gold); }
.custom-home .term .dot { color: var(--gold); }
.custom-home .term .out { color: var(--dim); }
.custom-home .term .err { color: #c97b6a; }
.custom-home .term .ok { color: #9bb87a; }

/* ---------- syntax token colors ---------- */
.custom-home .c-kw { color: var(--gold-bright); }
.custom-home .c-str { color: #a8b88a; }
.custom-home .c-kwd { color: #b8a3d6; }
.custom-home .c-com { color: #665e50; font-style: italic; }
.custom-home .c-fn { color: #d6c8a8; }

/* ---------- footer ---------- */
.custom-home footer { border-top: 1px solid var(--border-lo); padding: 34px 0; }
.custom-home .foot-in { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 14px; font-size: 13px; color: var(--dim); }
.custom-home .foot-in a { color: var(--muted); }

/* ---------- responsive nav ---------- */
@media (max-width: 880px) {
  .custom-home .nav-toggle { display: flex; }
  .custom-home .nav-links {
    position: absolute; top: 100%; left: 0; right: 0;
    margin-left: 0;
    flex-direction: column; align-items: stretch; gap: 0;
    padding: 6px 0;
    background: rgba(19, 17, 16, 0.97);
    backdrop-filter: blur(10px);
    border-bottom: 1px solid var(--border-lo);
    display: none;
  }
  .custom-home .nav-links.open { display: flex; }

  /* dropdowns become flat inline sections on mobile */
  .custom-home .nav-dropdown { position: static; }
  .custom-home .dd-label {
    padding: 11px 22px;
    cursor: default;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--dim) !important;
  }
  .custom-home .dd-caret { display: none; }  .custom-home .dd-menu {
    position: static;
    opacity: 1;
    visibility: visible;
    transform: none;
    background: none;
    border: none;
    box-shadow: none;
    padding: 0;
    min-width: 0;
  }
  .custom-home .dd-menu a { padding: 11px 22px 11px 34px; }

  .custom-home .nav-links > a { padding: 11px 22px; }
  .custom-home .nav-gh { padding: 11px 22px; }
  .custom-home .install-row { flex-direction: column; align-items: stretch; gap: 7px; }
  .custom-home .install-row .badge { width: auto; text-align: left; }
  .custom-home section { padding: 64px 0; }
  .custom-home .hero-paren { display: none; }
}

@media (prefers-reduced-motion: reduce) {
  html { scroll-behavior: auto; }
}
</style>
