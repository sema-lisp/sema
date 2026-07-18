// Splitter wiring. <sema-splitter> owns pointer, touch, keyboard, and ARIA
// behaviour; this module maps its deltas onto the active responsive layout.

const STORAGE_KEY = 'sema-playground';
const MOBILE_QUERY = '(max-width: 768px)';
const SPLITTER_SIZE = 4;

const MOBILE_SIDEBAR_MIN = 96;
const MOBILE_EDITOR_MIN = 160;
const MOBILE_RIGHT_COLUMN_MIN = 320;
const MOBILE_OUTPUT_MIN = 96;
const MOBILE_FILE_PANE_MIN = 60;

function loadState() {
  try { return JSON.parse(localStorage.getItem(STORAGE_KEY)) || {}; } catch { return {}; }
}

function saveState(patch) {
  const state = { ...loadState(), ...patch };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
}

function savedNumber(state, key, fallback = null) {
  return Number.isFinite(state[key]) ? state[key] : fallback;
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(Math.max(min, max), value));
}

export function initSplitters() {
  const mainEl = document.querySelector('main');
  const rightCol = document.querySelector('.right-column');
  const filesBody = document.getElementById('files-body');
  const filesHeader = document.querySelector('.files-panel-header');
  const mobileQuery = window.matchMedia(MOBILE_QUERY);
  const saved = loadState();

  let sidebarW = savedNumber(saved, 'sidebarW', 200);
  let editorRatio = savedNumber(saved, 'editorRatio', 0.55);
  let filesH = savedNumber(saved, 'filesH', 200);
  let filetreeW = savedNumber(saved, 'filetreeW', 200);

  let mobileSidebarH = savedNumber(saved, 'mobileSidebarH');
  let mobileEditorH = savedNumber(saved, 'mobileEditorH');
  let mobileFilesH = savedNumber(saved, 'mobileFilesH');
  let mobileFiletreeH = savedNumber(saved, 'mobileFiletreeH');

  const splitters = {
    sidebar: document.getElementById('splitter-sidebar'),
    editor: document.getElementById('splitter-editor'),
    output: document.getElementById('splitter-output'),
    filetree: document.getElementById('splitter-filetree'),
  };

  function desktopEditorAvailable() {
    return Math.max(0, mainEl.clientWidth - sidebarW - SPLITTER_SIZE * 2);
  }

  function mobileMainAvailable() {
    return Math.max(0, mainEl.clientHeight - SPLITTER_SIZE * 2);
  }

  function mobileFilesMin() {
    return MOBILE_FILE_PANE_MIN * 2 + SPLITTER_SIZE;
  }

  function mobileFilesMax() {
    return Math.max(
      mobileFilesMin(),
      rightCol.clientHeight - filesHeader.offsetHeight - SPLITTER_SIZE - MOBILE_OUTPUT_MIN,
    );
  }

  function setDirection(splitter, direction) {
    if (splitter.getAttribute('direction') !== direction) {
      splitter.setAttribute('direction', direction);
    }
  }

  function applyDesktopLayout() {
    setDirection(splitters.sidebar, 'horizontal');
    setDirection(splitters.editor, 'horizontal');
    setDirection(splitters.output, 'vertical');
    setDirection(splitters.filetree, 'horizontal');

    mainEl.style.setProperty('--sidebar-w', sidebarW + 'px');
    const available = desktopEditorAvailable();
    mainEl.style.setProperty('--right-col-w', Math.round(available * (1 - editorRatio)) + 'px');
    filesBody.style.setProperty('--files-h', filesH + 'px');
    filesBody.style.setProperty('--filetree-w', filetreeW + 'px');
  }

  function applyMobileLayout() {
    setDirection(splitters.sidebar, 'vertical');
    setDirection(splitters.editor, 'vertical');
    setDirection(splitters.output, 'vertical');
    setDirection(splitters.filetree, 'vertical');

    const mainAvailable = mobileMainAvailable();
    const sidebarMax = mainAvailable - MOBILE_EDITOR_MIN - MOBILE_RIGHT_COLUMN_MIN;
    if (mobileSidebarH == null) mobileSidebarH = Math.min(160, sidebarMax);
    mobileSidebarH = clamp(mobileSidebarH, MOBILE_SIDEBAR_MIN, sidebarMax);

    const editorMax = mainAvailable - mobileSidebarH - MOBILE_RIGHT_COLUMN_MIN;
    if (mobileEditorH == null) mobileEditorH = Math.min(260, editorMax);
    mobileEditorH = clamp(mobileEditorH, MOBILE_EDITOR_MIN, editorMax);

    mainEl.style.setProperty('--mobile-sidebar-h', Math.round(mobileSidebarH) + 'px');
    mainEl.style.setProperty('--mobile-editor-h', Math.round(mobileEditorH) + 'px');

    const filesMin = mobileFilesMin();
    const filesMax = mobileFilesMax();
    if (mobileFilesH == null) mobileFilesH = Math.min(160, filesMax);
    mobileFilesH = clamp(mobileFilesH, filesMin, filesMax);
    filesBody.style.setProperty('--mobile-files-h', Math.round(mobileFilesH) + 'px');

    const filetreeMax = mobileFilesH - MOBILE_FILE_PANE_MIN - SPLITTER_SIZE;
    if (mobileFiletreeH == null) {
      mobileFiletreeH = Math.round((mobileFilesH - SPLITTER_SIZE) / 2);
    }
    mobileFiletreeH = clamp(mobileFiletreeH, MOBILE_FILE_PANE_MIN, filetreeMax);
    filesBody.style.setProperty('--mobile-filetree-h', Math.round(mobileFiletreeH) + 'px');
  }

  function sidebarConfig() {
    if (mobileQuery.matches) {
      return {
        get: () => mobileSidebarH,
        set: (value) => { mobileSidebarH = value; },
        bounds: () => ({
          min: MOBILE_SIDEBAR_MIN,
          max: mobileMainAvailable() - mobileEditorH - MOBILE_RIGHT_COLUMN_MIN,
        }),
        persist: () => saveState({ mobileSidebarH }),
      };
    }
    return {
      get: () => sidebarW,
      set: (value) => { sidebarW = value; },
      bounds: () => ({ min: 120, max: 400 }),
      persist: () => saveState({ sidebarW }),
    };
  }

  function editorConfig() {
    if (mobileQuery.matches) {
      return {
        get: () => mobileEditorH,
        set: (value) => { mobileEditorH = value; },
        bounds: () => ({
          min: MOBILE_EDITOR_MIN,
          max: mobileMainAvailable() - mobileSidebarH - MOBILE_RIGHT_COLUMN_MIN,
        }),
        persist: () => saveState({ mobileEditorH }),
      };
    }
    return {
      get: () => Math.round(desktopEditorAvailable() * editorRatio),
      set: (value) => {
        const available = desktopEditorAvailable();
        if (available > 0) editorRatio = value / available;
      },
      bounds: () => ({ min: 200, max: desktopEditorAvailable() - 200 }),
      persist: () => saveState({ editorRatio }),
    };
  }

  function outputConfig() {
    if (mobileQuery.matches) {
      return {
        get: () => mobileFilesH,
        set: (value) => { mobileFilesH = value; },
        bounds: () => ({ min: mobileFilesMin(), max: mobileFilesMax() }),
        persist: () => saveState({ mobileFilesH }),
        invert: true,
      };
    }
    return {
      get: () => filesH,
      set: (value) => { filesH = value; },
      bounds: () => ({ min: 60, max: rightCol.clientHeight - 120 }),
      persist: () => saveState({ filesH }),
      invert: true,
    };
  }

  function filetreeConfig() {
    if (mobileQuery.matches) {
      return {
        get: () => mobileFiletreeH,
        set: (value) => { mobileFiletreeH = value; },
        bounds: () => ({
          min: MOBILE_FILE_PANE_MIN,
          max: mobileFilesH - MOBILE_FILE_PANE_MIN - SPLITTER_SIZE,
        }),
        persist: () => saveState({ mobileFiletreeH }),
      };
    }
    return {
      get: () => filetreeW,
      set: (value) => { filetreeW = value; },
      bounds: () => ({ min: 100, max: 400 }),
      persist: () => saveState({ filetreeW }),
    };
  }

  const configs = {
    sidebar: sidebarConfig,
    editor: editorConfig,
    output: outputConfig,
    filetree: filetreeConfig,
  };

  function syncSplitter(splitter, config) {
    const { min, max } = config.bounds();
    splitter.min = min;
    splitter.max = Math.max(min, max);
    splitter.setValue(Math.round(config.get()));
  }

  function applyLayout() {
    if (mobileQuery.matches) applyMobileLayout();
    else applyDesktopLayout();

    for (const name of Object.keys(splitters)) {
      syncSplitter(splitters[name], configs[name]());
    }
  }

  function wire(name) {
    const splitter = splitters[name];
    let drag = null;

    splitter.addEventListener('sema-resize-start', () => {
      const config = configs[name]();
      drag = { config, base: config.get() };
    });
    splitter.addEventListener('sema-resize', (event) => {
      if (drag == null) {
        const config = configs[name]();
        drag = { config, base: config.get() };
      }

      const { config, base } = drag;
      const { min, max } = config.bounds();
      const delta = config.invert ? -event.detail.delta : event.detail.delta;
      const requested = event.detail.absolute ? event.detail.delta : base + delta;
      config.set(clamp(requested, min, max));
      applyLayout();
    });
    splitter.addEventListener('sema-resize-end', () => {
      if (drag != null) drag.config.persist();
      drag = null;
    });
  }

  for (const name of Object.keys(splitters)) wire(name);

  let resizeFrame = null;
  function scheduleLayout() {
    if (resizeFrame != null) cancelAnimationFrame(resizeFrame);
    resizeFrame = requestAnimationFrame(() => {
      resizeFrame = null;
      applyLayout();
    });
  }

  window.addEventListener('resize', scheduleLayout);
  mobileQuery.addEventListener('change', applyLayout);
  applyLayout();
}
