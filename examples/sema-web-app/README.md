# Sema Web Compiled App Example

This example shows the recommended production flow for a browser app built with
`@sema-lang/sema-web`:

- write your UI in `app.sema`
- load it with `<script type="text/sema">`
- build a compiled `.vfs` archive for production
- serve the result as static files

## What It Demonstrates

- compiled archive loading via `dist/app.vfs`
- reactive state with `state`, `computed`, `watch`, and `batch`
- component rendering with `defcomponent`, `mount!`, and `on-mount`
- hash routing via `router/*`
- scoped styles via `css`
- persistence via `store/*`

## Run It

From the repository root:

```bash
make sema-web-example
```

Then open:

```text
http://127.0.0.1:8788
```

That target:

1. builds the WASM package
2. builds the JS packages
3. compiles `app.sema` to `dist/app.vfs`
4. copies the built JS, WASM, and browser dependencies into `dist/vendor/`
5. serves only `examples/sema-web-app/` with a static file server

It uses `npx serve` and keeps the example self-contained inside its own folder.

## Notes

- The example uses an import map that points at vendored files inside `dist/vendor/`.
- For an app outside this repository, replace those vendored files with your published package or bundler setup.
