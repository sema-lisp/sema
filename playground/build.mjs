import { readdirSync, readFileSync, writeFileSync, mkdirSync, existsSync, cpSync } from 'fs';
import { join } from 'path';
import { build } from 'esbuild';

const EXAMPLES_DIR = 'examples';
const OUTPUT_FILE = 'src/examples.js';
const DIST_DIR = 'dist';

// Category display names
const categoryNames = {
  'getting-started': 'Getting Started',
  'functional': 'Functional',
  'concurrency': 'Concurrency',
  'data': 'Data & Text',
  'filesystem': 'Filesystem',
  'patterns': 'Patterns',
  'visuals': 'Visuals',
  'math-crypto': 'Math & Crypto',
  'http': 'HTTP & APIs',
};

// Category display order
const categoryOrder = [
  'getting-started',
  'functional',
  'concurrency',
  'data',
  'filesystem',
  'http',
  'patterns',
  'visuals',
  'math-crypto',
];

// 1. Generate examples.js from .sema files
const dirs = readdirSync(EXAMPLES_DIR, { withFileTypes: true })
  .filter(d => d.isDirectory())
  .map(d => d.name);

// Sort directories by the defined order, unknown dirs go to the end
dirs.sort((a, b) => {
  const ai = categoryOrder.indexOf(a);
  const bi = categoryOrder.indexOf(b);
  return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
});

const categories = [];
for (const dir of dirs) {
  const dirPath = join(EXAMPLES_DIR, dir);
  const files = readdirSync(dirPath)
    .filter(f => f.endsWith('.sema'))
    .sort();

  const fileEntries = files.map(f => {
    const code = readFileSync(join(dirPath, f), 'utf-8');
    return `    { id: ${JSON.stringify(dir + '/' + f)}, name: ${JSON.stringify(f)}, code: ${JSON.stringify(code)} }`;
  });

  const displayName = categoryNames[dir] || dir.replace(/-/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
  categories.push(`  { category: ${JSON.stringify(displayName)}, files: [\n${fileEntries.join(',\n')}\n  ]}`);
}

const output = `export const examples = [\n${categories.join(',\n')}\n];\n`;
writeFileSync(OUTPUT_FILE, output);
console.log(`Generated ${OUTPUT_FILE} (${dirs.length} categories, ${dirs.reduce((n, d) => n + readdirSync(join(EXAMPLES_DIR, d)).filter(f => f.endsWith('.sema')).length, 0)} files)`);

// 2. Bundle with esbuild
if (!existsSync(DIST_DIR)) {
  mkdirSync(DIST_DIR, { recursive: true });
}

await build({
  entryPoints: ['src/app.js', 'src/sema-worker.js'],
  outdir: 'dist',
  bundle: true,
  format: 'esm',
  minify: false,
  sourcemap: true, // source-level debugging in DevTools (maps dist -> src)
  target: 'es2020',
  // '../pkg/*' is the WASM glue (loaded relative to dist/ at runtime, not part of
  // the bundle). './sema-ui.js' is the vendored @sema-lang/ui bundle below — app.js
  // imports `toast` from it; esbuild passes the specifier through unchanged, and
  // since it's already loaded as a module script before app.js, the browser
  // resolves both to the same dist/sema-ui.js module (no duplicate component
  // registration).
  external: ['../pkg/*', './sema-ui.js'],
});
console.log('Bundled dist/app.js + dist/sema-worker.js');

// 3. Vendor the @sema-lang/ui web-component bundle (provides <sema-editor>) and its
//    design tokens. Both come from the published npm package (`npm install`);
//    loaded by index.html before app.js / style.css respectively.
function vendorFromSemaUi(srcRelPath, destName, missingHint) {
  const src = join('node_modules/@sema-lang/ui', srcRelPath);
  if (existsSync(src)) {
    cpSync(src, join(DIST_DIR, destName));
    console.log(`Vendored dist/${destName} from @sema-lang/ui`);
  } else {
    console.warn(`WARNING: ${src} not found — run \`npm install\` first (${missingHint}).`);
  }
}
vendorFromSemaUi('dist/sema-ui.js', 'sema-ui.js', '<sema-editor> will be missing');
vendorFromSemaUi('src/styles/tokens.css', 'tokens.css', 'design tokens will be missing');
