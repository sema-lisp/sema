// Splitter wiring. The drag/keyboard/ARIA behaviour lives in <sema-splitter>; this
// module owns the layout — it applies each splitter's resize delta (or absolute
// keyboard target) to the CSS custom properties that size the panes, and persists.

const STORAGE_KEY = 'sema-playground';

function loadState() {
  try { return JSON.parse(localStorage.getItem(STORAGE_KEY)) || {}; } catch { return {}; }
}

function saveState(patch) {
  const state = { ...loadState(), ...patch };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
}

/**
 * Wire a <sema-splitter> to a size. `get`/`set` read/apply the current size;
 * `clamp` bounds it; `persist` saves on release. `invert` flips the delta (a bar
 * whose pane grows as you drag *up*).
 */
function wire(id, { get, set, clamp, persist, invert = false }) {
  const sp = document.getElementById(id);
  if (!sp) return;
  let base = null;
  sp.setValue(get());
  sp.addEventListener('sema-resize-start', () => { base = get(); });
  sp.addEventListener('sema-resize', (e) => {
    if (base == null) base = get();
    const d = invert ? -e.detail.delta : e.detail.delta;
    const size = clamp(e.detail.absolute ? e.detail.delta : base + d);
    set(size);
    sp.setValue(size);
  });
  sp.addEventListener('sema-resize-end', () => { base = null; persist(); });
}

export function initSplitters() {
  const mainEl = document.querySelector('main');
  const rightCol = document.querySelector('.right-column');
  const filesBody = document.getElementById('files-body');
  const saved = loadState();

  // ── 1. Sidebar width ──
  let sidebarW = saved.sidebarW ?? 200;
  mainEl.style.setProperty('--sidebar-w', sidebarW + 'px');

  // ── 2. Editor / right-column ratio ──
  let editorRatio = saved.editorRatio ?? 0.55;
  function applyEditorRatio() {
    const available = mainEl.clientWidth - sidebarW - 8;
    mainEl.style.setProperty('--right-col-w', Math.round(available * (1 - editorRatio)) + 'px');
  }
  applyEditorRatio();
  window.addEventListener('resize', applyEditorRatio);

  // ── 3. Output / files height ──
  let filesH = saved.filesH ?? 200;
  filesBody.style.setProperty('--files-h', filesH + 'px');

  // ── 4. File tree width ──
  let filetreeW = saved.filetreeW ?? 200;
  filesBody.style.setProperty('--filetree-w', filetreeW + 'px');

  wire('splitter-sidebar', {
    get: () => sidebarW,
    set: (v) => { sidebarW = v; mainEl.style.setProperty('--sidebar-w', v + 'px'); applyEditorRatio(); },
    clamp: (v) => Math.max(120, Math.min(400, v)),
    persist: () => saveState({ sidebarW }),
  });

  // Editor ratio: the drag delta changes the editor's pixel width; store as a ratio.
  const editorSp = document.getElementById('splitter-editor');
  if (editorSp) {
    let base = null;
    const startRatio = () => { base = editorRatio; };
    editorSp.addEventListener('sema-resize-start', startRatio);
    editorSp.addEventListener('sema-resize', (e) => {
      if (base == null) base = editorRatio;
      const available = mainEl.clientWidth - sidebarW - 8;
      const editorW = Math.max(200, Math.min(available - 200, Math.round(available * base) + e.detail.delta));
      editorRatio = editorW / available;
      applyEditorRatio();
    });
    editorSp.addEventListener('sema-resize-end', () => { base = null; saveState({ editorRatio }); });
  }

  wire('splitter-output', {
    get: () => filesH,
    set: (v) => { filesH = v; filesBody.style.setProperty('--files-h', v + 'px'); },
    clamp: (v) => Math.max(60, Math.min(rightCol.clientHeight - 120, v)),
    persist: () => saveState({ filesH }),
    invert: true,
  });

  wire('splitter-filetree', {
    get: () => filetreeW,
    set: (v) => { filetreeW = v; filesBody.style.setProperty('--filetree-w', v + 'px'); },
    clamp: (v) => Math.max(100, Math.min(400, v)),
    persist: () => saveState({ filetreeW }),
  });
}
