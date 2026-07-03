import { beforeEach, describe, expect, it, vi } from 'vitest'
import '../src/lib/sema-editor.js'
import type { SemaEditor } from '../src/lib/sema-editor.js'

async function waitFor(fn: () => unknown, timeout = 4000): Promise<void> {
  const start = performance.now()
  while (performance.now() - start < timeout) {
    if (fn()) return
    await new Promise((r) => setTimeout(r, 20))
  }
  throw new Error('waitFor timed out')
}

async function mount(attrs = ''): Promise<SemaEditor> {
  document.body.innerHTML = `<sema-editor ${attrs}></sema-editor>`
  const el = document.body.querySelector('sema-editor') as SemaEditor
  await el.updateComplete
  return el
}
const ta = (el: SemaEditor) => el.shadowRoot!.querySelector('textarea') as HTMLTextAreaElement
const hl = (el: SemaEditor) => el.shadowRoot!.querySelector('.hl') as HTMLElement
const gutterLines = (el: SemaEditor) => [...el.shadowRoot!.querySelectorAll('.gl')] as HTMLElement[]

describe('sema-editor', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })

  it('renders the value into the textarea and shows it in the overlay', async () => {
    const el = await mount()
    el.value = '(define x 1)'
    await el.updateComplete
    expect(ta(el).value).toBe('(define x 1)')
    expect(hl(el).textContent).toContain('(define x 1)')
  })

  it('highlights via the shared Shiki grammar once warm (same as sema-code)', async () => {
    const el = await mount()
    el.value = '(define x 1)'
    await el.updateComplete
    await waitFor(() => /class="tok-/.test(hl(el).innerHTML))
    expect(hl(el).innerHTML).toMatch(/tok-keyword/)
  })

  it('emits input with the new value on typing', async () => {
    const el = await mount()
    const spy = vi.fn()
    el.addEventListener('input', (e) => spy((e as CustomEvent).detail.value))
    ta(el).value = '42'
    ta(el).dispatchEvent(new InputEvent('input', { bubbles: true }))
    expect(spy).toHaveBeenCalledWith('42')
    expect(el.value).toBe('42')
  })

  it('Tab inserts tab-size spaces instead of moving focus', async () => {
    const el = await mount('tab-size="2"')
    const t = ta(el)
    t.focus()
    t.selectionStart = t.selectionEnd = 0
    t.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true }))
    expect(t.value.startsWith('  ')).toBe(true)
  })

  it('Shift+Tab removes up to tab-size leading spaces (dedent)', async () => {
    const el = await mount('tab-size="2"')
    const t = ta(el)
    el.value = '    (foo)'
    await el.updateComplete
    t.focus()
    t.selectionStart = t.selectionEnd = 6
    t.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true, cancelable: true }),
    )
    expect(el.value).toBe('  (foo)')
  })

  it('Tab indents every line of a multi-line selection', async () => {
    const el = await mount('tab-size="2"')
    const t = ta(el)
    el.value = 'a\nb'
    await el.updateComplete
    t.focus()
    t.selectionStart = 0
    t.selectionEnd = 3
    t.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true }))
    expect(el.value).toBe('  a\n  b')
  })

  it('Shift+Tab dedents every line of a multi-line selection', async () => {
    const el = await mount('tab-size="2"')
    const t = ta(el)
    el.value = '  a\n    b'
    await el.updateComplete
    t.focus()
    t.selectionStart = 0
    t.selectionEnd = el.value.length
    t.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true, cancelable: true }),
    )
    expect(el.value).toBe('a\n  b')
  })

  it('forwards the testid onto the inner textarea', async () => {
    const el = await mount('testid="cell-textarea"')
    expect(ta(el).getAttribute('data-testid')).toBe('cell-textarea')
  })

  it('lets native keydown reach the host (composed)', async () => {
    const el = await mount()
    const spy = vi.fn()
    el.addEventListener('keydown', (e) => {
      if ((e as KeyboardEvent).shiftKey) spy()
    })
    ta(el).dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Enter', shiftKey: true, bubbles: true, composed: true }),
    )
    expect(spy).toHaveBeenCalled()
  })

  // ── Gutter / breakpoints / current-line ──

  it('renders a line-number gutter with one entry per line', async () => {
    const el = await mount('line-numbers')
    el.value = 'a\nb\nc'
    await el.updateComplete
    expect(gutterLines(el).map((g) => g.textContent!.trim())).toEqual(['1', '2', '3'])
  })

  it('marks breakpoint lines and the current line in the gutter', async () => {
    const el = await mount('line-numbers current-line="1"')
    el.value = 'a\nb\nc'
    el.breakpoints = [2]
    await el.updateComplete
    const gl = gutterLines(el)
    expect(gl[1].classList.contains('bp')).toBe(true) // line 2 breakpoint
    expect(gl[0].classList.contains('cur')).toBe(true) // line 1 current
    expect(gl[2].classList.contains('bp')).toBe(false)
  })

  it('emits gutter-click with the 1-based line when a gutter line is clicked', async () => {
    const el = await mount('line-numbers')
    el.value = 'a\nb\nc'
    await el.updateComplete
    const spy = vi.fn()
    el.addEventListener('gutter-click', (e) => spy((e as CustomEvent).detail.line))
    gutterLines(el)[2].click()
    expect(spy).toHaveBeenCalledWith(3)
  })

  it('has no gutter unless line-numbers is set', async () => {
    const el = await mount()
    el.value = 'a\nb'
    await el.updateComplete
    expect(gutterLines(el).length).toBe(0)
  })
})
