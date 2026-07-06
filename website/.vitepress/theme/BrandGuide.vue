<script setup>
import {ref} from 'vue'
// The actual shipped, flattened icons (from assets/icons/svg via gen-brand-assets.py)
// so this guide can't drift from what really ships.
import {
  sMarkTile as canonMark,
  fileSemaLight as canonFileLight, fileSemaDark as canonFileDark,
  fileSemac as canonSemac, fileNotebookLight as canonNbLight, fileNotebookDark as canonNbDark,
} from './brandAssets'
// Code Typer (<sema-code-typer>) showcase. The component ships as the published
// @sema-lang/ui package (loaded lazily on the client), and the sample source is
// vendored into website/ — so nothing reaches outside the website-only Vercel
// deploy.
import {onMounted} from 'vue'
import mazeSource from './maze.sample.sema?raw'
onMounted(() => { import('@sema-lang/ui/standalone') })

const copied = ref({})

// Live generated cards (website/public/og/<slug>.jpg) produced by
// scripts/generate-og.mjs — shown as-is so this section can never drift from
// what actually ships. Regenerate with `make site-og`.
const activeOgVariation = ref(0)
const ogVariations = [
  { label: 'stdlib / http-json', slug: 'docs-stdlib-http-json' },
  { label: 'llm / conversations', slug: 'docs-llm-conversations' },
  { label: 'tools / dap', slug: 'docs-dap' },
  { label: 'tools / lsp', slug: 'docs-lsp' },
  { label: 'guide / quickstart', slug: 'docs-quickstart' }
]

const copyToClipboard = (text, key) => {
  if (navigator.clipboard) {
    navigator.clipboard.writeText(text).then(() => {
      copied.value[key] = true
      setTimeout(() => {
        copied.value[key] = false
      }, 2000)
    }).catch(err => {
      console.error('Failed to copy text: ', err)
    })
  }
}

const colors = [
  { name: 'gold', hex: '#c8a855', oklch: 'oklch(73.57% 0.144 85.34)', rgba: 'rgba(200, 168, 85, 1)', role: 'Primary accent, CTA, links, active states, key keywords.', type: 'Accent' },
  { name: 'gold-dim', hex: 'rgba(200, 168, 85, 0.5)', oklch: 'oklch(73.57% 0.144 85.34 / 0.5)', rgba: 'rgba(200, 168, 85, 0.5)', role: 'Transparent borders, stale/inactive indicators.', type: 'Accent Opacity' },
  { name: 'gold-glow', hex: 'rgba(200, 168, 85, 0.08)', oklch: 'oklch(73.57% 0.144 85.34 / 0.08)', rgba: 'rgba(200, 168, 85, 0.08)', role: 'Hover backgrounds on interactive rows.', type: 'Accent Opacity' },
  { name: 'gold-soft', hex: 'rgba(200, 168, 85, 0.14)', oklch: 'oklch(73.57% 0.144 85.34 / 0.14)', rgba: 'rgba(200, 168, 85, 0.14)', role: 'Selection overlay background.', type: 'Accent Opacity' },
  { name: 'bg', hex: '#131110', oklch: 'oklch(10.57% 0.007 68.32)', rgba: 'rgba(19, 17, 16, 1)', role: 'Page canvas, sidebar background, main window.', type: 'Background' },
  { name: 'bg-elevated', hex: '#181512', oklch: 'oklch(12.21% 0.012 71.95)', rgba: 'rgba(24, 21, 18, 1)', role: 'Cards, toolbars, raised surfaces, modals.', type: 'Background' },
  { name: 'bg-editor', hex: '#1c1916', oklch: 'oklch(13.75% 0.014 74.45)', rgba: 'rgba(28, 25, 22, 1)', role: 'Code editing areas, text inputs, focused blocks.', type: 'Background' },
  { name: 'bg-output', hex: '#0f0d0c', oklch: 'oklch(8.65% 0.006 68.32)', rgba: 'rgba(15, 13, 12, 1)', role: 'Deepest level. Output panels, read-only consoles.', type: 'Background' },
  { name: 'text-primary', hex: '#e9e3d6', oklch: 'oklch(91.31% 0.021 78.43)', rgba: 'rgba(233, 227, 214, 1)', role: 'Body copy on marketing pages, primary headings.', type: 'Text' },
  { name: 'text-secondary', hex: '#968c79', oklch: 'oklch(60.91% 0.025 80.24)', rgba: 'rgba(150, 140, 121, 1)', role: 'Standard UI labels, secondary headings, editor text.', type: 'Text' },
  { name: 'text-tertiary', hex: '#6b6354', oklch: 'oklch(44.89% 0.022 79.52)', rgba: 'rgba(107, 99, 84, 1)', role: 'Muted comments, captions, disabled states.', type: 'Text' },
  { name: 'border', hex: '#2b2620', oklch: 'oklch(19.34% 0.015 76.51)', rgba: 'rgba(43, 38, 32, 1)', role: 'Default borders, dividers, subtle structure.', type: 'Structure' },
  { name: 'border-focus', hex: '#c8a855', oklch: 'oklch(73.57% 0.144 85.34)', rgba: 'rgba(200, 168, 85, 1)', role: 'Highlighted/focused borders.', type: 'Structure' },
  { name: 'success', hex: '#6a9955', oklch: 'oklch(60.67% 0.117 138.83)', rgba: 'rgba(106, 153, 85, 1)', role: 'Success indicators, green highlights.', type: 'Semantic' },
  { name: 'error', hex: '#c85555', oklch: 'oklch(53.25% 0.163 24.31)', rgba: 'rgba(200, 85, 85, 1)', role: 'Error messages, validation alerts.', type: 'Semantic' }
]

const colorGroups = [
  {
    title: 'Accent & Opacity Scales',
    colors: colors.filter(c => c.type.startsWith('Accent'))
  },
  {
    title: 'Background Hierarchy',
    colors: colors.filter(c => c.type === 'Background')
  },
  {
    title: 'Typography & Text',
    colors: colors.filter(c => c.type === 'Text')
  },
  {
    title: 'Structure & Borders',
    colors: colors.filter(c => c.type === 'Structure')
  },
  {
    title: 'Semantic Status',
    colors: colors.filter(c => c.type === 'Semantic')
  }
]

const cssVariablesCode = `:root {
  --gold: #c8a855;
  --gold-dim: rgba(200, 168, 85, 0.5);
  --gold-glow: rgba(200, 168, 85, 0.08);
  --gold-soft: rgba(200, 168, 85, 0.14);
  --bg: #131110;
  --bg-elevated: #181512;
  --bg-editor: #1c1916;
  --bg-output: #0f0d0c;
  --text-primary: #e9e3d6;
  --text-secondary: #968c79;
  --text-tertiary: #6b6354;
  --border: #2b2620;
  --border-focus: #c8a855;
  --success: #6a9955;
  --error: #c85555;
}`

const logoSvgCode = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 366.00 132.00" width="366.00" height="132.00">
  <path d="M48.5000 104.3000L48.5000 114Q34 110.7000 26.0500 100.5000Q18.1000 90.3000 18.1000 75L18.1000 57Q18.1000 41.7000 26.0500 31.5000Q34 21.3000 48.5000 18L48.5000 27.6000Q42.2000 29.1000 37.6000 33.1500Q33 37.2000 30.5000 43.3000Q28 49.4000 28 57L28 75Q28 82.6000 30.5000 88.6500Q33 94.7000 37.6000 98.7500Q42.2000 102.8000 48.5000 104.3000" fill="#c8a855" />
  <path d="M93.2000 102.8000L88.8000 102.8000Q79.4000 102.8000 74.2000 98.6000Q69 94.4000 69 86.8000L78.8000 86.8000Q78.8000 90.4000 81.4500 92.4500Q84.1000 94.5000 88.8000 94.5000L93.2000 94.5000Q98.1000 94.5000 100.7500 92.4000Q103.4000 90.3000 103.4000 86.5000Q103.4000 79.8000 96.8000 79L82 76.9000Q76.1000 76 72.9000 72.0500Q69.7000 68.1000 69.7000 61.8000Q69.7000 54.4000 74.7000 50.3000Q79.7000 46.2000 88.7000 46.2000L93.1000 46.2000Q101.5000 46.2000 106.7000 50.2000Q111.9000 54.2000 112.2000 60.8000L102.2000 60.8000Q102 58 99.6000 56.1500Q97.2000 54.3000 93.1000 54.3000L88.7000 54.3000Q84.2000 54.3000 81.7500 56.3000Q79.3000 58.3000 79.3000 61.7000Q79.3000 67.2000 84.8000 67.9000L98.7000 69.9000Q113 71.8000 113 86.5000Q113 94.3000 107.8500 98.5500Q102.7000 102.8000 93.2000 102.8000 M152 103Q142.1000 103 136.0500 97.1000Q130 91.2000 130 81L130 68Q130 57.8000 136.0500 51.9000Q142.1000 46 152 46Q158.6000 46 163.5500 48.6500Q168.5000 51.3000 171.2500 56.0500Q174 60.8000 174 67.1000L174 77L139.7000 77L139.7000 81.8000Q139.7000 87.8000 143 91.2000Q146.3000 94.6000 152 94.6000Q156.8000 94.6000 159.9000 92.8000Q163 91 163.6000 87.8000L173.5000 87.8000Q172.5000 94.8000 166.6000 98.9000Q160.7000 103 152 103M139.7000 67.1000L139.7000 69.7000L164.3000 69.7000L164.3000 67.1000Q164.3000 60.8000 161.1000 57.4000Q157.9000 54 152 54Q146.1000 54 142.9000 57.4000Q139.7000 60.8000 139.7000 67.1000 M197.7000 102L188.7000 102L188.7000 47L197.1000 47L197.1000 54.5000L197.4000 54.5000Q197.8000 50.7000 200.2500 48.3500Q202.7000 46 206.5000 46Q210.2000 46 212.7000 48.2000Q215.2000 50.4000 216.3000 54.1000Q216.9000 50.3000 219.4000 48.1500Q221.9000 46 225.7000 46Q230.9000 46 234.1000 49.9500Q237.3000 53.9000 237.3000 60.2000L237.3000 102L228.3000 102L228.3000 60.3000Q228.3000 57.2000 226.7500 55.3500Q225.2000 53.5000 222.6000 53.5000Q220 53.5000 218.5000 55.3000Q217 57.1000 217 60.3000L217 102L209 102L209 60.3000Q209 57.2000 207.5000 55.3500Q206 53.5000 203.4000 53.5000Q200.8000 53.5000 199.2500 55.3000Q197.7000 57.1000 197.7000 60.3000 M268.9000 103Q260.4000 103 255.4500 98.2500Q250.5000 93.5000 250.5000 85.8000Q250.5000 78.1000 255.6500 73.4000Q260.8000 68.7000 269.2000 68.7000L285.3000 68.7000L285.3000 64.5000Q285.3000 54.4000 274.1000 54.4000Q269.1000 54.4000 266.0500 56.2500Q263 58.1000 262.8000 61.4000L253 61.4000Q253.5000 54.7000 259.1000 50.3500Q264.7000 46 274.1000 46Q284.2000 46 289.7000 50.8000Q295.2000 55.6000 295.2000 64.3000L295.2000 102L285.5000 102L285.5000 91.9000L285.3000 91.9000Q284.6000 97 280.2500 100Q275.9000 103 268.9000 103M271.5000 94.7000Q277.8000 94.7000 281.5500 91.6500Q285.3000 88.6000 285.3000 83.3000L285.3000 76L270.1000 76Q265.8000 76 263.1500 78.5500Q260.5000 81.1000 260.5000 85.3000Q260.5000 89.6000 263.4000 92.1500Q266.3000 94.7000 271.5000 94.7000" fill="#ffffff" />
  <path d="M316.5000 114L316.5000 104.3000Q322.8000 102.8000 327.4000 98.7500Q332 94.7000 334.5500 88.6500Q337.1000 82.6000 337.1000 75L337.1000 57Q337.1000 49.4000 334.5500 43.3000Q332 37.2000 327.4000 33.1500Q322.8000 29.1000 316.5000 27.6000L316.5000 18Q331 21.3000 339 31.5000Q347 41.7000 347 57L347 75Q347 90.3000 339 100.5000Q331 110.7000 316.5000 114" fill="#c8a855" />
</svg>`;

const logoSubtleSvgCode = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 366 132" style="width: 100%; height: auto; display: block;">
  <path d="M48.5000 104.3000L48.5000 114Q34 110.7000 26.0500 100.5000Q18.1000 90.3000 18.1000 75L18.1000 57Q18.1000 41.7000 26.0500 31.5000Q34 21.3000 48.5000 18L48.5000 27.6000Q42.2000 29.1000 37.6000 33.1500Q33 37.2000 30.5000 43.3000Q28 49.4000 28 57L28 75Q28 82.6000 30.5000 88.6500Q33 94.7000 37.6000 98.7500Q42.2000 102.8000 48.5000 104.3000" fill="currentColor" opacity="0.6" />
  <path d="M93.2000 102.8000L88.8000 102.8000Q79.4000 102.8000 74.2000 98.6000Q69 94.4000 69 86.8000L78.8000 86.8000Q78.8000 90.4000 81.4500 92.4500Q84.1000 94.5000 88.8000 94.5000L93.2000 94.5000Q98.1000 94.5000 100.7500 92.4000Q103.4000 90.3000 103.4000 86.5000Q103.4000 79.8000 96.8000 79L82 76.9000Q76.1000 76 72.9000 72.0500Q69.7000 68.1000 69.7000 61.8000Q69.7000 54.4000 74.7000 50.3000Q79.7000 46.2000 88.7000 46.2000L93.1000 46.2000Q101.5000 46.2000 106.7000 50.2000Q111.9000 54.2000 112.2000 60.8000L102.2000 60.8000Q102 58 99.6000 56.1500Q97.2000 54.3000 93.1000 54.3000L88.7000 54.3000Q84.2000 54.3000 81.7500 56.3000Q79.3000 58.3000 79.3000 61.7000Q79.3000 67.2000 84.8000 67.9000L98.7000 69.9000Q113 71.8000 113 86.5000Q113 94.3000 107.8500 98.5500Q102.7000 102.8000 93.2000 102.8000 M152 103Q142.1000 103 136.0500 97.1000Q130 91.2000 130 81L130 68Q130 57.8000 136.0500 51.9000Q142.1000 46 152 46Q158.6000 46 163.5500 48.6500Q168.5000 51.3000 171.2500 56.0500Q174 60.8000 174 67.1000L174 77L139.7000 77L139.7000 81.8000Q139.7000 87.8000 143 91.2000Q146.3000 94.6000 152 94.6000Q156.8000 94.6000 159.9000 92.8000Q163 91 163.6000 87.8000L173.5000 87.8000Q172.5000 94.8000 166.6000 98.9000Q160.7000 103 152 103M139.7000 67.1000L139.7000 69.7000L164.3000 69.7000L164.3000 67.1000Q164.3000 60.8000 161.1000 57.4000Q157.9000 54 152 54Q146.1000 54 142.9000 57.4000Q139.7000 60.8000 139.7000 67.1000 M197.7000 102L188.7000 102L188.7000 47L197.1000 47L197.1000 54.5000L197.4000 54.5000Q197.8000 50.7000 200.2500 48.3500Q202.7000 46 206.5000 46Q210.2000 46 212.7000 48.2000Q215.2000 50.4000 216.3000 54.1000Q216.9000 50.3000 219.4000 48.1500Q221.9000 46 225.7000 46Q230.9000 46 234.1000 49.9500Q237.3000 53.9000 237.3000 60.2000L237.3000 102L228.3000 102L228.3000 60.3000Q228.3000 57.2000 226.7500 55.3500Q225.2000 53.5000 222.6000 53.5000Q220 53.5000 218.5000 55.3000Q217 57.1000 217 60.3000L217 102L209 102L209 60.3000Q209 57.2000 207.5000 55.3500Q206 53.5000 203.4000 53.5000Q200.8000 53.5000 199.2500 55.3000Q197.7000 57.1000 197.7000 60.3000 M268.9000 103Q260.4000 103 255.4500 98.2500Q250.5000 93.5000 250.5000 85.8000Q250.5000 78.1000 255.6500 73.4000Q260.8000 68.7000 269.2000 68.7000L285.3000 68.7000L285.3000 64.5000Q285.3000 54.4000 274.1000 54.4000Q269.1000 54.4000 266.0500 56.2500Q263 58.1000 262.8000 61.4000L253 61.4000Q253.5000 54.7000 259.1000 50.3500Q264.7000 46 274.1000 46Q284.2000 46 289.7000 50.8000Q295.2000 55.6000 295.2000 64.3000L295.2000 102L285.5000 102L285.5000 91.9000L285.3000 91.9000Q284.6000 97 280.2500 100Q275.9000 103 268.9000 103M271.5000 94.7000Q277.8000 94.7000 281.5500 91.6500Q285.3000 88.6000 285.3000 83.3000L285.3000 76L270.1000 76Q265.8000 76 263.1500 78.5500Q260.5000 81.1000 260.5000 85.3000Q260.5000 89.6000 263.4000 92.1500Q266.3000 94.7000 271.5000 94.7000" fill="currentColor" />
  <path d="M316.5000 114L316.5000 104.3000Q322.8000 102.8000 327.4000 98.7500Q332 94.7000 334.5500 88.6500Q337.1000 82.6000 337.1000 75L337.1000 57Q337.1000 49.4000 334.5500 43.3000Q332 37.2000 327.4000 33.1500Q322.8000 29.1000 316.5000 27.6000L316.5000 18Q331 21.3000 339 31.5000Q347 41.7000 347 57L347 75Q347 90.3000 339 100.5000Q331 110.7000 316.5000 114" fill="currentColor" opacity="0.6" />
</svg>`;


const icons = {
  oldSema: `<svg viewBox="0 0 16 16" width="64" height="64"><rect width="16" height="16" rx="3" fill="#1a1a1a"/><text x="8" y="12.5" text-anchor="middle" font-family="Georgia, 'Times New Roman', serif" font-weight="600" font-size="13" fill="#c8a855">S</text></svg>`,
  newSema: canonFileDark,

  oldSemac: `<svg viewBox="0 0 16 16" width="64" height="64"><rect width="16" height="16" rx="3" fill="#1a1a1a"/><text x="8" y="12.5" text-anchor="middle" font-family="Georgia, 'Times New Roman', serif" font-weight="600" font-size="13" fill="#7a7a7a">S</text><rect x="9" y="9" width="6" height="6" rx="1" fill="#c8a855" opacity="0.8"/><text x="12" y="13.5" text-anchor="middle" font-family="monospace" font-weight="700" font-size="5" fill="#1a1a1a">c</text></svg>`,
  newSemac: canonSemac,

  oldSemaNotebook: `<svg viewBox="0 0 16 16" width="64" height="64"><rect width="16" height="16" rx="3" fill="#1a1a1a"/><rect x="3" y="1.5" width="10" height="13" rx="1" fill="none" stroke="#c8a855" stroke-width="0.7" opacity="0.5"/><line x1="4" y1="5" x2="12" y2="5" stroke="#c8a855" stroke-width="0.6" opacity="0.35"/><line x1="4" y1="8" x2="10" y2="8" stroke="#c8a855" stroke-width="0.6" opacity="0.35"/><text x="8" y="12.5" text-anchor="middle" font-family="Georgia, 'Times New Roman', serif" font-weight="600" font-size="7" fill="#c8a855">S</text></svg>`,
  newSemaNotebook: canonNbDark,

  oldSemaNotebookCells: `<svg viewBox="0 0 16 16" width="64" height="64"><rect width="16" height="16" rx="3" fill="#1a1a1a"/><rect x="3" y="1.5" width="10" height="13" rx="1" fill="#222" stroke="#c8a855" stroke-width="0.5" opacity="0.6"/><rect x="4" y="4" width="8" height="2.5" rx="0.5" fill="#c8a855" opacity="0.2"/><rect x="4" y="7.5" width="6" height="0.7" rx="0.3" fill="#aaa" opacity="0.4"/><rect x="4" y="9" width="4" height="0.7" rx="0.3" fill="#aaa" opacity="0.4"/><text x="8" y="12.5" text-anchor="middle" font-family="Georgia, 'Times New Roman', serif" font-weight="600" font-size="5" fill="#c8a855">S</text></svg>`,
  newSemaNotebookCells: `<svg viewBox="0 0 16 16" width="64" height="64"><rect width="16" height="16" rx="3" fill="#1a1a1a"/><rect x="3" y="1.5" width="10" height="13" rx="1" fill="#222" stroke="#c8a855" stroke-width="0.5" opacity="0.6"/><rect x="4" y="4" width="8" height="2.5" rx="0.5" fill="#c8a855" opacity="0.2"/><rect x="4" y="7.5" width="6" height="0.7" rx="0.3" fill="#aaa" opacity="0.4"/><rect x="4" y="9" width="4" height="0.7" rx="0.3" fill="#aaa" opacity="0.4"/><text x="8" y="13" text-anchor="middle" font-family="'JetBrains Mono', monospace" font-weight="700" font-size="4.5" fill="#ffffff"><tspan fill="#c8a855">(</tspan>s<tspan fill="#c8a855">)</tspan></text></svg>`,

  oldSemaNotebookPlay: `<svg viewBox="0 0 16 16" width="64" height="64"><rect width="16" height="16" rx="3" fill="#1a1a1a"/><rect x="2.5" y="1.5" width="10" height="13" rx="1" fill="none" stroke="#c8a855" stroke-width="0.7" opacity="0.5"/><line x1="3.5" y1="5" x2="11.5" y2="5" stroke="#c8a855" stroke-width="0.6" opacity="0.35"/><text x="7.5" y="12.5" text-anchor="middle" font-family="Georgia, 'Times New Roman', serif" font-weight="600" font-size="7" fill="#c8a855">S</text><circle cx="12.5" cy="12.5" r="2.5" fill="#c8a855" opacity="0.9"/><polygon points="11.8,11.5 11.8,13.5 13.3,12.5" fill="#1a1a1a"/></svg>`,
  newSemaNotebookPlay: `<svg viewBox="0 0 16 16" width="64" height="64"><rect width="16" height="16" rx="3" fill="#1a1a1a"/><rect x="2.5" y="1.5" width="10" height="13" rx="1" fill="none" stroke="#c8a855" stroke-width="0.7" opacity="0.5"/><line x1="3.5" y1="5" x2="11.5" y2="5" stroke="#c8a855" stroke-width="0.6" opacity="0.35"/><text x="7" y="11.5" text-anchor="middle" font-family="'JetBrains Mono', monospace" font-weight="700" font-size="5" fill="#ffffff"><tspan fill="#c8a855">(</tspan>s<tspan fill="#c8a855">)</tspan></text><circle cx="12.5" cy="12.5" r="2.5" fill="#c8a855" opacity="0.9"/><polygon points="11.8,11.5 11.8,13.5 13.3,12.5" fill="#1a1a1a"/></svg>`,

  oldSemaNotebookStacked: `<svg viewBox="0 0 16 16" width="64" height="64"><rect width="16" height="16" rx="3" fill="#1a1a1a"/><rect x="4" y="2.5" width="10" height="11" rx="1" fill="none" stroke="#c8a855" stroke-width="0.5" opacity="0.25"/><rect x="2.5" y="1.5" width="10" height="12" rx="1" fill="none" stroke="#c8a855" stroke-width="0.7" opacity="0.55"/><line x1="3.5" y1="5" x2="11.5" y2="5" stroke="#c8a855" stroke-width="0.6" opacity="0.35"/><line x1="3.5" y1="8" x2="9.5" y2="8" stroke="#c8a855" stroke-width="0.6" opacity="0.35"/><text x="7.5" y="11.5" text-anchor="middle" font-family="Georgia, 'Times New Roman', serif" font-weight="600" font-size="6" fill="#c8a855">S</text></svg>`,
  newSemaNotebookStacked: `<svg viewBox="0 0 16 16" width="64" height="64"><rect width="16" height="16" rx="3" fill="#1a1a1a"/><rect x="4" y="2.5" width="10" height="11" rx="1" fill="none" stroke="#c8a855" stroke-width="0.5" opacity="0.25"/><rect x="2.5" y="1.5" width="10" height="12" rx="1" fill="none" stroke="#c8a855" stroke-width="0.7" opacity="0.55"/><line x1="3.5" y1="5" x2="11.5" y2="5" stroke="#c8a855" stroke-width="0.6" opacity="0.35"/><line x1="3.5" y1="8" x2="9.5" y2="8" stroke="#c8a855" stroke-width="0.6" opacity="0.35"/><text x="7" y="11.5" text-anchor="middle" font-family="'JetBrains Mono', monospace" font-weight="700" font-size="5.5" fill="#ffffff"><tspan fill="#c8a855">(</tspan>s<tspan fill="#c8a855">)</tspan></text></svg>`,

  oldFavicon: `<svg viewBox="0 0 32 32" width="64" height="64"><rect width="32" height="32" rx="6" fill="#1a1a1a"/><text x="16" y="24.5" text-anchor="middle" font-family="Georgia, 'Times New Roman', serif" font-weight="600" font-size="26" fill="#c8a855">S</text></svg>`,
  newFavicon: canonMark,

  fileSema: canonFileDark,
  fileSemac: canonSemac,
  fileSemaNotebook: canonNbDark,

  // Light-theme variants: the glyph flips to a dark ink so it stays legible on
  // light IDE backgrounds (the dark-theme versions above use a white glyph).
  // .semac reads on both themes (muted gray), so it has no separate variant.
  fileSemaLight: canonFileLight,
  fileSemaNotebookLight: canonNbLight,

  // New-style notebook icon candidates (transparent (s) family). Not yet wired
  // into any product — parked here for future use (e.g. notebook actions /
  // tool windows). Dark-theme glyph (white s); pair with a light variant if used.
  nbPageBadge: `<svg viewBox="0 0 16 16" width="32" height="32" fill="none"><text x="7" y="11.5" text-anchor="middle" font-family="'JetBrains Mono', monospace" font-weight="700" font-size="8.5" fill="#ffffff"><tspan fill="#c8a855" fill-opacity="0.65">(</tspan>s<tspan fill="#c8a855" fill-opacity="0.65">)</tspan></text><rect x="10.5" y="10.5" width="4.5" height="4.5" rx="0.5" fill="#131110" stroke="#c8a855" stroke-width="0.75"/><line x1="12" y1="12" x2="13.5" y2="12" stroke="#c8a855" stroke-width="0.5"/><line x1="12" y1="13.5" x2="13.5" y2="13.5" stroke="#c8a855" stroke-width="0.5"/></svg>`,
  nbPlayBadge: `<svg viewBox="0 0 16 16" width="32" height="32" fill="none"><text x="7" y="11.5" text-anchor="middle" font-family="'JetBrains Mono', monospace" font-weight="700" font-size="8.5" fill="#ffffff"><tspan fill="#c8a855" fill-opacity="0.65">(</tspan>s<tspan fill="#c8a855" fill-opacity="0.65">)</tspan></text><circle cx="12.8" cy="12.6" r="3" fill="#c8a855"/><path d="M11.8 10.9 L14.2 12.6 L11.8 14.3 Z" fill="#131110"/></svg>`,
  nbStacked: `<svg viewBox="0 0 16 16" width="32" height="32" fill="none"><text x="7" y="11.5" text-anchor="middle" font-family="'JetBrains Mono', monospace" font-weight="700" font-size="8.5" fill="#ffffff"><tspan fill="#c8a855" fill-opacity="0.65">(</tspan>s<tspan fill="#c8a855" fill-opacity="0.65">)</tspan></text><rect x="11.3" y="9.6" width="3.6" height="3.6" rx="0.4" fill="#131110" stroke="#c8a855" stroke-width="0.7"/><rect x="9.9" y="11.0" width="3.6" height="3.6" rx="0.4" fill="#131110" stroke="#c8a855" stroke-width="0.7"/></svg>`,
  nbFullPage: `<svg viewBox="0 0 16 16" width="32" height="32" fill="none"><rect x="2.5" y="1.5" width="11" height="13" rx="1.5" fill="none" stroke="#c8a855" stroke-width="0.9" stroke-opacity="0.65"/><line x1="4.5" y1="4.6" x2="11.5" y2="4.6" stroke="#c8a855" stroke-width="0.7" stroke-opacity="0.45"/><line x1="4.5" y1="6.6" x2="9.5" y2="6.6" stroke="#c8a855" stroke-width="0.7" stroke-opacity="0.45"/><text x="8" y="12.6" text-anchor="middle" font-family="'JetBrains Mono', monospace" font-weight="700" font-size="6.5" fill="#ffffff"><tspan fill="#c8a855" fill-opacity="0.65">(</tspan>s<tspan fill="#c8a855" fill-opacity="0.65">)</tspan></text></svg>`,
  nbCellLines: `<svg viewBox="0 0 16 16" width="32" height="32" fill="none"><text x="8" y="8.6" text-anchor="middle" font-family="'JetBrains Mono', monospace" font-weight="700" font-size="8" fill="#ffffff"><tspan fill="#c8a855" fill-opacity="0.65">(</tspan>s<tspan fill="#c8a855" fill-opacity="0.65">)</tspan></text><line x1="4" y1="11.6" x2="12" y2="11.6" stroke="#c8a855" stroke-width="0.9" stroke-opacity="0.6"/><line x1="4" y1="13.6" x2="9.5" y2="13.6" stroke="#c8a855" stroke-width="0.9" stroke-opacity="0.6"/></svg>`,
}

const copyIcon = (key) => {
  copyToClipboard(icons[key], key)
}
</script>

<template>
  <div class="brand-guide">
    <!-- Sticky Banner/Header -->
    <header class="brand-hero">
      <div class="brand-container header-inner">
        <span class="hero-tag">Design System Specification</span>
        <h1 class="hero-title">Sema Brand Identity</h1>
        <p class="hero-subtitle">
          Canonical style guide for the Sema Lisp programming language, computational notebook, and CLI toolchain. Built
          around warm dark architectures, gold emphasis, and functional typography.
        </p>
      </div>
    </header>

    <div class="brand-guide-layout">
      <!-- Sticky Sidebar Navigation -->
      <nav class="brand-sidebar">
        <ul>
          <li><a href="#overview">Overview</a></li>
          <li><a href="#logo">01. Logotype</a></li>
          <li><a href="#typography">02. Typography</a></li>
          <li><a href="#prose">03. Prose Idioms</a></li>
          <li><a href="#colors">04. Color Palette</a></li>
          <li><a href="#depth">05. Depth &amp; Radius</a></li>
          <li><a href="#uikit">06. UI Kit Components</a></li>
          <li><a href="#syntax">07. Syntax convergence</a></li>
          <li><a href="#icons">08. Icon Showcase</a></li>
          <li><a href="#inventory">09. Project Inventory</a></li>
          <li><a href="#rules">10. Visual Rules</a></li>
          <li><a href="#opengraph">11. OpenGraph Cards</a></li>
          <!-- <li><a href="#typer">12. Code Typer</a></li> TEMPORARILY DISABLED -->
        </ul>
      </nav>

      <!-- Main Contents -->
      <main class="brand-content">
        <!-- Overview Section -->
        <section id="overview" class="brand-section">
          <div class="section-meta">
            <h2 class="section-title">Design Philosophy</h2>
            <p class="section-desc">
              Sema's identity reflects a modern, warm, and highly structured environment. It merges the mechanical
              precision of an s-expression evaluator with the organic fluency of LLM integration. Interfaces should feel
              like a premium text editor—focused, dark, and visually stable.
            </p>
          </div>
          <div class="overview-visual-grid">
            <div class="overview-card">
              <h4>Warm Architectural Dark</h4>
              <p>Interfaces are constructed from layered dark values, simulating physical depth. We avoid harsh #000
                black and bright white borders.</p>
            </div>
            <div class="overview-card">
              <h4>Accent as Focus</h4>
              <p>Gold is used sparingly as a high-contrast focus and status color. It draws the eye to running
                operations, active lines, and key indicators.</p>
            </div>
          </div>

          <div class="name-origin" style="margin-top: 3rem; padding-top: 2.5rem; border-top: 1px solid var(--border); max-width: 760px;">
            <span class="section-num" style="margin-bottom: 1.25rem;">The Name</span>
            <p style="font-family: 'Cormorant', Georgia, serif; font-size: 2rem; font-weight: 300; color: var(--text-primary); line-height: 1.3; margin: 0 0 1.15rem 0;">
              Sema takes its name from Ancient Greek <em style="color: var(--gold); font-style: italic;">sêma</em> <span style="color: var(--text-tertiary);">(σῆμα)</span> — a sign, signal, or token of meaning.
            </p>
            <p style="font-family: 'Inter', system-ui, sans-serif; font-size: 1rem; color: var(--text-secondary); line-height: 1.65; max-width: 42rem; margin: 0;">
              The same root runs through <em>semantics</em>, <em>semaphore</em>, and <em>semiotics</em> — fitting for a language built to carry meaning legibly between humans, machines, and the models that read both.
            </p>
          </div>
        </section>

        <!-- Logotype Section -->
        <section id="logo" class="brand-section">
          <div class="section-meta">
            <span class="section-num">01</span>
            <h2 class="section-title">The Logo Mark</h2>
            <p class="section-desc">
              The canonical logotype <code>(sema)</code> combines parentheses in warm gold with high-contrast monospace
              letters in pure white. It represents the marriage of Lisp s-expressions and modern machine-learning
              models.
            </p>
          </div>

          <div class="logo-display-card">
            <div class="logo-preview-area">
              <svg class="preview-svg" viewBox="0 0 366 132" fill="none">
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
            </div>
            <div class="logo-code-area">
              <div class="code-header">
                <span>logo.svg</span>
                <button class="btn-copy-code" @click="copyToClipboard(logoSvgCode, 'svg')">
                  {{ copied['svg'] ? 'Copied!' : 'Copy SVG Vector' }}
                </button>
              </div>
              <pre class="code-view"><code>{{ logoSvgCode }}</code></pre>
            </div>
          </div>

          <h3 class="logo-subheading" style="margin-top: 3rem; font-family: 'Cormorant', Georgia, serif; font-size: 1.5rem; color: #e9e3d6; font-weight: 400; margin-bottom: 0.5rem;">Subtle / Footer Logo Variant</h3>
          <p class="section-desc" style="margin-bottom: 1.5rem;">
            A monochrome variant using <code>fill="currentColor"</code> that inherits its color from the parent container. Designed for footers, secondary branding, and visual environments that require low contrast.
          </p>

          <div class="logo-display-card subtle-variant">
            <div class="logo-preview-area logo-subtle-preview" style="display: flex; flex-direction: column; gap: 2rem; align-items: stretch; justify-content: center; padding: 2.5rem 2rem;">
              <div class="logo-subtle-showcase color-tertiary" style="color: var(--text-tertiary);">
                <span class="subtle-color-label" style="font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: var(--text-tertiary); text-transform: uppercase; margin-bottom: 0.5rem; display: block;">Muted Tertiary (var(--text-tertiary))</span>
                <div class="subtle-svg-wrap" style="max-width: 130px;" v-html="logoSubtleSvgCode"></div>
              </div>
              <div class="logo-subtle-showcase color-secondary" style="color: var(--text-secondary);">
                <span class="subtle-color-label" style="font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: var(--text-secondary); text-transform: uppercase; margin-bottom: 0.5rem; display: block;">UI Secondary (var(--text-secondary))</span>
                <div class="subtle-svg-wrap" style="max-width: 130px;" v-html="logoSubtleSvgCode"></div>
              </div>
            </div>
            <div class="logo-code-area">
              <div class="code-header">
                <span>logo-subtle.svg</span>
                <button class="btn-copy-code" @click="copyToClipboard(logoSubtleSvgCode, 'svg-subtle')">
                  {{ copied['svg-subtle'] ? 'Copied!' : 'Copy SVG Vector' }}
                </button>
              </div>
              <pre class="code-view"><code>{{ logoSubtleSvgCode }}</code></pre>
            </div>
          </div>

          <div class="footer-mockup-wrap" style="margin-top: 2rem; border: 1px solid var(--border); border-radius: 8px; overflow: hidden; background-color: #0f0d0c;">
            <div class="mockup-header" style="font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: var(--text-tertiary); text-transform: uppercase; padding: 0.5rem 1rem; border-bottom: 1px solid var(--border); background-color: #181512;">Usage Context: Footer Navbar</div>
            <footer class="mockup-footer" style="padding: 1.5rem 2rem; background-color: #131110; border-top: 1px solid var(--border);">
              <div class="foot-in" style="display: flex; flex-wrap: wrap; justify-content: space-between; gap: 14px; font-size: 13px; color: var(--text-tertiary); align-items: center; width: 100%;">
                <span style="display: flex; align-items: center; gap: 8px;">
                  <span class="mockup-footer-logo" style="color: var(--text-tertiary); max-width: 65px; display: inline-block; vertical-align: middle;" v-html="logoSubtleSvgCode"></span>
                  <span style="opacity: 0.85; font-family: 'JetBrains Mono', monospace; font-size: 0.75rem;">— MIT</span>
                </span>
                <span>
                  <a href="#overview" style="color: var(--text-secondary); text-decoration: none; margin-right: 6px;">Docs</a>
                  <span style="color: var(--border); margin: 0 4px;">·</span>
                  <a href="#overview" style="color: var(--text-secondary); text-decoration: none; margin: 0 6px;">Benchmarks</a>
                  <span style="color: var(--border); margin: 0 4px;">·</span>
                  <a href="https://github.com/sema-lisp/sema" target="_blank" style="color: var(--text-secondary); text-decoration: none; margin: 0 6px;">Changelog</a>
                  <span style="color: var(--border); margin: 0 4px;">·</span>
                  <a href="https://github.com/sema-lisp/sema" target="_blank" style="color: var(--text-secondary); text-decoration: none; margin-left: 6px;">GitHub</a>
                </span>
              </div>
            </footer>
          </div>
        </section>

        <!-- Typography Section -->
        <section id="typography" class="brand-section">
          <div class="section-meta">
            <span class="section-num">02</span>
            <h2 class="section-title">Typography &amp; Specimen</h2>
            <p class="section-desc">
              Sema relies on a strict dual-font layout system. We use <strong>Cormorant</strong> (serif) for editorial
              headings and branding elements, and <strong>JetBrains Mono</strong> (monospace) for technical indicators,
              syntax nodes, and CLI instructions. Inter provides clean body prose.
            </p>
          </div>

          <div class="type-container">
            <!-- Cormorant Serif Specimen -->
            <div class="font-specimen-block">
              <h3 class="font-label">Cormorant — Editorial &amp; Headings</h3>
              <div class="font-preview-large serif-font">
                Aa Bb Cc Dd Ee Ff Gg Hh Ii Jj Kk Ll Mm Nn Oo Pp Qq Rr Ss Tt Uu Vv Ww Xx Yy Zz
              </div>
              <div class="font-specimen-lines">
                <div class="specimen-line">
                  <div class="specimen-info">Heading 1 · 300 Light</div>
                  <div class="specimen-sample serif-font h1-preview">The future of programming is conversational</div>
                </div>
                <div class="specimen-line">
                  <div class="specimen-info">Heading 2 · 300 Light</div>
                  <div class="specimen-sample serif-font h2-preview">Evaluation as a dialogical loop</div>
                </div>
                <div class="specimen-line">
                  <div class="specimen-info">Heading 3 · 500 Medium</div>
                  <div class="specimen-sample serif-font h3-preview">Special Forms &amp; Native Evaluator</div>
                </div>
              </div>
            </div>

            <!-- Monospace Specimen -->
            <div class="font-specimen-block">
              <h3 class="font-label">JetBrains Mono — Syntax &amp; Code</h3>
              <div class="font-preview-large mono-font">
                Aa Bb Cc Dd Ee Ff Gg Hh Ii Jj Kk Ll Mm Nn Oo Pp Qq Rr Ss Tt Uu Vv Ww Xx Yy Zz 1234567890
              </div>
              <div class="font-specimen-lines">
                <div class="specimen-line">
                  <div class="specimen-info">Code Block · 400 Regular</div>
                  <div class="specimen-sample mono-font code-preview">
                    (defagent coder {:model "claude-haiku-4-5" :system "Rust programmer"})
                  </div>
                </div>
                <div class="specimen-line">
                  <div class="specimen-info">Code Small · 400 Regular</div>
                  <div class="specimen-sample mono-font code-preview-small">
                    (map (lambda (x) (* x 2)) (range 1 10)) ; => (2 4 6 8 10 12 14 16 18)
                  </div>
                </div>
              </div>
            </div>

            <!-- Inter Sans Specimen -->
            <div class="font-specimen-block">
              <h3 class="font-label">Inter — UI &amp; Body Prose</h3>
              <div class="font-preview-large sans-font" style="font-family: 'Inter', system-ui, sans-serif;">
                Aa Bb Cc Dd Ee Ff Gg Hh Ii Jj Kk Ll Mm Nn Oo Pp Qq Rr Ss Tt Uu Vv Ww Xx Yy Zz 1234567890
              </div>
              <div class="font-specimen-lines">
                <div class="specimen-line">
                  <div class="specimen-info">Body Prose · 400 Regular</div>
                  <div class="specimen-sample sans-font body-preview" style="font-family: 'Inter', system-ui, sans-serif; font-size: 1rem; color: var(--text-secondary); line-height: 1.6; text-align: left;">
                    Sema Lisp merges the speed of Rust with the power of modern LLMs. It is designed to be highly structured, conversationally persistent, and fully sandboxed.
                  </div>
                </div>
                <div class="specimen-line">
                  <div class="specimen-info">UI Label · 500 Medium</div>
                  <div class="specimen-sample sans-font ui-preview" style="font-family: 'Inter', system-ui, sans-serif; font-size: 0.85rem; font-weight: 500; color: var(--text-primary); text-align: left;">
                    Active Session: coder-agent (llm-run-active)
                  </div>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- Prose Showcase Section -->
        <section id="prose" class="brand-section">
          <div class="section-meta">
            <span class="section-num">03</span>
            <h2 class="section-title">Prose &amp; Editorial Idioms</h2>
            <p class="section-desc">
              Consistent layouts for storytelling and editorial blocks used across marketing pages and major documentation entries.
            </p>
          </div>

          <div class="prose-idioms-container" style="display: flex; flex-direction: column; gap: 3.5rem;">
            <!-- Kicker Heading Block -->
            <div class="prose-block-specimen">
              <div class="specimen-meta-label" style="font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: var(--text-tertiary); text-transform: uppercase; margin-bottom: 1rem; border-bottom: 1px solid var(--border); padding-bottom: 0.25rem;">
                A. The Objection Header (Kicker)
              </div>
              <div class="kicker-heading-showcase" style="border: 1px solid var(--border); padding: 2.5rem; border-radius: 8px; background-color: #181512; max-width: 720px;">
                <p class="kicker" style="font-family: 'Cormorant', Georgia, serif; font-size: 1.15rem; color: #c8a855; font-style: italic; margin: 0 0 0.5rem 0;">“Wait — a Lisp?”</p>
                <h2 style="font-family: 'Cormorant', Georgia, serif; font-size: 2.2rem; font-weight: 300; color: #e9e3d6; margin: 0 0 1rem 0; line-height: 1.25;">You won't write most of it anyway.</h2>
                <p class="sub" style="font-size: 1.05rem; color: #968c79; line-height: 1.6; max-width: 42rem; margin: 0;">
                  Your coding agent will. And a Lisp is the <strong>language with the least surface</strong> for an agent to be wrong about—see the <a href="https://github.com/sema-lisp/sema" target="_blank" style="text-decoration: underline; color: var(--gold);">open-source repository</a> to review the <u>complete language specifications</u>.
                </p>
              </div>
            </div>

            <!-- Claims List Specimen -->
            <div class="prose-block-specimen">
              <div class="specimen-meta-label" style="font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: var(--text-tertiary); text-transform: uppercase; margin-bottom: 1rem; border-bottom: 1px solid var(--border); padding-bottom: 0.25rem;">
                B. The Claims List with Highlights (.claims &amp; &lt;mark&gt;)
              </div>
              <div class="claims-list-showcase" style="border: 1px solid var(--border); padding: 2.5rem; border-radius: 8px; background-color: #181512; max-width: 720px;">
                <ul class="claims">
                  <li>
                    <strong>Sixty years of training data.</strong>
                    Lisp predates nearly everything else in the corpus. Scheme, Common Lisp, Clojure, Racket — your agent has read all of it, and a <u>Lisp is a Lisp</u>. Review the <a href="#overview" style="text-decoration: underline; color: var(--gold);">design philosophy</a>.
                  </li>
                  <li>
                    <strong>The whole language fits in context.</strong>
                    Point your agent at <mark>llms.txt</mark> — where Sema diverges from the dialects it already knows, and nothing else. Constraints, <u>not a textbook</u>.
                  </li>
                </ul>
              </div>
            </div>

            <!-- Ship List Specimen -->
            <div class="prose-block-specimen">
              <div class="specimen-meta-label" style="font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: var(--text-tertiary); text-transform: uppercase; margin-bottom: 1rem; border-bottom: 1px solid var(--border); padding-bottom: 0.25rem;">
                C. The Chevron List (.ship-list)
              </div>
              <div class="ship-list-showcase" style="border: 1px solid var(--border); padding: 2.5rem; border-radius: 8px; background-color: #181512; max-width: 720px;">
                <ul class="ship-list">
                  <li>
                    <span><strong>Minimal footprint.</strong> The entire compiler and VM is a single statically linked binary with <u>zero external runtime dependencies</u>.</span>
                  </li>
                  <li>
                    <span><strong>Instant startup.</strong> VM boots in less than 1ms. Fast enough for <a href="https://github.com/sema-lisp/sema" target="_blank" style="text-decoration: underline; color: var(--gold);">serverless functions</a> and ephemeral CLI invocations.</span>
                  </li>
                </ul>
              </div>
            </div>
          </div>
        </section>

        <!-- Color Palette Section -->
        <section id="colors" class="brand-section">
          <div class="section-meta">
            <span class="section-num">04</span>
            <h2 class="section-title">Color Palette</h2>
            <p class="section-desc">
              Colors are structured entirely in a deep, warm dark palette. Click any swatch card below to copy its CSS
              value to your clipboard.
            </p>
          </div>

          <div class="css-variables-block" style="margin-bottom: 2.5rem;">
            <div class="code-header">
              <span>variables.css</span>
              <button class="btn-copy-code" @click="copyToClipboard(cssVariablesCode, 'variables')">
                {{ copied['variables'] ? 'Copied!' : 'Copy CSS Variables' }}
              </button>
            </div>
            <pre class="code-view"><code>{{ cssVariablesCode }}</code></pre>
          </div>

          <div class="table-wrap colors-table-wrap">
            <table class="colors-table">
              <thead>
                <tr>
                  <th style="width: 60px;">Color</th>
                  <th>Variable</th>
                  <th>Values / Formats</th>
                  <th>Role &amp; Application</th>
                </tr>
              </thead>
              <tbody v-for="group in colorGroups" :key="group.title">
                <tr class="color-group-header-row">
                  <td colspan="4" class="color-group-title">{{ group.title }}</td>
                </tr>
                <tr v-for="color in group.colors" :key="color.name" class="color-row" @click="copyToClipboard('var(--' + color.name + ')', color.name)">
                  <td class="color-cell">
                    <div class="color-preview-circle" :style="{ backgroundColor: color.hex }"></div>
                  </td>
                  <td class="color-name-cell">
                    <code>--{{ color.name }}</code>
                  </td>
                  <td class="color-formats-cell">
                    <div class="format-val"><code>hex: {{ color.hex }}</code></div>
                    <div class="format-val"><code>oklch: {{ color.oklch }}</code></div>
                    <div class="format-val"><code>rgba: {{ color.rgba }}</code></div>
                  </td>
                  <td class="color-role-cell">{{ color.role }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>

        <!-- Depth, Spacing & Radius Section -->
        <section id="depth" class="brand-section">
          <div class="section-meta">
            <span class="section-num">05</span>
            <h2 class="section-title">Depth, Spacing &amp; Radius</h2>
            <p class="section-desc">
              Sema represents visual hierarchy by layered background darkness and border boundaries, rather than drop
              shadows.
            </p>
          </div>

          <div class="depth-grid">
            <!-- Elevation Stack -->
            <div class="depth-column">
              <h3 class="depth-section-title">Elevation Levels</h3>
              <div class="elevation-stack">
                <div class="elevation-level" style="background:#131110;">
                  <div class="el-name">Page Canvas (--bg)</div>
                  <div class="el-value">#131110</div>
                  <div class="el-role">The main base viewport. Sidebars and status bars.</div>
                </div>
                <div class="elevation-level" style="background:#181512; border-left: 3px solid var(--border-focus);">
                  <div class="el-name">Surface (--bg-elevated)</div>
                  <div class="el-value">#181512</div>
                  <div class="el-role">Toolbars, feature cards, and modals. Sits directly above the canvas.</div>
                </div>
                <div class="elevation-level" style="background:#1c1916; border-left: 3px solid var(--gold);">
                  <div class="el-name">Editor Focus (--bg-editor)</div>
                  <div class="el-value">#1c1916</div>
                  <div class="el-role">Code input areas. Active indicators use 3px solid gold border.</div>
                </div>
                <div class="elevation-level" style="background:#0f0d0c;">
                  <div class="el-name">Output Panel (--bg-output)</div>
                  <div class="el-value">#0f0d0c</div>
                  <div class="el-role">Deepest visual layer. Embedded execution consoles.</div>
                </div>
              </div>
            </div>

            <!-- Scales (Radius & Spacing) -->
            <div class="depth-column">
              <h3 class="depth-section-title">Border Radius Scale</h3>
              <div class="radius-row">
                <div class="radius-sample">
                  <div class="radius-box" style="border-radius: 3px;"></div>
                  <div class="radius-label">sm 3px</div>
                </div>
                <div class="radius-sample">
                  <div class="radius-box" style="border-radius: 4px;"></div>
                  <div class="radius-label">md 4px</div>
                </div>
                <div class="radius-sample">
                  <div class="radius-box" style="border-radius: 6px;"></div>
                  <div class="radius-label">lg 6px</div>
                </div>
                <div class="radius-sample">
                  <div class="radius-box" style="border-radius: 8px;"></div>
                  <div class="radius-label">xl 8px</div>
                </div>
                <div class="radius-sample">
                  <div class="radius-box" style="border-radius: 20px;"></div>
                  <div class="radius-label">pill 20px</div>
                </div>
              </div>

              <h3 class="depth-section-title" style="margin-top: 2.5rem;">Spacing Scale</h3>
              <div class="spacing-list">
                <div class="spacing-row"><span class="spacing-label">xs (4px)</span>
                  <div class="spacing-bar" style="width:4px;"></div>
                </div>
                <div class="spacing-row"><span class="spacing-label">sm (8px)</span>
                  <div class="spacing-bar" style="width:8px;"></div>
                </div>
                <div class="spacing-row"><span class="spacing-label">md (16px)</span>
                  <div class="spacing-bar" style="width:16px;"></div>
                </div>
                <div class="spacing-row"><span class="spacing-label">lg (24px)</span>
                  <div class="spacing-bar" style="width:24px;"></div>
                </div>
                <div class="spacing-row"><span class="spacing-label">xl (32px)</span>
                  <div class="spacing-bar" style="width:32px;"></div>
                </div>
                <div class="spacing-row"><span class="spacing-label">2xl (48px)</span>
                  <div class="spacing-bar" style="width:48px;"></div>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- UI Kit Components Section -->
        <section id="uikit" class="brand-section">
          <div class="section-meta">
            <span class="section-num">06</span>
            <h2 class="section-title">UI Kit Components</h2>
            <p class="section-desc">
              Reference implementations of common user interface elements rendered using active system tokens.
            </p>
          </div>

          <div class="components-grid">
            <!-- Buttons Group -->
            <div class="components-group">
              <h3>Buttons &amp; Actions</h3>
              <div class="component-showcase-row">
                <span class="component-ref-label">button-primary</span>
                <div class="component-preview-cell">
                  <button class="btn-primary">Get Started</button>
                  <button class="btn-primary" style="opacity: 0.8;">Hover</button>
                  <button class="btn-primary" disabled>Disabled</button>
                </div>
              </div>

              <div class="component-showcase-row">
                <span class="component-ref-label">button-secondary</span>
                <div class="component-preview-cell">
                  <button class="btn-secondary">GitHub Repository</button>
                  <button class="btn-secondary active">Hover State</button>
                </div>
              </div>

              <div class="component-showcase-row">
                <span class="component-ref-label">button-run</span>
                <div class="component-preview-cell">
                  <button class="btn-run">Run <span class="kbd">⌘↵</span></button>
                </div>
              </div>

              <div class="component-showcase-row">
                <span class="component-ref-label">button-ghost</span>
                <div class="component-preview-cell">
                  <button class="btn-ghost">Default</button>
                  <button class="btn-ghost" style="color: var(--text-primary);">Hover</button>
                  <button class="btn-ghost active">Active</button>
                </div>
              </div>

              <div class="component-showcase-row">
                <span class="component-ref-label">button-pill</span>
                <div class="component-preview-cell">
                  <button class="btn-pill">+ Add Cell</button>
                  <button class="btn-pill hover-pill">+ Add Cell</button>
                </div>
              </div>

              <div class="component-showcase-row">
                <span class="component-ref-label">button-debug</span>
                <div class="component-preview-cell">
                  <div class="debug-toolbar">
                    <button class="btn-debug" title="Continue">▶</button>
                    <button class="btn-debug" title="Step Over">⏭</button>
                    <button class="btn-debug" title="Step Into">↓</button>
                    <button class="btn-debug danger" title="Stop">⬛</button>
                  </div>
                </div>
              </div>
            </div>

            <!-- Cards and Tags Group -->
            <div class="components-group">
              <h3>Cards, Tags &amp; Notebook Elements</h3>

              <!-- Feature Card -->
              <div class="component-showcase-row flex-column">
                <span class="component-ref-label">card-feature</span>
                <div class="card-feature">
                  <h4>Tail-Call Optimized</h4>
                  <p>Trampoline-based tree evaluator. Execute deeply recursive functions without heap exhaustion.</p>
                </div>
              </div>

              <!-- Tags -->
              <div class="component-showcase-row">
                <span class="component-ref-label">tags (provider &amp; function)</span>
                <div class="component-preview-cell">
                  <span class="tag-provider">Anthropic</span>
                  <span class="tag-provider">OpenAI</span>
                  <span class="tag-fn">string/split</span>
                  <span class="tag-fn">llm/complete</span>
                </div>
              </div>

              <!-- Notebook Cells -->
              <div class="component-showcase-row flex-column">
                <span class="component-ref-label">notebook-cell (active)</span>
                <div class="nb-cell active">
                  <div class="nb-cell-num">[1]</div>
                  <div class="nb-cell-editor">
                    <span class="syn-p-paren">(</span><span class="syn-p-keyword">defagent</span> coder <span
                    class="syn-p-paren">{</span><span class="syn-p-kwlit">:model</span> <span class="syn-p-string">"claude-3-5"</span><span
                    class="syn-p-paren">})</span>
                  </div>
                </div>
              </div>

              <div class="component-showcase-row flex-column">
                <span class="component-ref-label">notebook-cell (stale / inactive)</span>
                <div class="nb-cell stale">
                  <div class="nb-cell-num">[2*]</div>
                  <div class="nb-cell-editor">
                    <span class="syn-p-paren">(</span>agent/run coder <span
                    class="syn-p-string">"Hello world"</span><span class="syn-p-paren">)</span> <span
                    class="syn-p-comment">; stale output</span>
                  </div>
                </div>
              </div>

              <!-- Output Panel -->
              <div class="component-showcase-row flex-column">
                <span class="component-ref-label">output-panel</span>
                <div class="output-panel">
                  <div class="out-stdout">Executing compilation... done.</div>
                  <div class="out-value">42</div>
                  <div class="out-error">Error: symbol 'x' is unbound in environment</div>
                  <div class="out-meta">14ms · $0.00004</div>
                </div>
              </div>
            </div>
          </div>

          <!-- Comparison Code Panes Block -->
          <div class="comparison-panes-showcase" style="margin-top: 3.5rem; border-top: 1px dashed var(--border); padding-top: 2.5rem;">
            <h3 style="font-family: 'Cormorant', Georgia, serif; font-size: 1.5rem; color: #e9e3d6; font-weight: 400; margin-bottom: 0.5rem;">Comparison Code Panes</h3>
            <p class="section-desc" style="margin-bottom: 2rem;">
              Code specimens rendering on the website. Features two variations: a standard/accentless pane (used for standard Python or foreign SDKs) and an accented, active gold glow pane (used for Sema native files).
            </p>

            <div class="panes-compare-grid" style="display: grid; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr)); gap: 2rem; align-items: start;">
              <!-- Standard Pane (Python) -->
              <div class="pane python">
                <div class="pane-head">
                  <span class="t">agent.py — Python + SDK</span>
                  <span class="n">you write the machinery</span>
                </div>
                <pre><code><span class="syn-p-keyword">import</span> anthropic

client = anthropic.Anthropic()
resp = client.messages.create(
    model="claude-3-5-sonnet",
    max_tokens=1024,
    messages=[{"role": "user", "content": "hello"}]
)</code></pre>
                <div class="pane-foot">
                  Standard pane container using neutral borders (<code>#2b2620</code>) and standard raised background (<code>#181512</code>) without glows.
                </div>
              </div>

              <!-- Accented Pane (Sema) -->
              <div class="pane sema">
                <div class="pane-head">
                  <span class="t">agent.sema — Sema</span>
                  <span class="n">the machinery is the language</span>
                </div>
                <pre><code>(<span class="syn-p-keyword">defagent</span> <span class="syn-p-kwlit">coder</span>
  {<span class="syn-p-kwlit">:system</span>    <span class="syn-p-string">"You are a coding assistant."</span>
   <span class="syn-p-kwlit">:tools</span>     [read-file run-command]
   <span class="syn-p-kwlit">:max-turns</span> 10})</code></pre>
                <div class="pane-foot">
                  Accented active container using a gold border (<code>var(--gold-line)</code>) and a subtle gold ambient shadow glow.
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- Syntax highlighting Section -->
        <section id="syntax" class="brand-section">
          <div class="section-meta">
            <span class="section-num">07</span>
            <h2 class="section-title">Syntax Highlighting</h2>
            <p class="section-desc">
              Comparison between marketing/docs pages and the interactive playground environment, detailing target token
              styles.
            </p>
          </div>

          <div class="syntax-columns">
            <div class="syntax-col">
              <h4>Website Style</h4>
              <div class="syntax-code">
                <span class="syn-w-comment">;; Define a tool the LLM can call</span>
                <span class="syn-w-paren">(</span><span class="syn-w-keyword">deftool</span> get-weather
                <span class="syn-w-string">"Get weather for a city"</span>
                <span class="syn-w-paren">{</span><span class="syn-w-kwlit">:city</span> <span
                class="syn-w-paren">{</span><span class="syn-w-kwlit">:type</span> <span
                class="syn-w-kwlit">:string</span><span class="syn-w-paren">}}</span>
                <span class="syn-w-paren">(</span><span class="syn-w-keyword">lambda</span> <span
                class="syn-w-paren">(</span>city<span class="syn-w-paren">)</span>
                <span class="syn-w-paren">(</span><span class="syn-w-builtin">format</span> <span class="syn-w-string">"~a: 22°C, sunny"</span>
                city<span class="syn-w-paren">)))</span>
              </div>
            </div>

            <div class="syntax-col">
              <h4>Playground Style</h4>
              <div class="syntax-code">
                <span class="syn-p-comment">;; Define a tool the LLM can call</span>
                <span class="syn-p-paren">(</span><span class="syn-p-keyword">deftool</span> get-weather
                <span class="syn-p-string">"Get weather for a city"</span>
                <span class="syn-p-paren">{</span><span class="syn-p-kwlit">:city</span> <span
                class="syn-p-paren">{</span><span class="syn-p-kwlit">:type</span> <span
                class="syn-p-kwlit">:string</span><span class="syn-p-paren">}}</span>
                <span class="syn-p-paren">(</span><span class="syn-p-keyword">lambda</span> <span
                class="syn-p-paren">(</span>city<span class="syn-p-paren">)</span>
                <span class="syn-p-paren">(</span>format <span class="syn-p-string">"~a: 22°C, sunny"</span> city<span
                class="syn-p-paren">)))</span>
              </div>
            </div>
          </div>

          <div class="table-wrap" style="margin-top: 2rem;">
            <table>
              <thead>
              <tr>
                <th>Token</th>
                <th>Website</th>
                <th>Playground</th>
                <th>Canonical Target</th>
                <th>Convergence Rationale</th>
              </tr>
              </thead>
              <tbody>
              <tr>
                <td>Comment</td>
                <td><span class="color-dot" style="background:#5a5a4a;"></span>#5a5a4a</td>
                <td><span class="color-dot" style="background:var(--text-tertiary);"></span>#6b6354</td>
                <td><span class="color-dot" style="background:var(--text-tertiary);"></span>#6b6354</td>
                <td>Unified with tertiary typography scale</td>
              </tr>
              <tr>
                <td>Keyword</td>
                <td><span class="color-dot" style="background:#d4a052;"></span>#d4a052</td>
                <td><span class="color-dot" style="background:var(--gold);"></span>#c8a855</td>
                <td><span class="color-dot" style="background:var(--gold);"></span>#c8a855</td>
                <td>Use standard gold variable for core special forms</td>
              </tr>
              <tr>
                <td>String</td>
                <td><span class="color-dot" style="background:#8aaa6a;"></span>#8aaa6a</td>
                <td><span class="color-dot" style="background:#a8c47a;"></span>#a8c47a</td>
                <td><span class="color-dot" style="background:#a8c47a;"></span>#a8c47a</td>
                <td>Brighter green provides better contrast on warm darks</td>
              </tr>
              <tr>
                <td>Number</td>
                <td><span class="color-dot" style="background:#d08a60;"></span>#d08a60</td>
                <td><span class="color-dot" style="background:#d19a66;"></span>#d19a66</td>
                <td><span class="color-dot" style="background:#d19a66;"></span>#d19a66</td>
                <td>Consistent with modern dark editor configurations</td>
              </tr>
              <tr>
                <td>Keyword Lit</td>
                <td><span class="color-dot" style="background:#c89050;"></span>#c89050</td>
                <td><span class="color-dot" style="background:#7aacb8;"></span>#7aacb8</td>
                <td><span class="color-dot" style="background:#7aacb8;"></span>#7aacb8</td>
                <td>Muted blue distinguishes :symbols and options from core code</td>
              </tr>
              <tr>
                <td>Paren</td>
                <td><span class="color-dot" style="background:#444438;"></span>#444438</td>
                <td><span class="color-dot" style="background:#6a6258;"></span>#6a6258</td>
                <td><span class="color-dot" style="background:#6a6258;"></span>#6a6258</td>
                <td>Clear layout structure without distracting visual clutter</td>
              </tr>
              </tbody>
            </table>
          </div>
        </section>

        <!-- Icon Showcase Section -->
        <section id="icons" class="brand-section">
          <div class="section-meta">
            <span class="section-num">08</span>
            <h2 class="section-title">Icon Showcase &amp; Spec</h2>
            <p class="section-desc">
              Comparison between the old serif "S" assets and the new parenthesized <code>(s)</code> or
              <code>(sema)</code> specifications, showing custom indicators for file types.
            </p>
          </div>

          <div class="icons-comparison-list">
            <!-- Icon Pair 1: Favicon -->
            <div class="icon-pair-card">
              <div class="pair-meta">
                <h4>Favicon / Canonical Mark (32×32)</h4>
                <p>Rounded square logo badge. Left: Old Georgia serif style. Right: New parenthesized brand mark.</p>
              </div>
              <div class="pair-renders">
                <div class="render-item">
                  <div class="render-box" v-html="icons.oldFavicon"></div>
                  <button class="btn-copy-mini" @click="copyIcon('oldFavicon')">Copy SVG</button>
                </div>
                <div class="render-item">
                  <div class="render-box" v-html="icons.newFavicon"></div>
                  <button class="btn-copy-mini" @click="copyIcon('newFavicon')">Copy SVG</button>
                </div>
              </div>
            </div>

            <!-- Icon Pair 2: .sema file -->
            <div class="icon-pair-card">
              <div class="pair-meta">
                <h4>Sema Source File (.sema, 16×16)</h4>
                <p>Standard file association icon. Left: Old Georgia Serif 'S'. Right: New parenthesized 's' indicating
                  Lisp origins.</p>
              </div>
              <div class="pair-renders">
                <div class="render-item">
                  <div class="render-box" v-html="icons.oldSema"></div>
                  <button class="btn-copy-mini" @click="copyIcon('oldSema')">Copy SVG</button>
                </div>
                <div class="render-item">
                  <div class="render-box" v-html="icons.newSema"></div>
                  <button class="btn-copy-mini" @click="copyIcon('newSema')">Copy SVG</button>
                </div>
              </div>
            </div>

            <!-- Icon Pair 3: .semac compiled file -->
            <div class="icon-pair-card">
              <div class="pair-meta">
                <h4>Sema Compiled Bytecode (.semac, 16×16)</h4>
                <p>Compiled runtime artifact. Bottom-right modifier is a solid gold circle containing a dark 'c'. Base
                  logo is grayed to suggest a compiled asset.</p>
              </div>
              <div class="pair-renders">
                <div class="render-item">
                  <div class="render-box" v-html="icons.oldSemac"></div>
                  <button class="btn-copy-mini" @click="copyIcon('oldSemac')">Copy SVG</button>
                </div>
                <div class="render-item">
                  <div class="render-box" v-html="icons.newSemac"></div>
                  <button class="btn-copy-mini" @click="copyIcon('newSemac')">Copy SVG</button>
                </div>
              </div>
            </div>

            <!-- Icon Pair 4: .sema-nb Notebook file -->
            <div class="icon-pair-card">
              <div class="pair-meta">
                <h4>Sema Notebook File (.sema-nb, 16×16)</h4>
                <p>Computational notebook layout file. Bottom-right modifier is a clean gold double-line cell preview
                  layout outline.</p>
              </div>
              <div class="pair-renders">
                <div class="render-item">
                  <div class="render-box" v-html="icons.oldSemaNotebook"></div>
                  <button class="btn-copy-mini" @click="copyIcon('oldSemaNotebook')">Copy SVG</button>
                </div>
                <div class="render-item">
                  <div class="render-box" v-html="icons.newSemaNotebook"></div>
                  <button class="btn-copy-mini" @click="copyIcon('newSemaNotebook')">Copy SVG</button>
                </div>
              </div>
            </div>

            <!-- Additional Notebook Variants -->
            <div class="icon-pair-card">
              <div class="pair-meta">
                <h4>Notebook cells &amp; execution variants</h4>
                <p>Alternative outline representations of active notebook cells and play/runtime conditions.</p>
              </div>
              <div class="pair-renders">
                <div class="render-item">
                  <div class="render-box" v-html="icons.oldSemaNotebookCells"></div>
                  <button class="btn-copy-mini" @click="copyIcon('oldSemaNotebookCells')">Copy</button>
                </div>
                <div class="render-item">
                  <div class="render-box" v-html="icons.newSemaNotebookCells"></div>
                  <button class="btn-copy-mini" @click="copyIcon('newSemaNotebookCells')">Copy</button>
                </div>
                <div class="render-item">
                  <div class="render-box" v-html="icons.oldSemaNotebookPlay"></div>
                  <button class="btn-copy-mini" @click="copyIcon('oldSemaNotebookPlay')">Copy</button>
                </div>
                <div class="render-item">
                  <div class="render-box" v-html="icons.newSemaNotebookPlay"></div>
                  <button class="btn-copy-mini" @click="copyIcon('newSemaNotebookPlay')">Copy</button>
                </div>
              </div>
            </div>

            <!-- Transparent File Explorer Icons & Filetree Showcase -->
            <div class="icon-pair-card file-variants-card" style="grid-template-columns: 1.2fr 1fr; gap: 3rem;">
              <div class="pair-meta">
                <h4>Transparent File Explorer Icons</h4>
                <p>
                  High-fidelity, transparent-background variants optimized for file trees in IDEs (like VS Code, Zed, and IntelliJ). Each ships a light and dark variant — the glyph flips ink so it stays legible on any editor theme. Shown light / dark side by side below.
                </p>
                <div class="file-icons-horizontal" style="display: flex; gap: 2rem; margin-top: 1.5rem; flex-wrap: wrap;">
                  <div class="file-icon-showcase-item" style="display: flex; flex-direction: column; align-items: center; gap: 0.6rem;">
                    <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: var(--text-tertiary);">.sema</span>
                    <div style="display: flex; gap: 0.4rem;">
                      <div style="display: flex; flex-direction: column; align-items: center; gap: 0.3rem;">
                        <div class="file-icon-box" style="background-color: #f4f4f5; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.fileSemaLight"></div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.55rem; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.08em;">Light</span>
                      </div>
                      <div style="display: flex; flex-direction: column; align-items: center; gap: 0.3rem;">
                        <div class="file-icon-box" style="background-color: #131110; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.fileSema"></div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.55rem; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.08em;">Dark</span>
                      </div>
                    </div>
                    <button class="btn-copy-mini" @click="copyIcon('fileSema')">Copy SVG</button>
                  </div>
                  <div class="file-icon-showcase-item" style="display: flex; flex-direction: column; align-items: center; gap: 0.6rem;">
                    <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: var(--text-tertiary);">.semac</span>
                    <div style="display: flex; gap: 0.4rem;">
                      <div style="display: flex; flex-direction: column; align-items: center; gap: 0.3rem;">
                        <div class="file-icon-box" style="background-color: #f4f4f5; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.fileSemac"></div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.55rem; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.08em;">Light</span>
                      </div>
                      <div style="display: flex; flex-direction: column; align-items: center; gap: 0.3rem;">
                        <div class="file-icon-box" style="background-color: #131110; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.fileSemac"></div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.55rem; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.08em;">Dark</span>
                      </div>
                    </div>
                    <button class="btn-copy-mini" @click="copyIcon('fileSemac')">Copy SVG</button>
                  </div>
                  <div class="file-icon-showcase-item" style="display: flex; flex-direction: column; align-items: center; gap: 0.6rem;">
                    <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: var(--text-tertiary);">.sema-nb</span>
                    <div style="display: flex; gap: 0.4rem;">
                      <div style="display: flex; flex-direction: column; align-items: center; gap: 0.3rem;">
                        <div class="file-icon-box" style="background-color: #f4f4f5; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.fileSemaNotebookLight"></div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.55rem; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.08em;">Light</span>
                      </div>
                      <div style="display: flex; flex-direction: column; align-items: center; gap: 0.3rem;">
                        <div class="file-icon-box" style="background-color: #131110; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.fileSemaNotebook"></div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.55rem; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.08em;">Dark</span>
                      </div>
                    </div>
                    <button class="btn-copy-mini" @click="copyIcon('fileSemaNotebook')">Copy SVG</button>
                  </div>
                </div>
              </div>
              <div class="filetree-showcase-side" style="display: flex; flex-direction: column; gap: 0.75rem;">
                <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.7rem; color: var(--gold); text-transform: uppercase; letter-spacing: 0.05em; display: block; margin-bottom: 0.25rem;">IDE Filetree Showcase</span>
                <div class="filetree-mockup" style="border: 1px solid var(--border); border-radius: 6px; overflow: hidden; background-color: #0f0d0c; font-family: system-ui, sans-serif; padding-bottom: 0.5rem;">
                  <div class="mockup-tab" style="background-color: #181512; border-bottom: 1px solid var(--border); padding: 0.5rem 1rem; display: flex; justify-content: space-between; align-items: center;">
                    <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.05em;">EXPLORER: SEMA-PROJECT</span>
                    <div style="display: flex; gap: 3px;">
                      <span style="width: 5px; height: 5px; border-radius: 50%; background-color: var(--text-tertiary); display: inline-block;"></span>
                      <span style="width: 5px; height: 5px; border-radius: 50%; background-color: var(--text-tertiary); display: inline-block;"></span>
                      <span style="width: 5px; height: 5px; border-radius: 50%; background-color: var(--text-tertiary); display: inline-block;"></span>
                    </div>
                  </div>
                  <div class="tree-nodes" style="padding: 0.5rem; font-size: 0.8rem; display: flex; flex-direction: column; gap: 2px;">
                    <div class="tree-node branch" style="display: flex; align-items: center; gap: 0.4rem; color: var(--text-primary); padding: 0.2rem 0.5rem;">
                      <span style="color: var(--text-tertiary); font-size: 0.7rem; transform: rotate(90deg); display: inline-block;">›</span>
                      <span>📂</span>
                      <span style="font-weight: 500;">crates</span>
                    </div>
                    <div class="tree-node branch" style="display: flex; align-items: center; gap: 0.4rem; color: var(--text-primary); padding: 0.2rem 0.5rem 0.2rem 1.25rem;">
                      <span style="color: var(--text-tertiary); font-size: 0.7rem; transform: rotate(90deg); display: inline-block;">›</span>
                      <span>📂</span>
                      <span style="font-weight: 500;">examples</span>
                    </div>
                    <div class="tree-node leaf" style="display: flex; align-items: center; gap: 0.5rem; color: var(--text-secondary); padding: 0.2rem 0.5rem 0.2rem 2.25rem;">
                      <span class="file-icon" style="width: 16px; height: 16px; display: inline-flex; align-items: center; justify-content: center;" v-html="icons.fileSema"></span>
                      <span>hello.sema</span>
                    </div>
                    <div class="tree-node leaf" style="display: flex; align-items: center; gap: 0.5rem; color: var(--text-secondary); padding: 0.2rem 0.5rem 0.2rem 2.25rem;">
                      <span class="file-icon" style="width: 16px; height: 16px; display: inline-flex; align-items: center; justify-content: center;" v-html="icons.fileSema"></span>
                      <span>agent.sema</span>
                    </div>
                    <div class="tree-node branch" style="display: flex; align-items: center; gap: 0.4rem; color: var(--text-primary); padding: 0.2rem 0.5rem 0.2rem 1.25rem;">
                      <span style="color: var(--text-tertiary); font-size: 0.7rem; transform: rotate(90deg); display: inline-block;">›</span>
                      <span>📂</span>
                      <span style="font-weight: 500;">notebooks</span>
                    </div>
                    <div class="tree-node leaf" style="display: flex; align-items: center; gap: 0.5rem; color: var(--text-secondary); padding: 0.2rem 0.5rem 0.2rem 2.25rem;">
                      <span class="file-icon" style="width: 16px; height: 16px; display: inline-flex; align-items: center; justify-content: center;" v-html="icons.fileSemaNotebook"></span>
                      <span>research.sema-nb</span>
                    </div>
                    <div class="tree-node branch" style="display: flex; align-items: center; gap: 0.4rem; color: var(--text-primary); padding: 0.2rem 0.5rem 0.2rem 1.25rem;">
                      <span style="color: var(--text-tertiary); font-size: 0.7rem; transform: rotate(90deg); display: inline-block;">›</span>
                      <span>📂</span>
                      <span style="font-weight: 500;">target</span>
                    </div>
                    <div class="tree-node leaf active-node" style="display: flex; align-items: center; gap: 0.5rem; color: var(--text-primary); padding: 0.2rem 0.5rem 0.2rem 2.25rem; background-color: var(--gold-glow); border-left: 2px solid var(--gold);">
                      <span class="file-icon" style="width: 16px; height: 16px; display: inline-flex; align-items: center; justify-content: center;" v-html="icons.fileSemac"></span>
                      <span>hello.semac</span>
                    </div>
                  </div>
                </div>
              </div>
            </div>

            <!-- New-style notebook icon candidates (parked for later use) -->
            <div class="icon-pair-card">
              <div class="pair-meta">
                <h4>Notebook Icon Candidates <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.6rem; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.08em; border: 1px solid var(--border); border-radius: 10px; padding: 0.1rem 0.5rem; margin-left: 0.4rem;">parked</span></h4>
                <p>
                  Exploratory notebook marks in the new transparent <code>(s)</code> style — not yet shipped in any product, kept here for future use (notebook actions, tool-window icons). Dark-theme glyph shown; pair with a light variant when wired up.
                </p>
                <div style="display: flex; gap: 1.75rem; margin-top: 1.5rem; flex-wrap: wrap;">
                  <div style="display: flex; flex-direction: column; align-items: center; gap: 0.5rem;">
                    <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.6rem; color: var(--text-tertiary);">A · Page</span>
                    <div class="file-icon-box" style="background-color: #131110; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.nbPageBadge"></div>
                    <button class="btn-copy-mini" @click="copyIcon('nbPageBadge')">Copy</button>
                  </div>
                  <div style="display: flex; flex-direction: column; align-items: center; gap: 0.5rem;">
                    <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.6rem; color: var(--text-tertiary);">B · Play</span>
                    <div class="file-icon-box" style="background-color: #131110; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.nbPlayBadge"></div>
                    <button class="btn-copy-mini" @click="copyIcon('nbPlayBadge')">Copy</button>
                  </div>
                  <div style="display: flex; flex-direction: column; align-items: center; gap: 0.5rem;">
                    <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.6rem; color: var(--text-tertiary);">C · Stacked</span>
                    <div class="file-icon-box" style="background-color: #131110; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.nbStacked"></div>
                    <button class="btn-copy-mini" @click="copyIcon('nbStacked')">Copy</button>
                  </div>
                  <div style="display: flex; flex-direction: column; align-items: center; gap: 0.5rem;">
                    <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.6rem; color: var(--text-tertiary);">D · Full page</span>
                    <div class="file-icon-box" style="background-color: #131110; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.nbFullPage"></div>
                    <button class="btn-copy-mini" @click="copyIcon('nbFullPage')">Copy</button>
                  </div>
                  <div style="display: flex; flex-direction: column; align-items: center; gap: 0.5rem;">
                    <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.6rem; color: var(--text-tertiary);">E · Cell lines</span>
                    <div class="file-icon-box" style="background-color: #131110; border: 1px solid var(--border); border-radius: 6px; width: 56px; height: 56px; display: flex; align-items: center; justify-content: center;" v-html="icons.nbCellLines"></div>
                    <button class="btn-copy-mini" @click="copyIcon('nbCellLines')">Copy</button>
                  </div>
                </div>
              </div>
            </div>

            <!-- Cross-Platform Guidelines -->
            <div class="guidelines-card" style="margin-top: 3rem;">
              <h3 class="guidelines-title">Cross-Platform App Icon Guidelines</h3>
              <p class="guidelines-desc">
                When packaging Sema integrations, extensions, or applications for various marketplaces, adhere to the following platform specifications:
              </p>
              
              <div class="guidelines-grid">
                <div class="guide-item">
                  <h5>App Store (iOS / macOS)</h5>
                  <ul>
                    <li><strong>iOS</strong>: Must be flat square PNG (1024x1024). Transparency is forbidden; the system dynamically masks corners using a squircle.</li>
                    <li><strong>macOS</strong>: Transparency and subtle drop shadows are permitted. Recommends a tilted, dimensional sheet style.</li>
                  </ul>
                </div>
                <div class="guide-item">
                  <h5>Google Play (Android)</h5>
                  <ul>
                    <li><strong>Adaptive Icons</strong>: Requires separate background (512x512, safe zone 66%) and foreground layers to allow system-level parallax.</li>
                    <li><strong>Masking</strong>: Flat square uploads are automatically masked to circles, squircles, or rounded rects.</li>
                  </ul>
                </div>
                <div class="guide-item">
                  <h5>VS Code Extension Marketplace</h5>
                  <ul>
                    <li><strong>Proportions</strong>: 128x128px (or larger square e.g., 256x256). Transparent backgrounds are highly recommended.</li>
                    <li><strong>Contrast</strong>: Design must remain highly visible against both the default light and dark theme header colors.</li>
                  </ul>
                </div>
                <div class="guide-item">
                  <h5>JetBrains Marketplace</h5>
                  <ul>
                    <li><strong>Plugin Icon</strong>: 40x40px (or 80x80px for high-DPI). Vector SVG format is preferred.</li>
                    <li><strong>Style</strong>: Simple, high-contrast flat shapes. Avoid tiny text or highly detailed ornaments.</li>
                  </ul>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- Project Inventory Section -->
        <section id="inventory" class="brand-section">
          <div class="section-meta">
            <span class="section-num">09</span>
            <h2 class="section-title">Project Inventory</h2>
            <p class="section-desc">
              List of public domains, repositories, packages, and internal crates within the Sema workspace.
            </p>
          </div>

          <h3 class="inventory-subheading">Public Surfaces</h3>
          <div class="table-wrap">
            <table>
              <thead>
              <tr>
                <th>Surface</th>
                <th>URL / Identifier</th>
                <th>Role &amp; Deployment</th>
              </tr>
              </thead>
              <tbody>
              <tr>
                <td class="inv-name">Website</td>
                <td><a href="https://sema-lang.com" target="_blank">sema-lang.com</a></td>
                <td>Marketing site and documentation (VitePress, deployed via Vercel)</td>
              </tr>
              <tr>
                <td class="inv-name">Playground</td>
                <td><a href="https://sema.run" target="_blank">sema.run</a></td>
                <td>Interactive browser REPL and debugger (vanilla JS, deployed via Vercel)</td>
              </tr>
              <tr>
                <td class="inv-name">Package Registry</td>
                <td>pkg.sema-lang.com</td>
                <td>Community package registry API (Rust, Axum, SQLite)</td>
              </tr>
              <tr>
                <td class="inv-name">GitHub Organization</td>
                <td><a href="https://github.com/sema-lisp" target="_blank">github.com/sema-lisp</a></td>
                <td>Org hosting the loosely-coupled components (editor plugins, grammar, UI library)</td>
              </tr>
              <tr>
                <td class="inv-name">GitHub Repository</td>
                <td><a href="https://github.com/sema-lisp/sema" target="_blank">github.com/sema-lisp/sema</a></td>
                <td>Primary codebase repository (Cargo multi-crate workspace)</td>
              </tr>
              <tr>
                <td class="inv-name">Editor Plugins</td>
                <td><a href="https://github.com/sema-lisp" target="_blank">sema-lisp/&#123;vscode,zed,intellij,emacs,helix,sublime&#125;-sema, sema.&#123;vim,nvim&#125;</a></td>
                <td>Per-editor plugin repos (VS Code, Zed, IntelliJ, Emacs, Helix, Sublime, Vim, Neovim)</td>
              </tr>
              <tr>
                <td class="inv-name">Tree-sitter Grammar</td>
                <td><a href="https://github.com/sema-lisp/tree-sitter-sema" target="_blank">github.com/sema-lisp/tree-sitter-sema</a></td>
                <td>Shared grammar consumed by Zed, Helix, and Neovim (pinned commit/tag)</td>
              </tr>
              <tr>
                <td class="inv-name">UI Library</td>
                <td><a href="https://github.com/sema-lisp/ui" target="_blank">github.com/sema-lisp/ui</a> · <code>@sema-lang/ui</code></td>
                <td>Lit-based web components (published to npm), consumed by the site, notebook, and playground</td>
              </tr>
              <tr>
                <td class="inv-name">Homebrew Crate</td>
                <td><code>helgesverre/tap/sema-lang</code></td>
                <td>Formula for standard macOS command-line installation</td>
              </tr>
              <tr>
                <td class="inv-name">Cargo Crate</td>
                <td><code>sema-lang</code></td>
                <td>crates.io package hosting the main CLI executable and local REPL</td>
              </tr>
              </tbody>
            </table>
          </div>

          <h3 class="inventory-subheading" style="margin-top: 2.5rem;">Crate Workspace Structure</h3>
          <div class="table-wrap">
            <table>
              <thead>
              <tr>
                <th>Crate Name</th>
                <th>Responsibility &amp; Architectural Layer</th>
              </tr>
              </thead>
              <tbody>
              <tr>
                <td class="inv-name">sema-core</td>
                <td>NaN-boxed Value primitive, Environment (Rc+RefCell), Env callbacks, thread-local VFS.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-reader</td>
                <td>Lexer and S-Expression reader. Handles f-strings, short lambdas, and regex literals.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-vm</td>
                <td>Bytecode compiler and stack VM — the sole evaluator. Lowerer, optimizers, stack resolution, inline cache.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-eval</td>
                <td>Interpreter driver: VM-native macro expansion, module load/import system, prelude, eval/call callback wiring.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-stdlib</td>
                <td>Native stdlib functions (450+ total), higher-order dispatch loops.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-llm</td>
                <td>Model traits, Anthropic/OpenAI/Gemini/Ollama clients, pricing database, local fallback.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-lsp</td>
                <td>LSP Language Server (tower-lsp). Completions, definitions, hover metadata.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-dap</td>
                <td>Debug Adapter Protocol server. Frame inspector, stepped walk, breakpoint registry.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-fmt</td>
                <td>Custom code formatter and formatter CLI.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-notebook</td>
                <td>Jupyter-inspired notebook: <code>.sema-nb</code> format, shared-env evaluation engine, HTTP server, embedded UI.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-docs</td>
                <td>Canonical structured docs for builtins/special forms; generates the JSON index used by the LSP and REPL.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-mcp</td>
                <td>Model Context Protocol server exposing Sema evaluation and tooling to agents.</td>
              </tr>
              <tr>
                <td class="inv-name">sema-wasm</td>
                <td>Wasm bindings for parsing and running Sema code inside the client playground.</td>
              </tr>
              <tr>
                <td class="inv-name">sema</td>
                <td>Main CLI entrypoint. Invokes lsp, dap, REPL, run file, compile bytecode.</td>
              </tr>
              </tbody>
            </table>
          </div>
        </section>

        <!-- Visual Architecture Rules Section -->
        <section id="rules" class="brand-section">
          <div class="section-meta">
            <span class="section-num">10</span>
            <h2 class="section-title">Visual Architecture Rules</h2>
            <p class="section-desc">
              Governing guidelines for constructing extensions to documentation, playground components, or editor
              integrations.
            </p>
          </div>

          <div class="rules-grid">
            <div class="rule-card">
              <h4>1. No Drop Shadows</h4>
              <p>Represent visual depth entirely through color values. Higher levels use lighter warm-neutrals (e.g.
                <code>#181512</code>), while background canvas layers use deeper tones (e.g. <code>#131110</code>).</p>
            </div>
            <div class="rule-card">
              <h4>2. Rigid Font Boundaries</h4>
              <p>Monospace text (JetBrains Mono) is strictly reserved for executable s-expressions, code outputs, inline
                code tags, and technical labels. All descriptive prose must use Inter.</p>
            </div>
            <div class="rule-card">
              <h4>3. Gold is an Accent, Not a Paint</h4>
              <p>The primary gold (<code>#c8a855</code>) is reserved for highlighting primary active elements, selected
                menu items, cursor lines, and primary call-to-action buttons. Never scatter decorative gold borders.</p>
            </div>
            <div class="rule-card">
              <h4>4. Single-Pixel Borders</h4>
              <p>Default borders must be exactly <code>1px</code> solid with color <code>#2b2620</code>. Focus
                boundaries and active editor highlights use <code>2px</code> or <code>3px</code> solid gold.</p>
            </div>
          </div>
        </section>

        <!-- OpenGraph Cards Section -->
        <section id="opengraph" class="brand-section last-section">
          <div class="section-meta">
            <span class="section-num">11</span>
            <h2 class="section-title">OpenGraph Social Cards</h2>
            <p class="section-desc">
              The actual social preview cards, generated at 1200×630px from <code>og-template.html</code> by <code>scripts/generate-og.mjs</code> and shown here as the live images — homepage hero plus a per-page documentation variant (section category, version badge, page title, description). Regenerate with <code>make site-og</code>.
            </p>
          </div>

          <div class="og-showcase-container" style="display: flex; flex-direction: column; gap: 3rem; container-type: inline-size; width: 100%;">
            <!-- Homepage OG Card -->
            <div class="og-card-wrapper">
              <h4 style="font-family: 'JetBrains Mono', monospace; font-size: 0.75rem; color: var(--gold); text-transform: uppercase; margin-bottom: 1rem;">A. Homepage Preview (1200×630)</h4>
              <img src="/og/home.jpg" alt="Sema homepage OpenGraph card" loading="lazy" style="display: block; width: 100%; aspect-ratio: 1200 / 630; border: 1px solid var(--border); border-radius: 12px; box-shadow: 0 20px 50px rgba(0,0,0,0.5);" />
            </div>

            <!-- Docs/Reference OG Card -->
            <div class="og-card-wrapper">
              <h4 style="font-family: 'JetBrains Mono', monospace; font-size: 0.75rem; color: var(--gold); text-transform: uppercase; margin-bottom: 0.5rem;">B. Documentation / Reference Page Preview (1200×630)</h4>

              <!-- Tab selector -->
              <div class="og-variation-tabs" style="display: flex; gap: 0.5rem; flex-wrap: wrap; margin-bottom: 1.25rem;">
                <button
                  v-for="(v, index) in ogVariations"
                  :key="v.slug"
                  @click="activeOgVariation = index"
                  :style="{
                    backgroundColor: activeOgVariation === index ? 'rgba(200, 168, 85, 0.08)' : 'transparent',
                    borderColor: activeOgVariation === index ? 'var(--gold)' : 'var(--border)',
                    color: activeOgVariation === index ? 'var(--gold)' : 'var(--text-secondary)'
                  }"
                  style="font-family: 'JetBrains Mono', monospace; font-size: 0.7rem; padding: 0.4rem 0.8rem; border: 1px solid; border-radius: 4px; cursor: pointer; transition: all 0.15s; outline: none;"
                >
                  {{ v.label }}
                </button>
              </div>

              <img :src="`/og/${ogVariations[activeOgVariation].slug}.jpg`" :alt="`Documentation OpenGraph card — ${ogVariations[activeOgVariation].label}`" loading="lazy" style="display: block; width: 100%; aspect-ratio: 1200 / 630; border: 1px solid var(--border); border-radius: 12px; box-shadow: 0 20px 50px rgba(0,0,0,0.5);" />
            </div>
          </div>
        </section>

        <!-- 12. Code Typer — <sema-code-typer> from the published @sema-lang/ui. -->
        <section id="typer" class="brand-section">
          <div class="section-meta">
            <span class="section-num">12</span>
            <h2 class="section-title">Code Typer</h2>
            <p class="section-desc">
              <code>&lt;sema-code-typer&gt;</code> is a live web component that types code out with
              Sema syntax highlighting, a moving caret and optional editor chrome — a reusable
              toolkit piece for animated code in docs, demos and marketing. The same component
              exports to GIF or WebP via <code>npm&nbsp;run&nbsp;export:typer</code>.
            </p>
          </div>

          <ClientOnly>
            <div style="display: flex; flex-direction: column; gap: 2rem; max-width: 760px;">
              <sema-code-typer frame logo status line-numbers filename="maze.sema" rows="14" loop cps="42">{{ mazeSource }}</sema-code-typer>
              <sema-code-typer loop cps="30" aria-label="inline Sema typer">(define (square x) (* x x))</sema-code-typer>
            </div>
          </ClientOnly>
        </section>
      </main>
    </div>
  </div>
</template>

<style scoped>
.brand-guide {
  --bg-raise: #181512;
  --border-lo: #221e19;
  --gold-line: rgba(200, 168, 85, .28);
  --gold-bright: #e3c878;
  --gold-fade: rgba(200, 168, 85, .09);
  --dim: #6b6354;
  --muted: #968c79;
  --text: #e9e3d6;

  --gold: #c8a855;
  --gold-dim: rgba(200, 168, 85, 0.5);
  --gold-glow: rgba(200, 168, 85, 0.08);
  --gold-soft: rgba(200, 168, 85, 0.14);
  --bg: #131110;
  --bg-elevated: #181512;
  --bg-editor: #1c1916;
  --bg-output: #0f0d0c;
  --text-primary: #e9e3d6;
  --text-secondary: #968c79;
  --text-tertiary: #6b6354;
  --border: #2b2620;
  --border-focus: #c8a855;
  --success: #6a9955;
  --error: #c85555;

  background-color: #131110;
  color: #968c79;
  font-family: 'Inter', system-ui, sans-serif;
  min-height: 100vh;
  padding-bottom: 6rem;
  line-height: 1.6;
  -webkit-font-smoothing: antialiased;
}

.brand-container {
  max-width: 1400px;
  margin: 0 auto;
  padding: 0 2rem;
}

/* Hero Section */
.brand-hero {
  padding: 6rem 0 4rem;
  border-bottom: 1px solid #2b2620;
  background: radial-gradient(ellipse at 50% 100%, rgba(200, 168, 85, 0.05), transparent 70%);
  text-align: center;
  margin-bottom: 3rem;
}

.hero-tag {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  text-transform: uppercase;
  color: #c8a855;
  letter-spacing: 0.15em;
  display: inline-block;
  margin-bottom: 1rem;
  border: 1px solid rgba(200, 168, 85, 0.2);
  padding: 0.25rem 0.75rem;
  border-radius: 20px;
  background: rgba(200, 168, 85, 0.02);
}

.hero-title {
  font-family: 'Cormorant', Georgia, serif;
  font-size: clamp(2.5rem, 5vw, 4rem);
  font-weight: 300;
  color: #e9e3d6;
  line-height: 1.15;
  margin-bottom: 1.5rem;
  letter-spacing: -0.01em;
}

.hero-subtitle {
  font-size: 1.15rem;
  color: #968c79;
  max-width: 50rem;
  margin: 0 auto;
  font-weight: 300;
}

/* Layout Grid */
.brand-guide-layout {
  display: grid;
  grid-template-columns: 260px 1fr;
  gap: 4rem;
  max-width: 1400px;
  margin: 0 auto;
  padding: 0 2rem;
}

/* Sidebar Navigation */
.brand-sidebar {
  position: sticky;
  top: 100px;
  height: calc(100vh - 140px);
  overflow-y: auto;
  border-right: 1px solid #2b2620;
  padding-right: 2rem;
}

.brand-sidebar ul {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
}

.brand-sidebar a {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  color: #6b6354;
  text-decoration: none;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  transition: all 0.15s ease-in-out;
  display: block;
}

.brand-sidebar a:hover {
  color: #c8a855;
  transform: translateX(4px);
}

/* Main Content area */
.brand-content {
  min-width: 0;
}

.brand-section {
  padding-bottom: 6rem;
  margin-bottom: 4rem;
  border-bottom: 1px solid #2b2620;
  scroll-margin-top: 120px;
}

.brand-section.last-section {
  border-bottom: none;
}

.section-meta {
  margin-bottom: 3rem;
}

.section-num {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.8rem;
  color: #c8a855;
  display: block;
  margin-bottom: 0.5rem;
  opacity: 0.8;
}

.section-title {
  font-family: 'Cormorant', Georgia, serif;
  font-size: 2.2rem;
  font-weight: 300;
  color: #e9e3d6;
  margin: 0 0 1rem 0;
}

.section-desc {
  font-size: 1.05rem;
  color: #968c79;
  max-width: 48rem;
}

/* Overview Section Visuals */
.overview-visual-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 1.5rem;
}

.overview-card {
  background-color: #181512;
  border: 1px solid #2b2620;
  padding: 2rem;
  border-radius: 8px;
}

.overview-card h4 {
  font-family: 'Cormorant', Georgia, serif;
  font-size: 1.4rem;
  color: #e9e3d6;
  font-weight: 400;
  margin-bottom: 0.75rem;
}

.overview-card p {
  color: #968c79;
  font-size: 0.95rem;
  line-height: 1.6;
}

/* Logo Showcase */
.logo-display-card {
  display: grid;
  grid-template-columns: 1fr 2fr;
  background-color: #181512;
  border: 1px solid #2b2620;
  border-radius: 8px;
  overflow: hidden;
}

.logo-preview-area {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 4rem;
  background: radial-gradient(circle, rgba(200, 168, 85, 0.06) 0%, transparent 80%), #1c1916;
  border-right: 1px solid #2b2620;
  position: relative;
}

.logo-preview-area::after {
  content: '';
  position: absolute;
  inset: 0;
  background-image: radial-gradient(#2b2620 1px, transparent 1px);
  background-size: 16px 16px;
  opacity: 0.25;
  pointer-events: none;
}

.preview-svg {
  width: 100%;
  max-width: 200px;
  height: auto;
  z-index: 1;
}

.subtle-svg-wrap :deep(svg),
.mockup-footer-logo :deep(svg) {
  width: 100%;
  height: auto;
  display: block;
}

.logo-code-area {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-width: 0;
}

.code-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 0.75rem 1.25rem;
  background-color: #181512;
  border-bottom: 1px solid #2b2620;
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  color: #6b6354;
}

.btn-copy-code {
  background-color: transparent;
  border: 1px solid #2b2620;
  color: #c8a855;
  padding: 0.25rem 0.75rem;
  border-radius: 4px;
  cursor: pointer;
  transition: all 0.2s;
  font-size: 0.7rem;
  font-family: 'JetBrains Mono', monospace;
}

.btn-copy-code:hover {
  background-color: rgba(200, 168, 85, 0.08);
  border-color: #c8a855;
}

.code-view {
  margin: 0;
  padding: 1.25rem;
  background-color: #0f0d0c;
  overflow: auto;
  flex-grow: 1;
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  line-height: 1.6;
  color: #968c79;
  max-height: 250px;
}

/* Typography Section */
.type-container {
  display: flex;
  flex-direction: column;
  gap: 3rem;
}

.font-specimen-block {
  background-color: #181512;
  border: 1px solid #2b2620;
  border-radius: 8px;
  padding: 2.5rem;
}

.font-label {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  color: #c8a855;
  text-transform: uppercase;
  letter-spacing: 0.1em;
  margin: 0 0 1.5rem 0;
  border-bottom: 1px solid #2b2620;
  padding-bottom: 0.75rem;
}

.font-preview-large {
  font-size: 1.4rem;
  line-height: 1.5;
  color: #e9e3d6;
  padding-bottom: 2rem;
  border-bottom: 1px dashed #2b2620;
  margin-bottom: 2rem;
  word-break: break-all;
}

.serif-font {
  font-family: 'Cormorant', Georgia, serif;
}

.mono-font {
  font-family: 'JetBrains Mono', monospace;
}

.font-specimen-lines {
  display: flex;
  flex-direction: column;
  gap: 1.5rem;
}

.specimen-line {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.specimen-info {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.65rem;
  color: #6b6354;
  text-transform: uppercase;
}

.specimen-sample {
  color: #e9e3d6;
  line-height: 1.3;
}

.h1-preview {
  font-size: 2.2rem;
  font-weight: 300;
}

.h2-preview {
  font-size: 1.8rem;
  font-weight: 300;
}

.h3-preview {
  font-size: 1.4rem;
  font-weight: 500;
}

.code-preview {
  font-size: 0.85rem;
  background-color: #0f0d0c;
  padding: 0.75rem 1rem;
  border-radius: 4px;
  border: 1px solid #2b2620;
}

.code-preview-small {
  font-size: 0.75rem;
  background-color: #0f0d0c;
  padding: 0.75rem 1rem;
  border-radius: 4px;
  border: 1px solid #2b2620;
}

/* Claims and Lists styles */
.claims {
  list-style: none;
  padding: 0;
  margin: 0;
}

.claims li {
  padding: 16px 0;
  border-bottom: 1px solid #221e19;
  font-size: 15px;
  color: #968c79;
  line-height: 1.65;
}

.claims li:first-child {
  padding-top: 4px;
}

.claims li:last-child {
  border-bottom: none;
}

.claims strong {
  color: #e9e3d6;
  font-weight: 500;
  display: block;
  margin-bottom: 3px;
}

.claims code, .claims mark {
  font-family: 'JetBrains Mono', monospace;
  font-size: 12.5px;
  color: #e3c878;
  background: rgba(200, 168, 85, .09);
  padding: 1px 5px;
  border-radius: 4px;
}

.ship-list {
  list-style: none;
  padding: 0;
  margin: 0;
}

.ship-list li {
  display: flex;
  align-items: flex-start;
  gap: 14px;
  padding: 10px 0;
  font-size: 15px;
  color: #968c79;
}

.ship-list li::before {
  content: "›";
  color: #c8a855;
  font-family: 'JetBrains Mono', monospace;
  font-weight: bold;
  line-height: 1.6;
}

.ship-list strong {
  color: #e9e3d6;
  font-weight: 500;
}

.file-icon-box :deep(svg) {
  width: 24px !important;
  height: 24px !important;
}

.tree-nodes :deep(svg) {
  width: 16px !important;
  height: 16px !important;
}

.mockup-footer a {
  transition: color 0.15s ease;
}

.mockup-footer a:hover {
  color: var(--gold) !important;
}

/* Colors Showcase */
.colors-table-wrap {
  border-color: #2b2620;
}
.colors-table td {
  vertical-align: middle;
  font-size: 0.75rem;
}
.color-cell {
  text-align: center;
  padding: 0.5rem;
}
.color-preview-circle {
  width: 20px;
  height: 20px;
  border-radius: 4px;
  border: 1px solid rgba(255, 255, 255, 0.1);
  margin: 0 auto;
}
.color-row {
  cursor: pointer;
  transition: background-color 0.15s;
}
.color-row:hover {
  background-color: rgba(200, 168, 85, 0.03);
}
.color-name-cell {
  font-weight: 500;
  color: #e9e3d6;
  position: relative;
  white-space: nowrap;
}
.color-role-cell {
  font-family: 'Inter', system-ui, sans-serif;
  color: #968c79;
  font-size: 0.8rem;
  font-style: italic;
}
.color-group-header-row {
  background-color: #181512;
}
.color-group-title {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.7rem;
  color: #c8a855;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  font-weight: 600;
  padding: 0.6rem 1.25rem !important;
  border-top: 1px solid #2b2620;
}
.color-formats-cell {
  white-space: nowrap;
}
.color-formats-cell .format-val {
  margin-bottom: 0.15rem;
}
.color-formats-cell .format-val:last-child {
  margin-bottom: 0;
}

/* Depth, Radius, Spacing Section */
.depth-grid {
  display: grid;
  grid-template-columns: 1.1fr 1fr;
  gap: 3rem;
}

.depth-column {
  display: flex;
  flex-direction: column;
}

.depth-section-title {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.7rem;
  color: #c8a855;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  margin: 0 0 1.25rem 0;
  border-bottom: 1px solid #2b2620;
  padding-bottom: 0.5rem;
}

.elevation-stack {
  display: flex;
  flex-direction: column;
  border: 1px solid #2b2620;
  border-radius: 6px;
  overflow: hidden;
}

.elevation-level {
  padding: 1.25rem 1.5rem;
  border-bottom: 1px solid #2b2620;
}

.elevation-level:last-child {
  border-bottom: none;
}

.elevation-level .el-name {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.85rem;
  color: #e9e3d6;
  margin-bottom: 0.25rem;
}

.elevation-level .el-value {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.7rem;
  color: #c8a855;
  margin-bottom: 0.35rem;
}

.elevation-level .el-role {
  font-size: 0.8rem;
  color: #968c79;
  font-style: italic;
  line-height: 1.4;
}

.radius-row {
  display: flex;
  gap: 1.5rem;
  flex-wrap: wrap;
  align-items: flex-end;
}

.radius-sample {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.5rem;
}

.radius-box {
  width: 52px;
  height: 52px;
  border: 1px solid rgba(200, 168, 85, 0.4);
  background: #181512;
}

.radius-label {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.65rem;
  color: #6b6354;
}

.spacing-list {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
}

.spacing-row {
  display: flex;
  align-items: center;
  gap: 1rem;
}

.spacing-label {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.7rem;
  color: #6b6354;
  width: 70px;
}

.spacing-bar {
  height: 8px;
  background: #c8a855;
  opacity: 0.35;
  border-radius: 1px;
}

/* UI Kit Components Styling */
.components-grid {
  display: grid;
  grid-template-columns: 1fr;
  gap: 3.5rem;
}

.components-group {
  display: flex;
  flex-direction: column;
  gap: 1.5rem;
}

.components-group h3 {
  font-family: 'Cormorant', Georgia, serif;
  font-size: 1.5rem;
  color: #e9e3d6;
  border-bottom: 1px solid #2b2620;
  padding-bottom: 0.5rem;
  margin: 0 0 1rem 0;
}

.component-showcase-row {
  display: flex;
  align-items: center;
  gap: 2rem;
  border-bottom: 1px solid #1c1916;
  padding-bottom: 1.25rem;
}

.component-showcase-row.flex-column {
  flex-direction: column;
  align-items: flex-start;
  gap: 0.5rem;
}

.component-ref-label {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.7rem;
  color: #6b6354;
  min-width: 140px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.component-preview-cell {
  display: flex;
  align-items: center;
  gap: 1.25rem;
  flex-wrap: wrap;
}

/* Standard Button Tokens */
.btn-primary {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.8rem;
  font-weight: 500;
  letter-spacing: 0.04em;
  padding: 0.8rem 1.8rem;
  border-radius: 6px;
  border: none;
  background: #c8a855;
  color: #131110;
  cursor: pointer;
  transition: background-color 0.15s;
}

.btn-primary:hover:not(:disabled) {
  background: #e3c878;
}

.btn-primary:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}

.btn-secondary {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.8rem;
  letter-spacing: 0.04em;
  padding: 0.8rem 1.8rem;
  border-radius: 6px;
  border: 1px solid #2b2620;
  background: transparent;
  color: #e9e3d6;
  cursor: pointer;
  transition: all 0.15s;
}

.btn-secondary:hover, .btn-secondary.active {
  border-color: #6b6354;
  color: #c8a855;
  background-color: rgba(200, 168, 85, 0.03);
}

.btn-run {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  letter-spacing: 0.05em;
  padding: 0.4rem 1.2rem;
  border-radius: 4px;
  border: none;
  background: #c8a855;
  color: #131110;
  cursor: pointer;
  font-weight: 600;
  display: flex;
  align-items: center;
  gap: 0.5rem;
}

.kbd {
  font-family: system-ui, sans-serif;
  font-size: 0.65rem;
  opacity: 0.75;
  background: rgba(0, 0, 0, 0.15);
  padding: 0.1rem 0.35rem;
  border-radius: 3px;
  font-weight: bold;
}

.btn-ghost {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  letter-spacing: 0.04em;
  padding: 0.35rem 0.75rem;
  border-radius: 4px;
  border: none;
  background: transparent;
  color: #6b6354;
  cursor: pointer;
  transition: all 0.15s;
}

.btn-ghost:hover {
  color: #e9e3d6;
}

.btn-ghost.active {
  color: #c8a855;
  background: rgba(200, 168, 85, 0.08);
}

.btn-pill {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  letter-spacing: 0.03em;
  padding: 0.4rem 1.1rem;
  border-radius: 20px;
  border: 1px solid rgba(200, 168, 85, 0.5);
  background: transparent;
  color: #c8a855;
  cursor: pointer;
  transition: all 0.15s;
}

.btn-pill:hover, .btn-pill.hover-pill {
  background: rgba(200, 168, 85, 0.08);
  border-color: #c8a855;
}

.debug-toolbar {
  display: flex;
  gap: 0.25rem;
}

.btn-debug {
  font-family: system-ui, sans-serif;
  font-size: 0.85rem;
  width: 32px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid #2b2620;
  border-radius: 4px;
  background: transparent;
  color: #968c79;
  cursor: pointer;
  transition: all 0.15s;
}

.btn-debug:hover {
  background: rgba(200, 168, 85, 0.08);
  color: #c8a855;
  border-color: rgba(200, 168, 85, 0.5);
}

.btn-debug.danger:hover {
  color: #c85555;
  border-color: #c85555;
  background: rgba(200, 85, 85, 0.05);
}

/* Card feature */
.card-feature {
  background: #181512;
  border: 1px solid #2b2620;
  border-radius: 8px;
  padding: 1.5rem;
  width: 100%;
}

.card-feature h4 {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  color: #c8a855;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  margin: 0 0 0.5rem 0;
}

.card-feature p {
  font-size: 1.05rem;
  line-height: 1.5;
  margin: 0;
  color: #968c79;
}

/* Tags */
.tag-provider {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.72rem;
  padding: 0.25rem 0.75rem;
  border: 1px solid #2b2620;
  border-radius: 20px;
  color: #968c79;
  background: #181512;
}

.tag-fn {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.68rem;
  padding: 0.15rem 0.45rem;
  border: 1px solid #2b2620;
  border-radius: 4px;
  color: #6b6354;
  background: #1c1916;
}

/* Notebook Cells */
.nb-cell {
  display: flex;
  gap: 0;
  width: 100%;
  margin-bottom: 0.5rem;
}

.nb-cell-num {
  width: 44px;
  flex-shrink: 0;
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 0.6rem;
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  color: #6b6354;
}

.nb-cell-editor {
  flex-grow: 1;
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.8rem;
  line-height: 1.6;
  color: #e9e3d6;
  background: #1c1916;
  border: 1px solid #2b2620;
  border-radius: 4px;
  padding: 0.5rem 0.75rem;
}

.nb-cell.active .nb-cell-editor {
  border-left: 3px solid #c8a855;
  border-top-left-radius: 0;
  border-bottom-left-radius: 0;
}

.nb-cell.stale .nb-cell-editor {
  border-left: 3px dashed rgba(200, 168, 85, 0.4);
  border-top-left-radius: 0;
  border-bottom-left-radius: 0;
  opacity: 0.65;
}

/* Output Panel */
.output-panel {
  background: #0f0d0c;
  padding: 1.25rem;
  border: 1px solid #2b2620;
  border-radius: 6px;
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.8rem;
  line-height: 1.6;
  width: 100%;
}

.out-stdout {
  color: #968c79;
}

.out-value {
  color: #c8a855;
  border-left: 2px solid rgba(200, 168, 85, 0.4);
  padding-left: 0.75rem;
  margin-top: 0.25rem;
}

.out-error {
  color: #c85555;
  background: rgba(200, 85, 85, 0.05);
  border-left: 2px solid #c85555;
  padding: 0.4rem 0.75rem;
  margin-top: 0.5rem;
  border-radius: 0 4px 4px 0;
}

.out-meta {
  color: #6b6354;
  font-size: 0.7rem;
  margin-top: 0.75rem;
  padding-top: 0.5rem;
  border-top: 1px solid #2b2620;
}

/* Syntax Section */
.syntax-columns {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 2rem;
  margin-bottom: 2rem;
}

.syntax-col h4 {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  color: #c8a855;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  margin: 0 0 1rem 0;
}

.syntax-code {
  background: #1c1916;
  border: 1px solid #2b2620;
  border-radius: 6px;
  padding: 1.25rem;
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.8rem;
  line-height: 1.65;
  white-space: pre-wrap;
}

/* Syntax token styles */
.syn-w-comment {
  color: #5a5a4a;
  font-style: italic;
}

.syn-w-keyword {
  color: #d4a052;
}

.syn-w-string {
  color: #8aaa6a;
}

.syn-w-number {
  color: #d08a60;
}

.syn-w-kwlit {
  color: #c89050;
}

.syn-w-builtin {
  color: #88a8b8;
}

.syn-w-paren {
  color: #444438;
}

.syn-p-comment {
  color: #6b6354;
  font-style: italic;
}

.syn-p-keyword {
  color: #c8a855;
}

.syn-p-string {
  color: #a8c47a;
}

.syn-p-number {
  color: #d19a66;
}

.syn-p-kwlit {
  color: #7aacb8;
}

.syn-p-paren {
  color: #6a6258;
}

/* Tables Layout */
.table-wrap {
  overflow-x: auto;
  border: 1px solid #2b2620;
  border-radius: 6px;
}

table {
  width: 100%;
  border-collapse: collapse;
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
}

th, td {
  padding: 0.75rem 1.25rem;
  text-align: left;
  border-bottom: 1px solid #2b2620;
}

th {
  color: var(--gold-dim);
  font-weight: 500;
  letter-spacing: 0.05em;
  text-transform: uppercase;
  font-size: 0.7rem;
  background-color: #181512;
}

td {
  color: #968c79;
}

tr:last-child td {
  border-bottom: none;
}

.color-dot {
  display: inline-block;
  width: 10px;
  height: 10px;
  border-radius: 2px;
  margin-right: 0.5rem;
  vertical-align: middle;
  border: 1px solid rgba(255, 255, 255, 0.1);
}

/* Icon Comparison Showcase */
.icons-comparison-list {
  display: flex;
  flex-direction: column;
  gap: 2.5rem;
}

.icon-pair-card {
  background-color: #181512;
  border: 1px solid #2b2620;
  border-radius: 8px;
  padding: 2rem;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 3rem;
}

.pair-meta {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.pair-meta h4 {
  font-family: 'Cormorant', Georgia, serif;
  font-size: 1.35rem;
  color: #e9e3d6;
  font-weight: 400;
  margin: 0;
}

.pair-meta p {
  font-size: 0.9rem;
  color: #968c79;
  line-height: 1.5;
  margin: 0;
}

.pair-renders {
  display: flex;
  gap: 2rem;
  align-items: center;
  flex-wrap: wrap;
}

.render-item {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.6rem;
}

.render-box {
  background-color: #000000;
  border: 1px solid #2b2620;
  border-radius: 6px;
  width: 96px;
  height: 96px;
  display: flex;
  align-items: center;
  justify-content: center;
  position: relative;
}

.render-box :deep(svg) {
  width: 48px;
  height: 48px;
}

.btn-copy-mini {
  background: transparent;
  border: 1px solid #2b2620;
  color: #6b6354;
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.65rem;
  padding: 0.2rem 0.5rem;
  border-radius: 4px;
  cursor: pointer;
  transition: all 0.15s;
}

/* Guidelines section */
.guidelines-card {
  background-color: #181512;
  border: 1px solid #2b2620;
  border-radius: 8px;
  padding: 2.5rem;
}
.guidelines-title {
  font-family: 'Cormorant', Georgia, serif;
  font-size: 1.6rem;
  color: #e9e3d6;
  margin: 0 0 0.5rem 0;
  font-weight: 400;
}
.guidelines-desc {
  font-size: 0.95rem;
  color: #968c79;
  margin-bottom: 2rem;
}
.guidelines-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 2rem;
}
.guide-item h5 {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.8rem;
  color: #c8a855;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin: 0 0 0.75rem 0;
  border-bottom: 1px solid #2b2620;
  padding-bottom: 0.25rem;
}
.guide-item ul {
  list-style-type: disc;
  padding-left: 1.2rem;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}
.guide-item li {
  font-size: 0.85rem;
  color: #968c79;
  line-height: 1.5;
}
.guide-item li strong {
  color: #e9e3d6;
}

.btn-copy-mini:hover {
  border-color: #c8a855;
  color: #c8a855;
  background: rgba(200, 168, 85, 0.04);
}

/* Inventory Section Styles */
.inventory-subheading {
  font-family: 'Cormorant', Georgia, serif;
  font-size: 1.4rem;
  color: #e9e3d6;
  margin: 0 0 1rem 0;
  font-weight: 400;
}

.inv-name {
  color: #c8a855;
  font-weight: 500;
  white-space: nowrap;
}

.table-wrap table a {
  color: #e9e3d6;
  text-decoration: none;
  transition: color 0.15s;
}

.table-wrap table a:hover {
  color: #c8a855;
}

/* Rules Section */
.rules-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 1.5rem;
}

.rule-card {
  background-color: #181512;
  border: 1px solid #2b2620;
  border-radius: 6px;
  padding: 1.5rem;
}

.rule-card h4 {
  font-family: 'Cormorant', Georgia, serif;
  font-size: 1.2rem;
  color: #e9e3d6;
  margin: 0 0 0.5rem 0;
  font-weight: 400;
}

.rule-card p {
  color: #968c79;
  font-size: 0.9rem;
  line-height: 1.5;
  margin: 0;
}

/* Mobile Responsiveness */
@media (max-width: 900px) {
  .brand-guide-layout {
    grid-template-columns: 1fr;
    gap: 2rem;
    padding: 0 1rem;
  }

  .brand-sidebar {
    position: static;
    height: auto;
    border-right: none;
    border-bottom: 1px solid #2b2620;
    padding-right: 0;
    padding-bottom: 1rem;
  }

  .brand-sidebar ul {
    flex-direction: row;
    flex-wrap: wrap;
    gap: 0.75rem;
  }

  .icon-pair-card {
    grid-template-columns: 1fr;
    gap: 1.5rem;
  }

  .depth-grid {
    grid-template-columns: 1fr;
    gap: 2.5rem;
  }

  .syntax-columns {
    grid-template-columns: 1fr;
  }

  .logo-display-card {
    grid-template-columns: 1fr;
  }

  .logo-preview-area {
    border-right: none;
    border-bottom: 1px solid #2b2620;
  }

  .overview-visual-grid {
    grid-template-columns: 1fr;
  }
}

/* ---------- Code Panes ---------- */
.pane {
  background: var(--bg-raise);
  border: 1px solid var(--border);
  border-radius: 12px;
  overflow: hidden;
  box-sizing: border-box;
  text-align: left;
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
  font-family: 'JetBrains Mono', monospace;
  font-size: 12px;
}

.pane-head .t {
  color: var(--text-primary);
}

.pane.sema .pane-head .t {
  color: var(--gold-bright);
}

.pane-head .n {
  color: var(--dim);
}

.pane pre {
  margin: 0;
  font-family: 'JetBrains Mono', monospace;
  font-size: 12.5px;
  line-height: 1.62;
  padding: 18px 20px;
  overflow-x: auto;
  color: #c9c2b4;
  background-color: transparent;
}

.pane.python pre {
  position: relative;
  max-height: 560px;
  overflow-y: hidden;
  color: #9b9486;
}

.pane-foot {
  padding: 13px 18px;
  border-top: 1px solid var(--border-lo);
  font-size: 13px;
  color: var(--muted);
  line-height: 1.55;
  background-color: #181512;
}

.pane.sema .pane-foot {
  color: var(--text-primary);
}

/* ---------- OpenGraph Mockups ---------- */
.og-card {
  box-sizing: border-box;
  text-align: left;
}
.og-card * {
  box-sizing: border-box;
}
.og-card-wrapper {
  width: 100%;
}
.og-code-preview * {
  font-family: 'JetBrains Mono', monospace !important;
}
</style>
