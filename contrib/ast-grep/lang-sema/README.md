# @ast-grep/lang-sema

Sema language support for [@ast-grep/napi](https://www.npmjs.com/package/@ast-grep/napi), built on the official [`tree-sitter-sema`](https://github.com/sema-lisp/tree-sitter-sema) grammar.

## Usage

```js
import sema from '@ast-grep/lang-sema'
import { registerDynamicLanguage, parse } from '@ast-grep/napi'

registerDynamicLanguage({ sema })

const sg = parse('sema', '(define greeting "hello")')
const def = sg.root().find('(define $NAME $VAL)')
def.getMatch('NAME').text() // => 'greeting'
```

Metavariables use `expandoChar: '_'` internally because `$` is not a valid
symbol character in Sema — patterns are still written with `$VAR`/`$$$ARGS`
as usual.

See the [Sema ast-grep guide](https://sema-lang.com/docs/ast-grep) for CLI
usage, lint rules, and the list of node kinds.
