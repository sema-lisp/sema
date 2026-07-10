import init, { SemaInterpreter, formatCode } from '../pkg/sema_wasm.js';
import { examples } from './examples.js';
import { makeVfsHost, BACKENDS } from './vfs-backends.js';
import { initSplitters } from './splitters.js';
import { workerEvalEnabled, initWorker, evalViaWorker, cancelWorker, setWorkerOutputHandler } from './worker-client.js';
import { toast } from './sema-ui.js';

let interp = null;
// When true, eval runs on a Web Worker (real wall-clock async/sleep, responsive
// UI, cancellable). Active when the page is cross-origin isolated; opt out with
// ?no-worker. See worker-client.js.
let workerActive = false;
// True while a worker eval is in flight (so the Run button acts as Stop).
let workerRunning = false;
let vfsHost = null;
let vfsBackend = null;
let backendName = 'memory';
let activeFilePath = null;
let breakpoints = new Set();
let currentDebugLine = null;
let debugState = 'idle'; // 'idle' | 'running' | 'paused'

const STORAGE_KEY = 'sema-playground';

function loadState() {
  try { return JSON.parse(localStorage.getItem(STORAGE_KEY)) || {}; } catch { return {}; }
}

function saveState(patch) {
  const state = { ...loadState(), ...patch };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
}

// ── Files panel collapse ──

const filesPanel = document.getElementById('files-panel');
const filesBody = document.getElementById('files-body');
const collapseBtn = document.getElementById('files-collapse-btn');

collapseBtn.addEventListener('click', () => {
  const collapsed = filesBody.classList.toggle('collapsed');
  collapseBtn.textContent = collapsed ? '▸' : '▾';
  saveState({ filesCollapsed: collapsed });
});

// Restore collapsed state
const savedFilesCollapsed = loadState().filesCollapsed;
if (savedFilesCollapsed) {
  filesBody.classList.add('collapsed');
  collapseBtn.textContent = '▸';
}

// ── Example sidebar ──

// Flat lookup: example id -> { id, name, code }.
const examplesById = new Map();
for (const cat of examples) for (const f of cat.files) examplesById.set(f.id, f);

// Resolve a `?example=` query-param value to an example. Accepts either the
// full id (`getting-started/hello.sema`) or a bare filename (`hello.sema`),
// case-insensitively and with an optional `.sema` suffix — so shared links can
// be terse. Returns the example object, or null if nothing matches.
function resolveExampleParam(raw) {
  if (!raw) return null;
  const want = raw.trim().toLowerCase().replace(/\.sema$/, '');
  for (const f of examplesById.values()) {
    const id = f.id.toLowerCase().replace(/\.sema$/, '');
    const name = f.name.toLowerCase().replace(/\.sema$/, '');
    if (id === want || name === want) return f;
  }
  return null;
}

function loadExample(file) {
  editorEl.value = file.code;
  editorEl.resetHistory();
  scheduleHighlight();
  saveState({ lastExampleId: file.id, editorContent: file.code });
}

// Examples sidebar — dogfoods <sema-tree>: categories are expandable items, files
// are selectable leaves. sema-tree owns expand/collapse, keyboard nav, and ARIA.
function buildSidebar() {
  const tree = document.getElementById('sidebar-tree');
  const saved = loadState();
  const savedCollapsed = saved.collapsed || [];

  for (const cat of examples) {
    const catItem = document.createElement('sema-tree-item');
    catItem.setAttribute('label', cat.category);
    catItem.setAttribute('has-children', '');
    const collapsed = savedCollapsed.length > 0
      ? savedCollapsed.includes(cat.category)
      : cat.category !== 'Getting Started';
    if (!collapsed) catItem.setAttribute('expanded', '');

    for (const file of cat.files) {
      const fileItem = document.createElement('sema-tree-item');
      fileItem.setAttribute('label', file.name);
      fileItem.dataset.exampleId = file.id;
      catItem.appendChild(fileItem);
    }
    tree.appendChild(catItem);
  }

  // Every click emits sema-tree-select (a category toggles expand first), so one
  // handler both loads a picked file and persists which categories are collapsed.
  tree.addEventListener('sema-tree-select', (e) => {
    const el = e.detail.element;
    const collapsed = [...tree.querySelectorAll('sema-tree-item[has-children]')]
      .filter((it) => !it.expanded)
      .map((it) => it.getAttribute('label'));
    saveState({ collapsed });

    const id = el?.dataset?.exampleId;
    if (id && examplesById.has(id)) {
      tree.querySelectorAll('sema-tree-item').forEach((it) => { it.selected = it === el; });
      loadExample(examplesById.get(id));
    }
  });

  // A `?example=` query param auto-opens that example, overriding saved state —
  // this makes a URL a shareable direct link to a specific example.
  const requested = resolveExampleParam(new URLSearchParams(location.search).get('example'));
  const restoreId = requested ? requested.id : saved.lastExampleId;

  if (requested) loadExample(requested);
  else if (saved.editorContent) editorEl.value = saved.editorContent;

  if (restoreId) {
    const fileItem = tree.querySelector(`sema-tree-item[data-example-id="${CSS.escape(restoreId)}"]`);
    if (fileItem) {
      fileItem.selected = true;
      const parent = fileItem.parentElement; // the category <sema-tree-item>
      if (parent && parent.tagName?.toLowerCase() === 'sema-tree-item') parent.setAttribute('expanded', '');
    }
  }
}

// ── VFS File Tree ──

const fileTreeEl = document.getElementById('file-tree');
const fileViewerEl = document.getElementById('file-viewer');

// The persistent <sema-tree> root, built once and patched in place by
// reconcileVfsItems() on every refresh (null while the VFS is empty and the
// placeholder text is shown instead).
let vfsTreeEl = null;

function buildVfsTree(dir) {
  let entries;
  try { entries = interp.listFiles(dir); } catch { return []; }
  if (!entries || entries.length === 0) return [];

  const items = [];
  for (const name of entries) {
    const fullPath = dir === '/' ? '/' + name : dir + '/' + name;
    const isDir = interp.isDirectory(fullPath);
    items.push({ name, fullPath, isDir, children: isDir ? buildVfsTree(fullPath) : null });
  }

  items.sort((a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
    return a.name.localeCompare(b.name);
  });

  return items;
}

// Dogfoods <sema-tree> like the examples sidebar: directories are expandable
// parents, files are selectable leaves carrying their path. sema-tree owns the
// chevron, indentation, expand/collapse, keyboard nav, and ARIA.
//
// Patches `parent`'s <sema-tree-item> children to match `items` in place:
// existing nodes are matched by name+kind and reused, so a directory the user
// collapsed stays collapsed across refreshes; only additions/removals touch
// the DOM, and reordering moves existing nodes via insertBefore. Selection
// (`.selected`) is out of scope here — refreshFileTree() resyncs it from
// `activeFilePath` in one pass after the whole tree has settled.
function reconcileVfsItems(items, parent) {
  const existing = new Map();
  for (const child of parent.children) {
    if (child.tagName === 'SEMA-TREE-ITEM') existing.set(child.getAttribute('label'), child);
  }

  let prev = null;
  for (const item of items) {
    let node = existing.get(item.name);
    if (node && node.hasAttribute('has-children') === item.isDir) {
      existing.delete(item.name);
    } else {
      node = document.createElement('sema-tree-item');
      node.setAttribute('label', item.name);
      if (item.isDir) {
        node.setAttribute('has-children', '');
        node.setAttribute('expanded', ''); // directories start expanded
      }
    }

    if (item.isDir) {
      reconcileVfsItems(item.children, node);
    } else {
      node.dataset.path = item.fullPath;
    }

    const ref = prev ? prev.nextSibling : parent.firstChild;
    if (ref !== node) parent.insertBefore(node, ref);
    prev = node;
  }

  // Anything left in `existing` was removed from the VFS (or changed kind).
  for (const node of existing.values()) node.remove();
}

function refreshFileTree() {
  if (!interp) return;
  const items = buildVfsTree('/');

  if (items.length === 0) {
    vfsTreeEl = null;
    fileTreeEl.innerHTML = '<div class="vfs-tree-empty">(empty — run code to create files)</div>';
    return;
  }

  if (!vfsTreeEl) {
    fileTreeEl.innerHTML = '';
    vfsTreeEl = document.createElement('sema-tree');
    // A leaf carries data-path; a directory click just toggles expansion.
    vfsTreeEl.addEventListener('sema-tree-select', (e) => {
      const path = e.detail.element?.dataset?.path;
      if (path) viewFile(path);
    });
    fileTreeEl.appendChild(vfsTreeEl);
  }

  reconcileVfsItems(items, vfsTreeEl);

  for (const node of vfsTreeEl.querySelectorAll('sema-tree-item[data-path]')) {
    node.selected = node.dataset.path === activeFilePath;
  }
}

// ── File Viewer ──

function viewFile(path) {
  activeFilePath = path;
  const content = interp.readFile(path);
  fileViewerEl.innerHTML = '';
  fileViewerEl.textContent = content ?? '(empty file)';

  // Expand files panel if collapsed
  if (filesBody.classList.contains('collapsed')) {
    filesBody.classList.remove('collapsed');
    collapseBtn.textContent = '▾';
    saveState({ filesCollapsed: false });
  }

  refreshFileTree();
}

// ── VFS Stats ──

const vfsStatsEl = document.getElementById('vfs-stats');

function formatBytes(bytes) {
  if (bytes < 1024) return bytes + ' B';
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
  return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
}

function refreshVfsStats() {
  if (!interp) return;
  const s = interp.vfsStats();
  vfsStatsEl.textContent = s.files > 0 ? `${s.files} files · ${formatBytes(s.bytes)}` : '';
}

// ── Upload files into VFS ──

const uploadInput = document.getElementById('vfs-upload');
const uploadBtn = document.getElementById('upload-btn');
const dropOverlay = document.getElementById('drop-overlay');
const clearVfsBtn = document.getElementById('clear-vfs-btn');

uploadBtn.addEventListener('click', () => uploadInput.click());

uploadInput.addEventListener('change', async () => {
  if (uploadInput.files.length > 0) {
    await uploadFiles(uploadInput.files);
    uploadInput.value = '';
  }
});

// Drag and drop on the files panel
let dragCounter = 0;

filesPanel.addEventListener('dragenter', (e) => {
  e.preventDefault();
  dragCounter++;
  dropOverlay.classList.remove('hidden');
});

filesPanel.addEventListener('dragleave', (e) => {
  e.preventDefault();
  dragCounter--;
  if (dragCounter <= 0) {
    dragCounter = 0;
    dropOverlay.classList.add('hidden');
  }
});

filesPanel.addEventListener('dragover', (e) => {
  e.preventDefault();
});

filesPanel.addEventListener('drop', async (e) => {
  e.preventDefault();
  dragCounter = 0;
  dropOverlay.classList.add('hidden');
  if (e.dataTransfer.files.length > 0) {
    await uploadFiles(e.dataTransfer.files);
  }
});

async function uploadFiles(fileList) {
  if (!interp) return;

  interp.mkdir('/uploads');

  let uploaded = 0;
  for (const file of fileList) {
    if (file.size > 1024 * 1024) {
      toast.warning(`Skipped ${file.name} (>1MB)`);
      continue;
    }
    try {
      const text = await file.text();
      const path = '/uploads/' + file.name;
      interp.writeFile(path, text);
      uploaded++;
    } catch (e) {
      toast.error(`Upload failed: ${e.message}`);
    }
  }

  if (uploaded > 0) {
    toast.success(`Uploaded ${uploaded} file(s) to /uploads/`);

    if (backendName !== 'memory' && vfsBackend) {
      try { await vfsBackend.flush(vfsHost); } catch {}
    }

    // Expand files panel to show uploaded files
    if (filesBody.classList.contains('collapsed')) {
      filesBody.classList.remove('collapsed');
      collapseBtn.textContent = '▾';
      saveState({ filesCollapsed: false });
    }
  }

  refreshFileTree();
  refreshVfsStats();
}

// Clear VFS — gated behind a confirm dialog (destructive, irreversible).
const clearVfsDialog = document.getElementById('clear-vfs-dialog');

clearVfsBtn.addEventListener('click', () => {
  if (!interp) return;
  clearVfsDialog.show();
});

document.getElementById('clear-vfs-cancel-btn').addEventListener('click', () => clearVfsDialog.close());

document.getElementById('clear-vfs-confirm-btn').addEventListener('click', async () => {
  clearVfsDialog.close();
  interp.resetVFS();
  if (vfsBackend?.reset) await vfsBackend.reset();
  activeFilePath = null;
  fileViewerEl.innerHTML = '<div class="viewer-placeholder">Click a file to preview</div>';
  refreshFileTree();
  refreshVfsStats();
});

// ── Backend toggle ──

const backendToggle = document.getElementById('backend-toggle');

backendToggle.addEventListener('sema-change', async (e) => {
  const newName = e.detail.value;
  if (newName === backendName || !interp) return;

  const newBackend = BACKENDS[newName]();
  await newBackend.init?.();
  interp.resetVFS();
  await newBackend.hydrate(vfsHost);

  vfsBackend = newBackend;
  backendName = newName;
  saveState({ backend: newName });
  // <sema-toggle-group> owns the selected/active state.

  activeFilePath = null;
  fileViewerEl.innerHTML = '<div class="viewer-placeholder">Click a file to preview</div>';
  refreshFileTree();
  refreshVfsStats();
});

// ── Init ──

async function main() {
  buildSidebar();
  initSplitters();
  await init();
  interp = new SemaInterpreter();
  vfsHost = makeVfsHost(interp);

  // Opt-in worker path: run eval off the main thread for real async/sleep.
  if (workerEvalEnabled()) {
    try {
      await initWorker();
      workerActive = true;
      // Stream the worker's output lines into the pane live as they're produced.
      setWorkerOutputHandler((line) => {
        const div = document.createElement('div');
        div.className = 'output-line';
        div.setAttribute('data-testid', 'output-line');
        div.textContent = line;
        outputEl.appendChild(div);
        outputEl.scrollTop = outputEl.scrollHeight;
      });
    } catch (e) {
      console.warn('worker eval unavailable, using main thread:', e);
      workerActive = false;
    }
  }

  // Restore backend preference
  const saved = loadState();
  const storedBackend = saved.backend ?? 'memory';
  if (BACKENDS[storedBackend]) {
    backendName = storedBackend;
    backendToggle.value = storedBackend; // group reflects the selected toggle
  }

  vfsBackend = BACKENDS[backendName]();
  await vfsBackend.init?.();
  await vfsBackend.hydrate(vfsHost);

  document.getElementById('version').textContent = `v${interp.version()}`;
  document.getElementById('status').textContent = 'Ready';
  document.getElementById('status').className = 'status-text status-ready';
  document.getElementById('run-btn').disabled = false;
  document.getElementById('fmt-btn').disabled = false;
  document.getElementById('debug-btn').disabled = false;
  document.getElementById('output').innerHTML = '<div class="output-welcome">Ready. Write some Sema code and press Run.</div>';

  document.getElementById('loading').classList.add('hidden');
  refreshVfsStats();
}

// ── Run ──

const outputEl = document.getElementById('output');

async function run() {
  if (!interp) return;
  if (workerRunning) return; // a worker eval is already in flight
  const code = editorEl.value;
  if (!code.trim()) return;

  const runBtn = document.getElementById('run-btn');
  const statusEl = document.getElementById('status');

  // On the worker path the main thread stays free, so the Run button becomes a
  // live "Stop" (cancellation), and a "Running…" status can actually paint
  // (async/sleep paces in real wall-clock time). On the main-thread path the
  // button just disables (the UI is blocked during eval anyway).
  if (workerActive) {
    workerRunning = true;
    runBtn.textContent = 'Stop';
    runBtn.removeAttribute('shortcut'); // sema-button renders the shortcut badge; hide it while "Stop"
    runBtn.danger = true; // run-variant danger styling marks Stop as destructive
    statusEl.textContent = 'Running…';
    statusEl.className = 'status-text status-loading';
    // Clear now so streamed output lines (see setWorkerOutputHandler) land in a
    // fresh pane and appear live as the program runs.
    outputEl.innerHTML = '';
  } else {
    runBtn.disabled = true;
  }

  const t0 = performance.now();
  let result;
  if (workerActive) {
    // The worker owns eval; keep the main-thread interp as a synchronous VFS
    // mirror so the existing file-tree/preview/persistence code is unchanged.
    // Seed the worker with the mirror, then reflect any file changes back.
    const { result: r, vfs } = await evalViaWorker(code, interp.dumpVfs());
    result = r;
    interp.loadVfs(vfs);
  } else {
    result = await interp.evalVMAsync(code);
  }
  const elapsed = performance.now() - t0;

  if (workerActive) {
    workerRunning = false;
    runBtn.textContent = 'Run';
    runBtn.setAttribute('shortcut', '⌘↵'); // restore the badge (rendered by sema-button)
    runBtn.danger = false;
    const cancelled = result.error && result.error.includes('cancelled');
    statusEl.textContent = result.error ? (cancelled ? 'Stopped' : 'Error') : 'Ready';
    statusEl.className = result.error
      ? (cancelled ? 'status-text status-ready' : 'status-text status-error')
      : 'status-text status-ready';
  } else {
    runBtn.disabled = false;
  }

  // On the worker path the output lines already streamed in live (and the pane
  // was cleared at run start), so we only append the value/error + timing here.
  // On the main-thread path output is batched, so clear and render it now.
  if (!workerActive) {
    outputEl.innerHTML = '';
    if (result.output && result.output.length > 0) {
      for (const line of result.output) {
        const div = document.createElement('div');
        div.className = 'output-line';
        div.setAttribute('data-testid', 'output-line');
        div.textContent = line;
        outputEl.appendChild(div);
      }
    }
  }

  if (result.error) {
    const div = document.createElement('div');
    div.className = 'output-error';
    div.setAttribute('data-testid', 'output-error');
    div.textContent = result.error;
    outputEl.appendChild(div);
  } else if (result.value !== null) {
    const div = document.createElement('div');
    div.className = 'output-value';
    div.setAttribute('data-testid', 'output-value');
    div.textContent = `=> ${result.value}`;
    outputEl.appendChild(div);
  }

  const timing = document.createElement('div');
  timing.className = 'output-timing';
  timing.textContent = `Evaluated in ${elapsed.toFixed(1)}ms · bytecode VM`;
  outputEl.appendChild(timing);

  // Refresh VFS state
  refreshFileTree();
  refreshVfsStats();

  // Auto-flush for persistent backends
  if (backendName !== 'memory') {
    try {
      await vfsBackend.flush(vfsHost);
    } catch (e) {
      toast.error(`Persist failed: ${e.message}`);
    }
  }

  // Re-read active file in case it changed
  if (activeFilePath && interp.fileExists(activeFilePath)) {
    const content = interp.readFile(activeFilePath);
    fileViewerEl.textContent = content ?? '(empty file)';
  }
}

// Run button (acts as Stop while a worker eval is in flight)
document.getElementById('run-btn').addEventListener('click', () => {
  if (workerActive && workerRunning) cancelWorker();
  else run();
});

// Format button
document.getElementById('fmt-btn').addEventListener('click', () => {
  const code = editorEl.value;
  if (!code.trim()) return;
  const result = formatCode(code, 80, 2, false);
  if (result.error) {
    outputEl.innerHTML = '';
    const div = document.createElement('div');
    div.className = 'output-error';
    div.setAttribute('data-testid', 'output-error');
    div.textContent = `Format error: ${result.error}`;
    outputEl.appendChild(div);
  } else if (result.formatted !== null) {
    editorEl.value = result.formatted;
    scheduleHighlight();
    debounceSaveEditor();
  }
});

// Clear button
document.getElementById('clear-btn').addEventListener('click', () => {
  outputEl.innerHTML = '';
});

// ── Editor (<sema-editor>: highlighting + gutter + breakpoints + undo built-in) ──

const editorEl = document.getElementById('editor');

// Push breakpoint + current-line state into the editor's gutter. Line numbers and
// syntax highlighting are rendered by the component from its own value.
function updateGutter() {
  editorEl.breakpoints = Array.from(breakpoints);
  editorEl.currentLine = currentDebugLine || 0;
}

// Kept for call-site compatibility: the editor highlights itself, so a refresh
// only needs to re-sync the gutter markers.
function scheduleHighlight() {
  updateGutter();
}

function setsEqual(a, b) {
  if (a.size !== b.size) return false;
  for (const v of a) if (!b.has(v)) return false;
  return true;
}

let validLinesCode = null; // editor content when validBreakpointLines was computed

/** Ensure validBreakpointLines is up-to-date for the current editor content. */
function ensureValidLines() {
  if (!interp) return;
  const code = editorEl.value;
  if (validBreakpointLines && validLinesCode === code) return;
  try {
    const lines = interp.getValidBreakpointLines(code);
    validBreakpointLines = new Set(lines);
    validLinesCode = code;
  } catch (_) {
    // Parse error — clear valid lines so no snapping happens
    validBreakpointLines = null;
    validLinesCode = null;
  }
}

function snapToValidLine(lineNum) {
  if (!validBreakpointLines || validBreakpointLines.has(lineNum)) return lineNum;
  const valid = Array.from(validBreakpointLines).sort((a, b) => a - b);
  let best = null;
  let bestDist = Infinity;
  for (const v of valid) {
    const d = Math.abs(v - lineNum);
    if (d < bestDist) { bestDist = d; best = v; }
  }
  return best !== null ? best : lineNum;
}

function toggleBreakpoint(lineNum) {
  ensureValidLines();
  if (breakpoints.has(lineNum)) {
    breakpoints.delete(lineNum);
  } else {
    lineNum = snapToValidLine(lineNum);
    if (breakpoints.has(lineNum)) {
      // Already have a breakpoint at the snapped line — toggle it off
      breakpoints.delete(lineNum);
    } else {
      breakpoints.add(lineNum);
    }
  }
  updateGutter();
  if (interp && interp.debugIsActive()) {
    interp.debugSetBreakpoints(Array.from(breakpoints));
  }
}

// ── Debug state machine ──

const debugBtn = document.getElementById('debug-btn');
const debugControls = document.getElementById('debug-controls');

function setDebugState(state) {
  debugState = state;
  const runBtn = document.getElementById('run-btn');
  const fmtBtn = document.getElementById('fmt-btn');

  switch (state) {
    case 'idle':
      debugBtn.disabled = false;
      runBtn.disabled = false;
      fmtBtn.disabled = false;
      debugControls.classList.add('hidden');
      editorEl.readonly = false;
      currentDebugLine = null;
      validBreakpointLines = null;
      updateGutter();
      const varsPanel = document.getElementById('debug-vars');
      if (varsPanel) varsPanel.remove();
      document.getElementById('status').textContent = 'Ready';
      document.getElementById('status').className = 'status-text status-ready';
      break;
    case 'running':
      debugBtn.disabled = true;
      runBtn.disabled = true;
      fmtBtn.disabled = true;
      debugControls.classList.remove('hidden');
      editorEl.readonly = true;
      document.getElementById('status').textContent = 'Debugging…';
      document.getElementById('status').className = 'status-text status-loading';
      break;
    case 'paused':
      debugBtn.disabled = true;
      runBtn.disabled = true;
      fmtBtn.disabled = true;
      debugControls.classList.remove('hidden');
      editorEl.readonly = true;
      document.getElementById('status').textContent = `Paused at line ${currentDebugLine}`;
      document.getElementById('status').className = 'status-text status-loading';
      break;
  }
}

function escapeHtml(s) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

let validBreakpointLines = null; // Set of lines that can have breakpoints

function handleDebugResult(result) {
  // Sync breakpoint positions if the response includes validation info
  if (result.validLines) {
    validBreakpointLines = new Set(result.validLines);
  }
  if (result.breakpoints) {
    // Replace user breakpoints with snapped positions from WASM
    const snapped = new Set(result.breakpoints);
    if (!setsEqual(breakpoints, snapped)) {
      breakpoints = snapped;
      updateGutter();
    }
  }

  if (result.output && result.output.length > 0) {
    for (const line of result.output) {
      const div = document.createElement('div');
      div.className = 'output-line';
      div.setAttribute('data-testid', 'output-line');
      div.textContent = line;
      outputEl.appendChild(div);
    }
  }

  if (result.status === 'stopped') {
    currentDebugLine = result.line;
    updateGutter();
    scrollToLine(result.line);
    updateVariablesPanel();
    setDebugState('paused');
  } else if (result.status === 'yielded') {
    // VM yielded to keep browser responsive — resume after yielding to event loop
    setTimeout(() => {
      if (debugState === 'running' && interp) {
        try {
          handleDebugResult(interp.debugPoll());
        } catch (e) {
          showDebugError(e);
        }
      }
    }, 0);
  } else if (result.status === 'http_needed') {
    // VM hit an HTTP call — perform the fetch and restart the debug session
    handleDebugHttpNeeded(result.request);
    return;
  } else if (result.status === 'finished') {
    if (result.value !== null && result.value !== undefined) {
      const div = document.createElement('div');
      div.className = 'output-value';
      div.setAttribute('data-testid', 'output-value');
      div.textContent = `=> ${result.value}`;
      outputEl.appendChild(div);
    }
    interp.debugStop();
    setDebugState('idle');
  } else if (result.status === 'error') {
    const div = document.createElement('div');
    div.className = 'output-error';
    div.setAttribute('data-testid', 'output-error');
    div.textContent = result.error;
    outputEl.appendChild(div);
    interp.debugStop();
    setDebugState('idle');
  }
}

let debugHttpRetries = 0;
const MAX_DEBUG_HTTP_RETRIES = 50;

async function handleDebugHttpNeeded(request) {
  debugHttpRetries++;
  if (debugHttpRetries > MAX_DEBUG_HTTP_RETRIES) {
    showDebugError(new Error('Exceeded maximum HTTP requests during debug session'));
    return;
  }

  try {
    // Let WASM perform the fetch and cache the response natively
    const success = await interp.debugPerformFetch(JSON.stringify(request));
    if (!success) {
      showDebugError(new Error(`HTTP fetch failed: ${request.method} ${request.url}`));
      return;
    }

    // Restart the debug session — cached response will be used this time
    const code = editorEl.value;
    outputEl.innerHTML = '';
    const result = interp.debugStart(code, Array.from(breakpoints));
    handleDebugResult(result);
  } catch (e) {
    showDebugError(e);
  }
}

function showDebugError(e) {
  const div = document.createElement('div');
  div.className = 'output-error';
  div.setAttribute('data-testid', 'output-error');
  div.textContent = e.message || String(e);
  outputEl.appendChild(div);
  try { interp.debugStop(); } catch (_) { /* ignore */ }
  setDebugState('idle');
}

function scrollToLine(line) {
  editorEl.scrollToLine(line);
}

function updateVariablesPanel() {
  const existing = document.getElementById('debug-vars');
  if (existing) existing.remove();

  if (debugState !== 'paused' || !interp) return;

  const locals = interp.debugGetLocals();
  if (!locals || !Array.isArray(locals) || locals.length === 0) return;

  const panel = document.createElement('div');
  panel.id = 'debug-vars';
  panel.className = 'debug-vars-panel';
  panel.setAttribute('data-testid', 'debug-vars');

  const header = document.createElement('div');
  header.className = 'debug-vars-header';
  header.textContent = 'Variables';
  panel.appendChild(header);

  for (const v of locals) {
    const row = document.createElement('div');
    row.className = 'debug-var-row';
    row.setAttribute('data-testid', 'debug-var-row');
    row.innerHTML = `<span class="debug-var-name" data-testid="debug-var-name">${escapeHtml(v.name)}</span> = <span class="debug-var-value" data-testid="debug-var-value">${escapeHtml(v.value)}</span> <span class="debug-var-type">(${escapeHtml(v.type)})</span>`;
    panel.appendChild(row);
  }

  outputEl.insertBefore(panel, outputEl.firstChild);
}

// Debug button
debugBtn.addEventListener('click', () => {
  if (!interp || debugState !== 'idle') return;
  const code = editorEl.value;
  if (!code.trim()) return;

  outputEl.innerHTML = '';
  setDebugState('running');
  debugHttpRetries = 0;

  try {
    const result = interp.debugStart(code, Array.from(breakpoints));
    handleDebugResult(result);
  } catch (e) {
    showDebugError(e);
  }
});

// Debug control buttons
document.getElementById('dbg-continue').addEventListener('click', () => {
  if (!interp || debugState !== 'paused') return;
  setDebugState('running');
  try { handleDebugResult(interp.debugContinue()); } catch (e) { showDebugError(e); }
});

document.getElementById('dbg-step-over').addEventListener('click', () => {
  if (!interp || debugState !== 'paused') return;
  setDebugState('running');
  try { handleDebugResult(interp.debugStepOver()); } catch (e) { showDebugError(e); }
});

document.getElementById('dbg-step-into').addEventListener('click', () => {
  if (!interp || debugState !== 'paused') return;
  setDebugState('running');
  try { handleDebugResult(interp.debugStepInto()); } catch (e) { showDebugError(e); }
});

document.getElementById('dbg-step-out').addEventListener('click', () => {
  if (!interp || debugState !== 'paused') return;
  setDebugState('running');
  try { handleDebugResult(interp.debugStepOut()); } catch (e) { showDebugError(e); }
});

document.getElementById('dbg-stop').addEventListener('click', () => {
  if (!interp) return;
  interp.debugStop();
  setDebugState('idle');
});

// ── Editor events ──
// The editor emits `input` (CustomEvent<{value}>) on edits; it also highlights,
// gutters, scroll-syncs, and manages undo internally. Clicking a gutter line fires
// `gutter-click` — we own the breakpoint policy (snap to valid lines).

editorEl.addEventListener('input', () => {
  debounceSaveEditor();
  // Invalidate valid breakpoint lines cache when code changes
  validBreakpointLines = null;
  validLinesCode = null;
});
editorEl.addEventListener('gutter-click', (e) => toggleBreakpoint(e.detail.line));

// Debounced editor content save
let saveTimer = 0;
function debounceSaveEditor() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveState({ editorContent: editorEl.value });
  }, 500);
}

// Keyboard shortcut: Cmd/Ctrl+Enter and Tab/Shift+Tab
editorEl.addEventListener('keydown', (e) => {
  // Debug keyboard shortcuts
  if (e.key === 'F5' && debugState === 'paused') {
    e.preventDefault();
    document.getElementById('dbg-continue').click();
    return;
  }
  if (e.key === 'F10' && debugState === 'paused') {
    e.preventDefault();
    document.getElementById('dbg-step-over').click();
    return;
  }
  if (e.key === 'F11' && !e.shiftKey && debugState === 'paused') {
    e.preventDefault();
    document.getElementById('dbg-step-into').click();
    return;
  }
  if (e.key === 'F11' && e.shiftKey && debugState === 'paused') {
    e.preventDefault();
    document.getElementById('dbg-step-out').click();
    return;
  }
  if (e.key === 'Escape' && debugState !== 'idle') {
    e.preventDefault();
    document.getElementById('dbg-stop').click();
    return;
  }
  if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
    e.preventDefault();
    run();
  }
  // Tab / Shift+Tab indentation is handled inside <sema-editor>.
});

// Highlight initial content
scheduleHighlight();

main().then(() => { scheduleHighlight(); });
