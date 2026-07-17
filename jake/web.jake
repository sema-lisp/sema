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
task deploy: [build]
    @confirm "Deploy the docs site to production?"
    @cd website
    npx vercel --prod --yes
