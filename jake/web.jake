# VitePress docs site (sema-lang.com). Namespaced as `site`.
# Vercel CLI is intentionally not a repo dep; install globally or via npx.

@group web
@desc "Start the docs site dev server"
task dev:
    @needs npm
    @cd website
    npm run dev

@group web
@desc "Build the docs site for production"
task build:
    @needs npm
    @cd website
    npm run build

@group web
@desc "Build + preview the production site locally"
task preview: [build]
    @cd website
    npm run preview

# Check vendored OG assets and regenerate per-page cards. Run after editing the
# template, logo, page titles, or version; commit the images before deploying.
@group web
@desc "Regenerate per-page OpenGraph cards (public/og/*.jpg)"
task og:
    @cd website
    npm run og:check
    npm run og

@group web
@desc "Build + deploy the docs site to production (Vercel)"
@needs npx
task deploy: [build]
    @confirm "Deploy the docs site to production?"
    @cd website
    npx vercel --prod --yes
