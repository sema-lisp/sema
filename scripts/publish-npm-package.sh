#!/usr/bin/env bash

# Publish from the current package directory. RC builds must never replace
# the stable npm dist-tag. An unavailable registry or failed publish is fatal.
set -euo pipefail

package="$(node -p 'const p = require("./package.json"); p.name + "@" + p.version')"
version="$(node -p 'require("./package.json").version')"
tag=latest
if [[ "$version" == *-* ]]; then
  tag=next
fi

if npm view "$package" version >/dev/null 2>&1; then
  echo "$package is already published; leaving dist-tags unchanged"
else
  npm publish --provenance --access public --tag "$tag"
fi
