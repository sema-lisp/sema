---
name: "import"
module: "special-forms"
syntax: "(import \"path\" sym1 sym2 ...)"
---

Load a module from a file or package and make its exported bindings available in the current environment. Unlike `load`, `import` uses the module system: only bindings declared with `export` inside a `module` form become visible. Non-exported bindings remain private.

The first argument is evaluated to a string path. It may be a relative path (resolved against the current file's directory), an absolute path, or a package identifier such as `github.com/user/repo`. The evaluator checks the virtual file system first (used by bundled executables), then falls back to the real filesystem.

Selective import is supported by listing bare symbols after the path. Only the named symbols are imported; if any symbol is not exported by the module, an error is raised. If no symbols are listed, all exported bindings are imported.

Modules are cached after the first load, so importing the same module multiple times in a single session is fast and idempotent. Cyclic imports are detected and return an error instead of causing infinite recursion.

```sema
;; Import all exports from a local module
(import "math-utils.sema")
(square 5)                              ; => 25
```

```sema
;; Selective import: only bring in specific symbols
(import "math-utils.sema" square cube)
```

```sema
;; Import from a package
(import "github.com/user/repo")
(greet "world")                         ; => "hello world"
```

**Note:** `import` requires at least one argument (the path). The sandbox may restrict file-system access for imports. Use `load` instead if you simply want to execute a file in the current scope without module privacy boundaries.
