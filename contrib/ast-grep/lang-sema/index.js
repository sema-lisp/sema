const path = require('node:path')
const fs = require('node:fs')
const { resolvePrebuild } = require('@ast-grep/setup-lang')

function getLibPath() {
  const prebuild = resolvePrebuild(__dirname)
  if (prebuild) {
    return prebuild
  }

  const native = path.join(__dirname, 'parser.so')
  if (fs.existsSync(native)) {
    return native
  }

  throw new Error('No parser found. Please ensure the parser is built or a prebuild is available.')
}

let libPath

module.exports = {
  get libraryPath() {
    if (!libPath) {
      libPath = getLibPath()
    }
    return libPath
  },
  extensions: ['sema'],
  languageSymbol: 'tree_sitter_sema',
  expandoChar: '_',
}
