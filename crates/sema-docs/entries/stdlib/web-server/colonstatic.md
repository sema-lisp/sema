---
name: ":static"
module: "web-server"
section: "Static File Serving"
---

Serve an entire directory of static files using the `:static` route type in `http/router`. Files are served with automatic MIME types, cache headers, and path traversal protection.

```sema
(define routes
  [[:static "/assets" "./public"]
   [:get    "/*"      handle-spa]])

(http/serve (http/router routes) {:port 3000})
```

```bash
$ curl http://localhost:3000/assets/style.css
body { color: red; }

$ curl -I http://localhost:3000/assets/style.css
Content-Type: text/css
Cache-Control: public, max-age=3600
```

The `:static` route takes a URL prefix and a directory path. Requests matching the prefix are mapped to files in the directory:

- `GET /assets/style.css` → reads `./public/style.css`
- `GET /assets/js/app.js` → reads `./public/js/app.js`
- `GET /assets/` → reads `./public/index.html` (directory index)

**Fallthrough**: If a file doesn't exist, the route does *not* match — the router continues to the next route. This enables SPA (single-page application) patterns where a catch-all route serves `index.html` for client-side routing:

```sema
(define routes
  [[:static "/assets" "./dist/assets"]
   [:get    "/*"      (fn (_) (http/file "./dist/index.html"))]])

(http/serve (http/router routes) {:port 3000})
```

**Security**: Path traversal attempts (e.g. `../etc/passwd`) are rejected with a 400 response. Only GET and HEAD methods are accepted.
