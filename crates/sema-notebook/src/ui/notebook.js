/* Sema Notebook — Alpine.js Component (ES module) */
import { toast } from './vendor/sema-ui.js';

document.addEventListener('alpine:init', () => {
  Alpine.data('notebook', () => ({
    // ── State ──
    cells: [],
    title: 'Untitled',
    focusedCellId: null,
    canUndo: false,
    shiftEnterUsed: localStorage.getItem('sema-nb-shift-enter-used') === 'true',
    resetDialogOpen: false,

    // ── Lifecycle ──
    init() {
      this.load();
    },

    // ── API helper ──
    async api(method, path, body) {
      const opts = { method, headers: { 'Content-Type': 'application/json' } };
      if (body !== undefined) opts.body = JSON.stringify(body);
      const res = await fetch(path, opts);
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || res.statusText);
      }
      return res.json();
    },

    // ── Data loading ──
    async load() {
      try {
        const data = await this.api('GET', '/api/notebook');
        this.title = data.title || 'Untitled';
        this.canUndo = !!data.can_undo;
        // The notebook owns the markdown edit<->render toggle (`_rendered`): preserve
        // it across reloads; new/empty markdown opens in edit, content-bearing renders.
        const prev = {};
        this.cells.forEach(c => { if (c._rendered !== undefined) prev[c.id] = c._rendered; });
        this.cells = (data.cells || []).map(c => {
          if (c.cell_type === 'markdown') {
            c._rendered = prev[c.id] !== undefined ? prev[c.id] : !!(c.source && c.source.trim());
          }
          return c;
        });
      } catch (e) {
        console.error('Failed to load notebook:', e);
      }
    },

    // ── Cell evaluation ──
    async evalCell(id) {
      const cell = this.cells.find(c => c.id === id);
      if (!cell) return;
      // Sync source to server
      try { await this.api('POST', '/api/cells/' + id, { source: cell.source }); } catch (e) { /* ignore */ }
      cell._loading = true;
      try {
        await this.api('POST', '/api/cells/' + id + '/eval');
        const idx = this.cells.findIndex(c => c.id === id);
        if (idx < this.cells.length - 1) this.focusedCellId = this.cells[idx + 1].id;
        this.shiftEnterUsed = true;
        localStorage.setItem('sema-nb-shift-enter-used', 'true');
        await this.load();
      } catch (e) {
        cell._loading = false;
        console.error('Eval failed:', e);
      }
    },

    async evalCellStay(id) {
      const cell = this.cells.find(c => c.id === id);
      if (!cell) return;
      try { await this.api('POST', '/api/cells/' + id, { source: cell.source }); } catch (e) { /* ignore */ }
      cell._loading = true;
      try {
        await this.api('POST', '/api/cells/' + id + '/eval');
        this.focusedCellId = id;
        await this.load();
      } catch (e) {
        cell._loading = false;
        console.error('Eval failed:', e);
      }
    },

    async evalAll() {
      const sources = this.cells
        .filter(c => c.cell_type === 'code')
        .map(c => [c.id, c.source]);
      try {
        await this.api('POST', '/api/eval-all', { sources });
        await this.load();
      } catch (e) {
        console.error('Eval all failed:', e);
      }
    },

    // The editable control lives in the editor's shadow root; the host element's
    // focus() delegates into it. (A new/empty markdown cell opens in edit mode, so
    // it too renders a <sema-editor>.) `sema-editor` upgrades asynchronously (it
    // registers from a `type="module"` script), so a focus() called right after
    // Alpine renders the host can land before upgrade and silently no-op —
    // customElements.whenDefined guards against that race.
    focusCellEditor(id) {
      this.$nextTick(() => {
        customElements.whenDefined('sema-editor').then(() => {
          const el = document.querySelector('#cell-' + id + ' sema-editor');
          if (el) el.focus();
        });
      });
    },

    // ── Cell management ──
    async addCell(type, afterId) {
      try {
        const body = { type, source: '' };
        if (afterId) body.after = afterId;
        const data = await this.api('POST', '/api/cells', body);
        await this.load();
        this.focusedCellId = data.id;
        this.focusCellEditor(data.id);
      } catch (e) {
        console.error('Failed to create cell:', e);
      }
    },

    async insertCell(type, afterId) {
      const body = { type, source: '' };
      if (afterId && afterId !== 'top') body.after = afterId;
      try {
        const data = await this.api('POST', '/api/cells', body);
        await this.load();
        this.focusedCellId = data.id;
        this.focusCellEditor(data.id);
      } catch (e) {
        console.error('Failed to insert cell:', e);
      }
    },

    async deleteCell(id) {
      try {
        await this.api('DELETE', '/api/cells/' + id);
        if (this.focusedCellId === id) this.focusedCellId = null;
        await this.load();
      } catch (e) {
        console.error('Delete failed:', e);
      }
    },

    async moveCell(id, dir) {
      const idx = this.cells.findIndex(c => c.id === id);
      const newIdx = idx + dir;
      if (newIdx < 0 || newIdx >= this.cells.length) return;
      const ids = this.cells.map(c => c.id);
      [ids[idx], ids[newIdx]] = [ids[newIdx], ids[idx]];
      try {
        await this.api('POST', '/api/cells/reorder', { cell_ids: ids });
        await this.load();
      } catch (e) {
        console.error('Move failed:', e);
      }
    },

    // ── Save / Undo / Reset ──
    async save() {
      try {
        // Flush every cell's current source to the server first. Edits that were
        // never evaluated (markdown, un-run code) live only in the browser, and
        // the server serializes its own copy — so without this, save writes
        // stale content and the edits appear lost.
        await Promise.all([this.persistTitle(), ...this.cells.map(c => this.persistSource(c))]);
        await this.api('POST', '/api/save');
        toast.success('Saved');
      } catch (e) {
        toast.error('Save failed: ' + e.message);
      }
    },

    async undo() {
      try {
        const data = await this.api('POST', '/api/undo');
        this.canUndo = !!data.can_undo;
        await this.load();
      } catch (e) {
        console.error('Undo failed:', e);
      }
    },

    openReset() {
      this.resetDialogOpen = true;
    },

    async confirmReset() {
      this.resetDialogOpen = false;
      try {
        await this.api('POST', '/api/reset');
        this.canUndo = false;
        await this.load();
      } catch (e) {
        console.error('Reset failed:', e);
      }
    },

    // ── Editing / persistence ──
    // Push a cell's current source to the server. Source edits otherwise reach
    // the server only when a code cell is evaluated, so markdown edits and
    // un-run code edits would be dropped on save/reload.
    persistSource(cell) {
      return this.api('POST', '/api/cells/' + cell.id, { source: cell.source }).catch(() => {});
    },

    // Push the notebook title to the server. Like cell source, the title is
    // client-only state until synced, so without this a renamed notebook would
    // save under its old title.
    persistTitle() {
      return this.api('POST', '/api/title', { title: this.title }).catch(() => {});
    },

    // ── Markdown edit <-> render (consumer-owned toggle over the primitives) ──
    // Click a rendered <sema-markdown> to edit its source in a <sema-editor>.
    editMarkdown(id) {
      const cell = this.cells.find(c => c.id === id);
      if (!cell) return;
      cell._rendered = false;
      this.focusedCellId = id;
      this.focusCellEditor(id);
    },

    // Editor lost focus (or Escape): persist, and return a non-empty markdown cell
    // to its rendered view (blur is the natural "done editing" signal).
    onBlur(cell) {
      this.persistSource(cell);
      if (cell.cell_type === 'markdown' && cell.source.trim()) {
        cell._rendered = true;
      }
    },

    // ── Keyboard / Input ──
    // Shift+Enter renders a markdown cell; in a code cell it evaluates.
    handleShiftEnter(cell) {
      if (cell.cell_type === 'markdown') {
        this.persistSource(cell);
        if (cell.source.trim()) cell._rendered = true;
      } else {
        this.evalCell(cell.id);
      }
    },

    formatMeta(output) {
      const parts = [];
      if (output.meta) {
        if (output.meta.duration_ms != null) parts.push(output.meta.duration_ms + 'ms');
        if (output.meta.cost_usd != null) parts.push('$' + output.meta.cost_usd.toFixed(4));
      }
      return parts.join(' \u00b7 ');
    },

    toggleOutput(header) {
      const chevron = header.querySelector('.output-chevron');
      const content = header.nextElementSibling;
      if (chevron) chevron.classList.toggle('collapsed');
      if (content) content.classList.toggle('collapsed');
    },
  }));
});
