const { setup } = require('@ast-grep/nursery')
const sema = require('./index')
const assert = require('node:assert')

setup({
  dirname: __dirname,
  name: 'sema',
  treeSitterPackage: 'tree-sitter-sema',
  languageRegistration: sema,
  testRunner: parse => {
    const sg = parse('(define greeting "hello")')
    const root = sg.root()
    const def = root.find('(define $NAME $VAL)')
    assert.equal(def.kind(), 'list')
    assert.equal(def.getMatch('NAME').text(), 'greeting')
    assert.equal(def.getMatch('VAL').text(), '"hello"')
  },
})
