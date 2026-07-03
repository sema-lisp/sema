import { describe, expect, it } from 'vitest'
import { tokenizeSema, highlightSemaSync, escapeHtml } from '../src/internal/sema-tokenize.js'

describe('tokenizeSema', () => {
  it('classifies comments, strings, numbers, booleans, keyword-literals, keywords', () => {
    const types = (s: string) => tokenizeSema(s).map((t) => `${t.type}:${t.text}`)
    expect(types('; hi')).toEqual(['comment:; hi'])
    expect(types('"a\\"b"')).toEqual(['string:"a\\"b"'])
    expect(types('42')).toEqual(['number:42'])
    expect(types('#t')).toEqual(['boolean:#t'])
    expect(types(':key')).toEqual(['keyword-lit::key'])
    expect(types('define')).toEqual(['keyword:define'])
    expect(types('foo')).toEqual(['plain:foo'])
  })

  it('concatenated token text reconstructs the input exactly', () => {
    const src = '(define (sq x) ; c\n  (* x x))'
    expect(tokenizeSema(src).map((t) => t.text).join('')).toBe(src)
  })
})

describe('highlightSemaSync', () => {
  it('wraps classified tokens in tok-* spans and escapes html', () => {
    const html = highlightSemaSync('(define x "a<b")')
    expect(html).toContain('<span class="tok-keyword">define</span>')
    expect(html).toContain('<span class="tok-string">"a&lt;b"</span>')
    expect(html).toContain('<span class="tok-punctuation">(</span>')
  })

  it('returns escaped plain text for non-sema langs', () => {
    expect(highlightSemaSync('# h <x>', 'markdown')).toBe('# h &lt;x&gt;')
  })

  it('appends a space when the source ends in a newline (pre renders the final line)', () => {
    expect(highlightSemaSync('a\n').endsWith(' ')).toBe(true)
  })

  it('escapeHtml escapes &, <, >', () => {
    expect(escapeHtml('a & <b>')).toBe('a &amp; &lt;b&gt;')
  })
})
