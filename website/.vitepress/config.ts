import { defineConfig } from 'vitepress'
// Canonical source: editors/vscode/sema/syntaxes/sema.tmLanguage.json
import semaLang from './sema.tmLanguage.json'
// Brand syntax theme — matches the hand-coded snippets + playground palette
// (gold keywords, green strings, orange numbers, cyan :keywords, dim parens).
import semaCodeTheme from './sema-code-theme.json'
import { SITE, OG_WIDTH, OG_HEIGHT, OG_EXT, ogSlug } from './og.shared.mjs'
// Auto-generates /llms.txt (index) and /llms-full.txt (all docs) at build time,
// so they never go stale. Replaces the old hand-maintained public/llms*.txt.
import llmstxt from 'vitepress-plugin-llms'

export default defineConfig({
  title: 'Sema',
  description: 'A Lisp with first-class LLM primitives, implemented in Rust.',
  appearance: 'force-dark',

  // Emit extensionless URLs (/docs/stdlib/csv) as the canonical form. The build
  // still writes csv.html files; Vercel (cleanUrls in vercel.json) serves the
  // clean path 200 and 308-redirects the .html form to it. Keeps internal links,
  // the sitemap, and per-page og:url/canonical (transformHead) all extensionless.
  cleanUrls: true,

  // <sema-*> are web components (e.g. <sema-code-typer>), not Vue components.
  vue: {
    template: {
      compilerOptions: {
        isCustomElement: (tag: string) => tag.startsWith('sema-'),
      },
    },
  },

  // Allow importing the built @sema/ui bundle + example sources from the repo root.
  vite: {
    server: { fs: { allow: ['../..'] } },
    plugins: [
      llmstxt({
        domain: 'https://sema-lang.com',
        title: 'Sema',
        description: 'A Scheme-like Lisp with first-class LLM primitives, implemented in Rust.',
        // Surfaced at the top of llms.txt (before the table of contents): the
        // delta-from-other-Lisps cheat sheet + a link to the full agent page.
        details: [
          '**Coding agents: read [Sema for LLM agents](https://sema-lang.com/docs/for-agents.md) first** — every way Sema differs from other Lisps in one short page. The essentials:',
          '',
          '- New functions are slash-namespaced (`file/read`, `string/split`, `regex/match?`); predicates end in `?`; conversions use `->`. Legacy Scheme string ops are kept (`string-append`).',
          '- Only `#f` and `nil` are falsy — `0`, `""`, and the empty list are truthy.',
          '- Lists are vector-backed (O(1) `nth`, O(n) `cons`); mutable state is `define` + `set!` (there is no `atom`/`swap!`).',
          '- Clojure-style surface: `:keywords`, `{:k v}` maps, `[1 2 3]` vectors, `#(* % %)` lambdas, `f"...${x}"` strings, `#"regex"` literals.',
          '- LLMs are language primitives: `llm/complete`, `deftool`/`agent/run`, `llm/extract`, cassettes, OpenTelemetry, vector store.',
          '',
          'The table of contents below indexes the full docs — fetch only the specific `/docs/**/*.md` pages you need, on demand. Do not load `/llms-full.txt` (the full concatenation, ~200k tokens) into context.',
        ].join('\n'),
      }),
    ],
  },

  sitemap: {
    hostname: 'https://sema-lang.com'
  },

  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' }],
    ['link', { rel: 'alternate', type: 'text/plain', href: '/llms.txt', title: 'LLM-friendly documentation index' }],
    ['meta', { property: 'og:type', content: 'website' }],
    ['meta', { property: 'og:title', content: 'Sema — Agent-native Lisp for LLM Workflows' }],
    ['meta', { property: 'og:description', content: 'A Scheme-like Lisp where prompts are s-expressions, conversations are persistent data structures, and LLM calls are just another form of evaluation. Implemented in Rust.' }],
    ['meta', { property: 'og:url', content: 'https://sema-lang.com' }],
    ['meta', { property: 'og:image', content: `${SITE}/og/home.${OG_EXT}` }],
    ['meta', { property: 'og:image:width', content: String(OG_WIDTH) }],
    ['meta', { property: 'og:image:height', content: String(OG_HEIGHT) }],
    ['meta', { property: 'og:locale', content: 'en_US' }],
    ['meta', { property: 'og:site_name', content: 'Sema' }],
    ['meta', { name: 'twitter:card', content: 'summary_large_image' }],
    ['meta', { name: 'twitter:image', content: `${SITE}/og/home.${OG_EXT}` }],
    ['meta', { name: 'twitter:title', content: 'Sema — Agent-native Lisp for LLM Workflows' }],
    ['meta', { name: 'twitter:description', content: 'A Scheme-like Lisp where prompts are s-expressions, conversations are persistent data structures, and LLM calls are just another form of evaluation.' }],
    ['meta', { name: 'theme-color', content: '#c8a855' }],
    ['link', { rel: 'preconnect', href: 'https://fonts.googleapis.com' }],
    ['link', { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' }],
    ['link', { href: 'https://fonts.googleapis.com/css2?family=Cormorant:ital,wght@0,300;0,400;0,500;0,600;1,400&family=Inter:wght@300;400;500;600&family=JetBrains+Mono:wght@400;500&display=swap', rel: 'stylesheet' }],
    ['script', { src: 'https://analytics.ahrefs.com/analytics.js', 'data-key': 'qIVZDGpfYQL6+4W26miIww', async: '' }],
  ],


  themeConfig: {
    logo: { src: '/logo.svg', alt: 'Sema' },
    siteTitle: false,

    nav: [
      { text: 'Guide', link: '/docs/' },
      {
        text: 'Reference',
        items: [
          { text: 'Standard Library', link: '/docs/stdlib/' },
          { text: 'LLM & Agents', link: '/docs/llm/' },
          { text: 'CLI & Tooling', link: '/docs/cli' }
        ]
      },
      { text: 'Internals', link: '/docs/internals/architecture' },
      { text: 'Playground', link: 'https://sema.run', target: '_blank' }
    ],

    sidebar: {
      '/docs/stdlib/': [
        {
          text: 'Standard Library Reference',
          items: [
            { text: 'Overview', link: '/docs/stdlib/' }
          ]
        },
        {
          text: 'Core Types',
          collapsed: false,
          items: [
            { text: 'Math & Arithmetic', link: '/docs/stdlib/math' },
            { text: 'Strings & Characters', link: '/docs/stdlib/strings' },
            { text: 'Lists', link: '/docs/stdlib/lists' },
            { text: 'Vectors', link: '/docs/stdlib/vectors' },
            { text: 'Maps & HashMaps', link: '/docs/stdlib/maps' },
            { text: 'Predicates', link: '/docs/stdlib/predicates' },
            { text: 'Bytevectors', link: '/docs/stdlib/bytevectors' },
            { text: 'Typed Arrays', link: '/docs/stdlib/typed-arrays' }
          ]
        },
        {
          text: 'File & Data Formats',
          collapsed: true,
          items: [
            { text: 'File I/O & Paths', link: '/docs/stdlib/file-io' },
            { text: 'PDF Processing', link: '/docs/stdlib/pdf' },
            { text: 'CSV Parsing', link: '/docs/stdlib/csv' },
            { text: 'TOML Parsing', link: '/docs/stdlib/toml' }
          ]
        },
        {
          text: 'Networking & Web',
          collapsed: true,
          items: [
            { text: 'HTTP & JSON', link: '/docs/stdlib/http-json' },
            { text: 'Web Server', link: '/docs/stdlib/web-server' }
          ]
        },
        {
          text: 'System & Databases',
          collapsed: true,
          items: [
            { text: 'System', link: '/docs/stdlib/system' },
            { text: 'SQLite Database', link: '/docs/stdlib/sqlite' },
            { text: 'Key-Value Store', link: '/docs/stdlib/kv-store' },
            { text: 'Serial Ports', link: '/docs/stdlib/serial' },
            { text: 'Regex Engine', link: '/docs/stdlib/regex' },
            { text: 'Crypto & Encoding', link: '/docs/stdlib/crypto' },
            { text: 'Date & Time', link: '/docs/stdlib/datetime' },
            { text: 'Context Manager', link: '/docs/stdlib/context' },
            { text: 'Terminal Styling', link: '/docs/stdlib/terminal' },
            { text: 'Playground & WASM', link: '/docs/stdlib/playground' }
          ]
        },
        {
          text: 'Concurrency & Streams',
          collapsed: true,
          items: [
            { text: 'Streams', link: '/docs/stdlib/streams' },
            { text: 'Concurrency', link: '/docs/stdlib/concurrency' },
            { text: 'Records', link: '/docs/stdlib/records' },
            { text: 'Text Processing', link: '/docs/stdlib/text-processing' }
          ]
        }
      ],

      '/docs/llm/': [
        {
          text: 'LLM Essentials',
          collapsed: false,
          items: [
            { text: 'Overview', link: '/docs/llm/' },
            { text: 'Completion & Chat', link: '/docs/llm/completion' },
            { text: 'Tools & Agents', link: '/docs/llm/tools-agents' },
            { text: 'Conversations', link: '/docs/llm/conversations' },
            { text: 'Prompts & Messages', link: '/docs/llm/prompts' },
            { text: 'Structured Extraction', link: '/docs/llm/extraction' }
          ]
        },
        {
          text: 'Going Further',
          collapsed: false,
          items: [
            { text: 'Providers', link: '/docs/llm/providers' },
            { text: 'Cost & Budgets', link: '/docs/llm/cost' },
            { text: 'Caching', link: '/docs/llm/caching' },
            { text: 'Resilience & Retry', link: '/docs/llm/resilience' },
            { text: 'Embeddings', link: '/docs/llm/embeddings' },
            { text: 'Vector Store & Math', link: '/docs/llm/vector-store' },
            { text: 'Cassettes', link: '/docs/llm/cassettes' }
          ]
        },
        {
          text: 'Observability',
          collapsed: false,
          items: [
            { text: 'Tracing & Metrics', link: '/docs/llm/observability' },
            { text: 'Backend Compatibility', link: '/docs/llm/otel-compat' }
          ]
        },
        {
          text: 'Cookbook',
          collapsed: false,
          items: [
            { text: 'Workflows', link: '/docs/llm/workflows' },
            { text: 'RAG: Retrieve & Rerank', link: '/docs/llm/rag' }
          ]
        }
      ],

      '/docs/internals/': [
        {
          text: 'Engine Internals',
          collapsed: false,
          items: [
            { text: 'Architecture', link: '/docs/internals/architecture' },
            { text: 'Build a Bytecode VM', link: '/docs/internals/build-a-bytecode-vm' },
            { text: 'Bytecode VM', link: '/docs/internals/bytecode-vm' },
            { text: 'Bytecode File Format', link: '/docs/internals/bytecode-format' },
            { text: 'Executable Format', link: '/docs/internals/executable-format' },
            { text: 'Evaluator & TCO', link: '/docs/internals/evaluator' },
            { text: 'Reader & Spans', link: '/docs/internals/reader' },
            { text: 'Fuzzing the VM', link: '/docs/internals/fuzzing' },
            { text: 'Performance', link: '/docs/internals/performance' },
            { text: 'Lisp Dialect Benchmark', link: '/docs/internals/lisp-comparison' },
            { text: 'Feature Comparison', link: '/docs/internals/feature-comparison' },
            { text: 'Glossary', link: '/docs/internals/glossary' }
          ]
        }
      ],

      '/docs/': [
        {
          text: 'Getting Started',
          collapsed: false,
          items: [
            { text: 'Introduction', link: '/docs/' },
            { text: 'Quickstart', link: '/docs/quickstart' },
            { text: 'Basic Syntax', link: '/docs/tutorial/basics' },
            { text: 'Functions & Scope', link: '/docs/tutorial/functions' },
            { text: 'Concurrency & Async', link: '/docs/tutorial/concurrency' }
          ]
        },
        {
          text: 'Language Reference',
          collapsed: false,
          items: [
            { text: 'Data Types', link: '/docs/language/data-types' },
            { text: 'Special Forms', link: '/docs/language/special-forms' },
            { text: 'Macros & Modules', link: '/docs/language/macros-modules' }
          ]
        },
        {
          text: 'Tooling & Workspace',
          collapsed: false,
          items: [
            { text: 'CLI Commands', link: '/docs/cli' },
            { text: 'Code Formatter', link: '/docs/formatter' },
            { text: 'Shell Completions', link: '/docs/shell-completions' },
            { text: 'Editor Integration', link: '/docs/editors' },
            { text: 'Language Server (LSP)', link: '/docs/lsp' },
            { text: 'Debugger (DAP)', link: '/docs/dap' },
            { text: 'MCP Server', link: '/docs/mcp' },
            { text: 'Notebooks', link: '/docs/notebook' },
            { text: 'Packages & Modules', link: '/docs/packages' }
          ]
        },
        {
          text: 'Integration & Embedding',
          collapsed: false,
          items: [
            { text: 'Embedding in Rust', link: '/docs/embedding' },
            { text: 'Embedding in JavaScript', link: '/docs/embedding-js' }
          ]
        }
      ]
    },

    search: {
      provider: 'local',
    },

    socialLinks: [
      { icon: 'github', link: 'https://github.com/HelgeSverre/sema' },
    ],

    outline: {
      level: [2, 3],
      label: 'On this page'
    },

    editLink: {
      pattern: 'https://github.com/HelgeSverre/sema/edit/main/website/:path',
      text: 'Edit this page on GitHub'
    },

    lastUpdated: {
      text: 'Updated at',
      formatOptions: {
        dateStyle: 'medium',
        timeStyle: 'short'
      }
    },
  },

  srcExclude: ['**/node_modules/**'],

  // Per-page OpenGraph: point each page at its generated card
  // (public/og/<slug>.<ext>) and its canonical URL, replacing the global
  // defaults declared in `head`. Images are produced by scripts/generate-og.mjs.
  transformHead({ pageData, head }) {
    const rel = pageData.relativePath
    const img = `${SITE}/og/${ogSlug(rel)}.${OG_EXT}`

    // cleanUrls: extensionless canonical paths (no .html). Index pages map to
    // their directory; every other page is its path sans the .md extension.
    let path = rel.replace(/\.md$/, '')
    if (path === 'index') path = ''
    else if (path.endsWith('/index')) path = path.slice(0, -'/index'.length) + '/'
    const url = `${SITE}/${path}`

    const title = (pageData.frontmatter?.title as string) || pageData.title
    const description = (pageData.frontmatter?.description as string) || pageData.description

    const override: Record<string, string> = {
      'og:image': img,
      'og:image:width': String(OG_WIDTH),
      'og:image:height': String(OG_HEIGHT),
      'twitter:image': img,
      'og:url': url,
    }
    if (title) {
      override['og:title'] = title
      override['twitter:title'] = title
    }
    if (description) {
      override['og:description'] = description
      override['twitter:description'] = description
    }

    // VitePress *appends* whatever this hook returns to the existing `head`, so we
    // must return ONLY the new/override tags — returning the whole head array would
    // emit every global tag (fonts, favicon, analytics…) a second time. Drop the
    // placeholders we're overriding from `head` in place, then append the real ones.
    for (let i = head.length - 1; i >= 0; i--) {
      const [tag, attrs] = head[i] as [string, Record<string, string>]
      const key = attrs?.property || attrs?.name
      if (tag === 'link' && attrs?.rel === 'canonical') head.splice(i, 1)
      else if (tag === 'meta' && key in override) head.splice(i, 1)
    }
    const added: [string, Record<string, string>][] = []
    for (const [key, content] of Object.entries(override)) {
      const attr = key.startsWith('twitter:') ? 'name' : 'property'
      added.push(['meta', { [attr]: key, content }])
    }
    // Explicit canonical so the clean URL is the one search engines consolidate to.
    added.push(['link', { rel: 'canonical', href: url }])
    return added
  },

  markdown: {
    languages: [semaLang as any],
    theme: { light: semaCodeTheme as any, dark: semaCodeTheme as any },
  },
})
