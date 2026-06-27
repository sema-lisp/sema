<script setup>
import { ref, computed } from 'vue'
import HomeSearch from './HomeSearch.vue'

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
          <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 366.00 132.00" class="logo-svg" role="img" aria-label="Sema logo">
            <path
              d="M48.5000 104.3000L48.5000 114Q34 110.7000 26.0500 100.5000Q18.1000 90.3000 18.1000 75L18.1000 57Q18.1000 41.7000 26.0500 31.5000Q34 21.3000 48.5000 18L48.5000 27.6000Q42.2000 29.1000 37.6000 33.1500Q33 37.2000 30.5000 43.3000Q28 49.4000 28 57L28 75Q28 82.6000 30.5000 88.6500Q33 94.7000 37.6000 98.7500Q42.2000 102.8000 48.5000 104.3000"
              fill="#c8a855" />
            <path
              d="M93.2000 102.8000L88.8000 102.8000Q79.4000 102.8000 74.2000 98.6000Q69 94.4000 69 86.8000L78.8000 86.8000Q78.8000 90.4000 81.4500 92.4500Q84.1000 94.5000 88.8000 94.5000L93.2000 94.5000Q98.1000 94.5000 100.7500 92.4000Q103.4000 90.3000 103.4000 86.5000Q103.4000 79.8000 96.8000 79L82 76.9000Q76.1000 76 72.9000 72.0500Q69.7000 68.1000 69.7000 61.8000Q69.7000 54.4000 74.7000 50.3000Q79.7000 46.2000 88.7000 46.2000L93.1000 46.2000Q101.5000 46.2000 106.7000 50.2000Q111.9000 54.2000 112.2000 60.8000L102.2000 60.8000Q102 58 99.6000 56.1500Q97.2000 54.3000 93.1000 54.3000L88.7000 54.3000Q84.2000 54.3000 81.7500 56.3000Q79.3000 58.3000 79.3000 61.7000Q79.3000 67.2000 84.8000 67.9000L98.7000 69.9000Q113 71.8000 113 86.5000Q113 94.3000 107.8500 98.5500Q102.7000 102.8000 93.2000 102.8000 M152 103Q142.1000 103 136.0500 97.1000Q130 91.2000 130 81L130 68Q130 57.8000 136.0500 51.9000Q142.1000 46 152 46Q158.6000 46 163.5500 48.6500Q168.5000 51.3000 171.2500 56.0500Q174 60.8000 174 67.1000L174 77L139.7000 77L139.7000 81.8000Q139.7000 87.8000 143 91.2000Q146.3000 94.6000 152 94.6000Q156.8000 94.6000 159.9000 92.8000Q163 91 163.6000 87.8000L173.5000 87.8000Q172.5000 94.8000 166.6000 98.9000Q160.7000 103 152 103M139.7000 67.1000L139.7000 69.7000L164.3000 69.7000L164.3000 67.1000Q164.3000 60.8000 161.1000 57.4000Q157.9000 54 152 54Q146.1000 54 142.9000 57.4000Q139.7000 60.8000 139.7000 67.1000 M197.7000 102L188.7000 102L188.7000 47L197.1000 47L197.1000 54.5000L197.4000 54.5000Q197.8000 50.7000 200.2500 48.3500Q202.7000 46 206.5000 46Q210.2000 46 212.7000 48.2000Q215.2000 50.4000 216.3000 54.1000Q216.9000 50.3000 219.4000 48.1500Q221.9000 46 225.7000 46Q230.9000 46 234.1000 49.9500Q237.3000 53.9000 237.3000 60.2000L237.3000 102L228.3000 102L228.3000 60.3000Q228.3000 57.2000 226.7500 55.3500Q225.2000 53.5000 222.6000 53.5000Q220 53.5000 218.5000 55.3000Q217 57.1000 217 60.3000L217 102L209 102L209 60.3000Q209 57.2000 207.5000 55.3500Q206 53.5000 203.4000 53.5000Q200.8000 53.5000 199.2500 55.3000Q197.7000 57.1000 197.7000 60.3000 M268.9000 103Q260.4000 103 255.4500 98.2500Q250.5000 93.5000 250.5000 85.8000Q250.5000 78.1000 255.6500 73.4000Q260.8000 68.7000 269.2000 68.7000L285.3000 68.7000L285.3000 64.5000Q285.3000 54.4000 274.1000 54.4000Q269.1000 54.4000 266.0500 56.2500Q263 58.1000 262.8000 61.4000L253 61.4000Q253.5000 54.7000 259.1000 50.3500Q264.7000 46 274.1000 46Q284.2000 46 289.7000 50.8000Q295.2000 55.6000 295.2000 64.3000L295.2000 102L285.5000 102L285.5000 91.9000L285.3000 91.9000Q284.6000 97 280.2500 100Q275.9000 103 268.9000 103M271.5000 94.7000Q277.8000 94.7000 281.5500 91.6500Q285.3000 88.6000 285.3000 83.3000L285.3000 76L270.1000 76Q265.8000 76 263.1500 78.5500Q260.5000 81.1000 260.5000 85.3000Q260.5000 89.6000 263.4000 92.1500Q266.3000 94.7000 271.5000 94.7000"
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
          <a href="/what-is-sema" :class="{ 'nav-active': activeNav === 'what-is-sema' }">What is Sema?</a>

          <div class="nav-dropdown" :class="{ 'dd-active': docsActive }">
            <a href="/docs/" class="dd-label">Docs <span class="dd-caret">&#x25be;</span></a>
            <div class="dd-menu">
              <a v-for="item in docsItems" :key="item.key" :href="item.link"
                 :class="{ 'nav-active': activeNav === item.key }">{{ item.label }}</a>
            </div>
          </div>

          <div class="nav-dropdown" :class="{ 'dd-active': featuresActive }">
            <a href="/feature/notebook" class="dd-label">Features <span class="dd-caret">&#x25be;</span></a>
            <div class="dd-menu">
              <a v-for="item in featureItems" :key="item.key" :href="item.link"
                 :class="{ 'nav-active': activeNav === item.key }">{{ item.label }}</a>
            </div>
          </div>

          <a href="https://sema.run" target="_blank" rel="noopener" class="vp-external-link-icon">
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

    <main>
      <slot :copy-text="copyText" />
    </main>

    <footer>
      <div class="wrap foot-in">
        <span><span style="color:var(--gold)">(</span>sema<span style="color:var(--gold)">)</span></span>
        <span>
          <a href="/docs/">Docs</a> ·
          <a href="/docs/internals/lisp-comparison">Benchmarks</a> ·
          <a href="https://github.com/HelgeSverre/sema/blob/main/CHANGELOG.md">Changelog</a> ·
          <a href="https://github.com/HelgeSverre/sema">GitHub</a> ·
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
}
.custom-home .dd-caret { font-size: 9px; opacity: 0.6; }
.custom-home .nav-dropdown:hover .dd-label,
.custom-home .nav-dropdown.dd-active .dd-label { color: var(--text) !important; }
.custom-home .nav-dropdown.dd-active .dd-label { color: var(--gold-bright) !important; }

.custom-home .dd-menu {
  position: absolute;
  top: 100%;
  left: -12px;
  min-width: 180px;
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
  text-decoration: none;
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
  .custom-home .dd-caret { display: none; }
  .custom-home .dd-menu {
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
