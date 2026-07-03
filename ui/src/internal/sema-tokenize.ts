/**
 * Synchronous Sema tokenizer/highlighter for the live editor overlay.
 *
 * The editor repaints on every keystroke, so it needs a synchronous highlighter;
 * @sema/ui's Shiki path (`highlightToHtml`) is async and reserved for static
 * `sema-code` and markdown fences. Output uses the shared `tok-*` classes from
 * `styles/syntax.css`, so the overlay themes via the same `--syntax-*` variables.
 * Ported from `playground/src/highlight.js`; keep aligned with the canonical
 * TextMate grammar (`grammars/sema.tmLanguage.json`).
 */
export const SEMA_KEYWORDS = new Set<string>([
  'define', 'defun', 'lambda', 'fn', 'if', 'cond', 'case', 'when', 'unless',
  'let', 'let*', 'letrec', 'begin', 'do', 'and', 'or', 'not',
  'set!', 'quote', 'quasiquote', 'unquote', 'unquote-splicing',
  'define-record-type', 'defmacro', 'defagent', 'deftool',
  'try', 'catch', 'throw', 'error',
  'import', 'module', 'export', 'load', 'require',
  'delay', 'force', 'eval', 'macroexpand', 'with-budget', 'else',
  '->', '->>', 'as->', 'some->',
  'map', 'filter', 'foldl', 'foldr', 'reduce', 'for-each', 'apply',
]);

export interface SemaToken {
  type: string;
  text: string;
}

export function tokenizeSema(code: string): SemaToken[] {
  const tokens: SemaToken[] = [];
  let i = 0;
  while (i < code.length) {
    if (code[i] === ';') {
      const start = i;
      while (i < code.length && code[i] !== '\n') i++;
      tokens.push({ type: 'comment', text: code.slice(start, i) });
    } else if (code[i] === '"') {
      const start = i;
      i++;
      while (i < code.length && code[i] !== '"') {
        if (code[i] === '\\' && i + 1 < code.length) i++;
        i++;
      }
      if (i < code.length) i++; // closing quote
      tokens.push({ type: 'string', text: code.slice(start, i) });
    } else if ('()[]{}\'`,'.includes(code[i])) {
      tokens.push({ type: 'paren', text: code[i] });
      i++;
    } else if (/\s/.test(code[i])) {
      const start = i;
      while (i < code.length && /\s/.test(code[i])) i++;
      tokens.push({ type: 'ws', text: code.slice(start, i) });
    } else {
      const start = i;
      while (i < code.length && !/[\s()[\]{}"`;,]/.test(code[i])) i++;
      const word = code.slice(start, i);
      if (word === '#t' || word === '#f' || word === 'true' || word === 'false' || word === 'nil') {
        tokens.push({ type: 'boolean', text: word });
      } else if (/^-?\d+(\.\d+)?$/.test(word)) {
        tokens.push({ type: 'number', text: word });
      } else if (word.startsWith(':') && word.length > 1) {
        tokens.push({ type: 'keyword-lit', text: word });
      } else if (SEMA_KEYWORDS.has(word)) {
        tokens.push({ type: 'keyword', text: word });
      } else {
        tokens.push({ type: 'plain', text: word });
      }
    }
  }
  return tokens;
}

export function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

const TYPE_TO_CLASS: Record<string, string> = {
  comment: 'tok-comment',
  string: 'tok-string',
  number: 'tok-number',
  boolean: 'tok-boolean',
  'keyword-lit': 'tok-keyword-lit',
  keyword: 'tok-keyword',
  paren: 'tok-punctuation',
};

/**
 * Inner HTML for a `<pre>` overlay: classified tokens wrapped in `tok-*` spans,
 * everything HTML-escaped. Non-`sema` langs render as escaped plain text (the
 * overlay simply shows unhighlighted source). A trailing space is appended when
 * the source ends in a newline so the final (empty) line has height.
 */
export function highlightSemaSync(code: string, lang = 'sema'): string {
  if (lang !== 'sema') return escapeHtml(code);
  if (!code) return '\n';
  let html = '';
  for (const t of tokenizeSema(code)) {
    const escaped = escapeHtml(t.text);
    const cls = TYPE_TO_CLASS[t.type];
    html += cls ? `<span class="${cls}">${escaped}</span>` : escaped;
  }
  if (code.endsWith('\n')) html += ' ';
  return html;
}
