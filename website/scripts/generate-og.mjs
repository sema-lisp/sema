// Generates per-page OpenGraph card images from website/og-template.html.
//
// Renders the template in headless Chromium (via Playwright) at 1200x630,
// once per page, driving its homepage/docs variants through URL query params.
// Output: website/public/og/<slug>.jpg  (slug from og.shared.mjs::ogSlug)
//
// Usage:
//   node scripts/generate-og.mjs            # all pages
//   node scripts/generate-og.mjs home docs  # only matching slugs (substring)
//
// Re-run after editing og-template.html, the logo, page titles, or the
// version, then commit the regenerated public/og/*.jpg before deploying.

import { chromium } from 'playwright'
import { readFileSync, writeFileSync, readdirSync, mkdirSync, statSync, existsSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { dirname, join, relative } from 'node:path'
import { OG_WIDTH, OG_HEIGHT, OG_EXT, ogSlug, categoryFor, isHomepage } from '../.vitepress/og.shared.mjs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const WEBSITE = join(__dirname, '..')
const TEMPLATE = join(WEBSITE, 'og-template.html')
const OUT_DIR = join(WEBSITE, 'public', 'og')
// Records the input hash each card was last rendered from, so an unchanged page
// is skipped (never re-encoded) — a fresh JPEG would differ byte-for-byte across
// Chromium versions and churn git even when the card looks identical. Lives
// outside public/ so it isn't deployed; commit it alongside the images.
const MANIFEST = join(WEBSITE, 'og-manifest.json')
const args = process.argv.slice(2)
const force = args.includes('--force') || args.includes('-f')
const filters = args.filter((a) => a !== '--force' && a !== '-f')

// Current Sema version, shown as the badge on docs cards.
function semaVersion() {
  try {
    const cargo = readFileSync(join(WEBSITE, '..', 'Cargo.toml'), 'utf8')
    const m = cargo.match(/^\s*version\s*=\s*"([^"]+)"/m)
    return m ? `v${m[1]}` : 'docs'
  } catch {
    return 'docs'
  }
}

// Recursively collect every .md page under website/. Skips node_modules,
// public, and every dot-entry — a stale `.vercel/output` from a local CLI
// build contains generated .md copies of every page and must not count.
function collectPages(dir) {
  const out = []
  for (const name of readdirSync(dir).sort()) {
    if (name.startsWith('.') || name === 'node_modules' || name === 'public') continue
    const full = join(dir, name)
    const st = statSync(full)
    if (st.isDirectory()) out.push(...collectPages(full))
    else if (name.endsWith('.md')) out.push(full)
  }
  return out
}

function stripInline(s) {
  return s
    .replace(/`([^`]+)`/g, '$1') // inline code
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1') // links -> text
    .replace(/[*_~]/g, '') // emphasis
    .replace(/\s+/g, ' ')
    .trim()
}

// Pull a title (first H1) and description (frontmatter `description:` or first
// real paragraph) out of a markdown file.
function pageMeta(md) {
  let body = md
  let fmDescription = null
  const fm = md.match(/^---\n([\s\S]*?)\n---\n?/)
  if (fm) {
    const d = fm[1].match(/^description:\s*(.+)$/m)
    if (d) fmDescription = stripInline(d[1].replace(/^["']|["']$/g, ''))
    body = md.slice(fm[0].length)
  }
  const lines = body.split('\n')
  let title = null
  let descParts = []
  let seenH1 = false
  for (const line of lines) {
    const h1 = line.match(/^#\s+(.+)$/)
    if (!seenH1) {
      if (h1) {
        title = stripInline(h1[1])
        seenH1 = true
      }
      continue
    }
    const t = line.trim()
    if (!t) {
      if (descParts.length) break
      continue
    }
    if (/^[#>`|:-]|^!\[|^<|^\s*[-*]\s/.test(t)) {
      if (descParts.length) break
      continue
    }
    descParts.push(t)
    if (descParts.join(' ').length > 200) break
  }
  let description = fmDescription || stripInline(descParts.join(' '))
  if (description.length > 165) {
    let cut = description.slice(0, 162)
    const lastSpace = cut.lastIndexOf(' ')
    if (lastSpace > 120) cut = cut.slice(0, lastSpace) // break on a word boundary
    description = cut.replace(/[\s.,;:—-]+$/, '') + '…'
  }
  return { title, description }
}

const BADGE = semaVersion()

function paramsForPage(relPath, md) {
  if (isHomepage(relPath)) return { variant: 'homepage' } // keep designed defaults
  const { title, description } = pageMeta(md)
  return {
    variant: 'docs',
    title: title || 'Sema',
    subtitle: description || 'A Lisp with runtime types for prompts, conversations, tools, and agents, implemented in Rust.',
    category: categoryFor(relPath),
    badge: BADGE,
  }
}

function templateUrl(params) {
  const u = pathToFileURL(TEMPLATE)
  for (const [k, v] of Object.entries(params)) u.searchParams.set(k, v)
  return u.toString()
}

const pages = collectPages(WEBSITE).map((full) => {
  const relPath = relative(WEBSITE, full).replace(/\\/g, '/')
  return { full, relPath, slug: ogSlug(relPath), out: join(OUT_DIR, `${ogSlug(relPath)}.${OG_EXT}`) }
})

// One-off cards that live outside the website docs tree (different product /
// domain / output path) but share the same template + brand.
const SPECIAL = [
  {
    slug: 'playground',
    out: join(WEBSITE, '..', 'playground', 'og-playground.jpg'),
    params: {
      variant: 'homepage',
      titleHtml: 'Sema <span>Playground</span>',
      subtitle:
        'Try Sema in your browser — prompts, conversations, tools, and agents are runtime values. No install; runs entirely in WebAssembly.',
      domain: 'sema.run',
    },
  },
  {
    // GitHub repo header / social preview — the brand hero (template defaults).
    slug: 'github',
    out: join(WEBSITE, '..', 'assets', 'og-github.jpg'),
    params: { variant: 'homepage' },
  },
]

mkdirSync(OUT_DIR, { recursive: true })

const QUALITY = 92

// Hash of everything shared across cards: the template markup (which carries the
// inline logo SVG + all CSS) and the vendored fonts. Rendering also depends on
// the Chromium version, but that's deliberately excluded — a renderer bump that
// doesn't change the design shouldn't rewrite every card. Use --force for that.
function baseInputHash() {
  const h = createHash('sha256')
  h.update(readFileSync(TEMPLATE))
  for (const f of ['cormorant-normal.woff2', 'inter-normal.woff2', 'jetbrains-mono-normal.woff2', 'jetbrains-mono-italic.woff2']) {
    const p = join(WEBSITE, 'public', 'fonts', f)
    if (existsSync(p)) h.update(readFileSync(p))
  }
  return h.digest('hex')
}

// Stable per-job hash: base inputs + this card's params + output geometry.
function jobInputHash(base, params) {
  const stable = JSON.stringify(Object.keys(params).sort().reduce((a, k) => ((a[k] = params[k]), a), {}))
  return createHash('sha256').update(base).update(stable).update(`${OG_WIDTH}x${OG_HEIGHT}q${QUALITY}`).digest('hex')
}

const BASE = baseInputHash()

const jobs = [
  ...pages.map((p) => ({
    slug: p.slug,
    out: p.out,
    params: paramsForPage(p.relPath, readFileSync(p.full, 'utf8')),
    match: `${p.slug} ${p.relPath}`,
  })),
  ...SPECIAL.map((s) => ({ slug: s.slug, out: s.out, params: s.params, match: s.slug })),
].map((j) => ({ ...j, hash: jobInputHash(BASE, j.params) }))

const selected = filters.length ? jobs.filter((j) => filters.some((f) => j.match.includes(f))) : jobs

// Skip any card whose inputs are unchanged since it was last rendered (and whose
// image still exists), unless --force. This is what makes regeneration
// idempotent: unchanged pages are never re-encoded, so they can't churn git.
let manifest = {}
try {
  manifest = JSON.parse(readFileSync(MANIFEST, 'utf8'))
} catch {}

const stale = force ? selected : selected.filter((j) => manifest[j.slug] !== j.hash || !existsSync(j.out))
const skipped = selected.length - stale.length

console.log(`Generating ${stale.length} OG image(s)  [badge ${BADGE}]${skipped ? `, ${skipped} unchanged` : ''}`)

let ok = 0
if (stale.length) {
  const browser = await chromium.launch()
  const page = await browser.newPage({
    viewport: { width: OG_WIDTH, height: OG_HEIGHT },
    deviceScaleFactor: 1,
  })

  const externalRequests = []
  await page.route(/^https?:\/\//, (route) => {
    externalRequests.push(route.request().url())
    return route.abort('blockedbyclient')
  })

  for (const j of stale) {
    const externalRequestStart = externalRequests.length
    await page.goto(templateUrl(j.params), { waitUntil: 'load' })
    await page.waitForSelector('html[data-og-ready="1"]', { timeout: 10000 })
    const jobExternalRequests = externalRequests.slice(externalRequestStart)
    if (jobExternalRequests.length) {
      throw new Error(`OG template made external request(s): ${jobExternalRequests.join(', ')}`)
    }
    mkdirSync(dirname(j.out), { recursive: true })
    await page.screenshot({
      path: j.out,
      type: 'jpeg',
      quality: QUALITY,
      clip: { x: 0, y: 0, width: OG_WIDTH, height: OG_HEIGHT },
    })
    manifest[j.slug] = j.hash
    ok++
    const label = j.params.title || j.params.titleHtml?.replace(/<[^>]+>/g, '') || ''
    console.log(`  ✓ ${relative(join(WEBSITE, '..'), j.out)}  (${j.params.variant}${label ? ` — ${label}` : ''})`)
  }

  await browser.close()
}

// Rewrite the manifest (sorted keys) only when it actually changed, so a no-op
// run leaves it byte-identical and git stays clean.
const serialized = JSON.stringify(Object.keys(manifest).sort().reduce((a, k) => ((a[k] = manifest[k]), a), {}), null, 2) + '\n'
if (!existsSync(MANIFEST) || readFileSync(MANIFEST, 'utf8') !== serialized) {
  writeFileSync(MANIFEST, serialized)
}

console.log(`Done: ${ok} written, ${skipped} unchanged.`)
