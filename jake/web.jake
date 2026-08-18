# VitePress docs site (sema-lang.com). Namespaced as `site`.
# Vercel CLI is intentionally not a repo dep; install globally or via npx.

# website/ isn't an npm workspace member (root workspaces are packages/*), so
# it keeps its own node_modules; install it on demand so a wiped node_modules
# doesn't break `jake site.dev`. Guarded so a present install is a no-op.
@group site
@desc "Install website npm deps (skips if node_modules exists)"
task deps:
    @needs npm
    @cd website
    [ -d node_modules ] || npm install

@group site
@desc "Start the docs site dev server"
task dev: [deps]
    @cd website
    npm run dev

@group site
@desc "Build the docs site for production"
task build: [deps]
    @cd website
    npm run build

@group site
@desc "Build + preview the production site locally"
task preview: [build]
    @cd website
    npm run preview

# Check vendored OG assets and regenerate per-page cards. Run after editing the
# template, logo, page titles, or version; commit the images before deploying.
@group site
@desc "Regenerate per-page OpenGraph cards (public/og/*.jpg)"
task og: [deps]
    @cd website
    npm run og:check
    npm run og

@group site
@desc "Build + deploy the docs site to production (Vercel)"
@needs npx
task deploy: [og, build]
    @confirm "Deploy the docs site to production?"
    # `og` ran first (hash-idempotent) so the cards match the current version and
    # titles; they are git-tracked, so flag a regen that still needs committing.
    git status --porcelain website/public/og website/og-manifest.json playground/og-playground.jpg | grep -q . && echo "NOTE: OG cards regenerated — commit website/public/og + og-manifest.json (+ playground/og-playground.jpg)" || true
    # Vercel uploads only `website/`, so the workspace Cargo.toml is absent during the
    # remote build and config.ts cannot read the version from it. Pass it explicitly;
    # without this the hero silently renders without a version.
    cd website && npx vercel --prod --yes --build-env SEMA_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' ../Cargo.toml | head -1)"
    # A failed remote build leaves the previous deploy live — verify the promoted
    # site actually serves the current version before calling this done.
    sleep 5
    curl -sf https://sema-lang.com/ | grep -q "v$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1) · MIT" && echo "site.deploy: live hero serves v$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1) ✓" || { echo "site.deploy: the live hero does not show the current version — the deploy did not promote or SEMA_VERSION was lost" >&2; exit 1; }
