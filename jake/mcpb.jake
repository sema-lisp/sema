# MCP Bundle (.mcpb) packaging. Namespaced as `mcpb`.
#
# Builds ONE cross-platform .mcpb from the per-target binaries cargo-dist
# attaches to a GitHub release (macOS lipo'd universal + both Linux arches
# behind a uname shim + Windows x64). Meant to run LAST in the release flow,
# on a macOS host (lipo is macOS-only). See docs/plans/archive/2026-07-17-mcpb-bundle.md.

@group mcpb
@desc "Validate the committed MCPB manifest + Linux shim"
@needs npx "install Node.js"
task validate:
    npx --yes @anthropic-ai/mcpb@latest validate mcpb/manifest.json
    sh -n mcpb/sema-linux
    echo "mcpb: manifest + shim OK"

@group mcpb
@desc "Build sema.mcpb from a release tag's assets (tag=vX.Y.Z, default: latest)"
@needs gh "brew install gh"
@needs lipo "macOS host required (lipo is macOS-only)"
task pack tag="":
    @if eq({{tag}}, "")
        ./scripts/pack-mcpb.sh
    @else
        ./scripts/pack-mcpb.sh --tag {{tag}}
    @end

@group mcpb
@desc "Build + upload sema.mcpb to the release (tag=vX.Y.Z, default: latest)"
@needs gh "brew install gh"
@needs lipo "macOS host required (lipo is macOS-only)"
task publish tag="":
    @if eq({{tag}}, "")
        ./scripts/pack-mcpb.sh --upload
    @else
        ./scripts/pack-mcpb.sh --tag {{tag}} --upload
    @end

@group mcpb
@desc "Remove locally built .mcpb artifacts (dist/)"
task clean:
    rm -rf dist
    echo "mcpb: removed dist/"
