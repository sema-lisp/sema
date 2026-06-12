---
name: "load"
module: "special-forms"
syntax: "(load \"filename\")"
---

Load and evaluate a Sema source file in the current environment. The file is read, parsed, and each top-level expression is evaluated sequentially. The last expression's value is returned. Unlike `import`, `load` does not use the module system — all top-level definitions from the loaded file become available directly in the current scope.

Paths are resolved relative to the current file's directory. If the path is absolute, it is used as-is. `load` checks the Virtual File System (VFS) first, which is used for bundled executables and embedded packages. The `load` form requires the `FS_READ` sandbox capability.

**Warning:** Using `load` pollutes the current namespace. Prefer `import` for structured module dependencies.

```sema
(load "helpers.sema")
```

Loading a file that defines utilities:

```sema
;; helpers.sema
(define (greet name)
  (format "Hello, ~a" name))

;; main.sema
(load "helpers.sema")
(greet "World")  ; => "Hello, World"
```

`load` returns the value of the last expression evaluated in the file:

```sema
(define result (load "config.sema"))
;; if config.sema ends with (define port 8080), result is 8080
```
