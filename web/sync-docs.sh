#!/usr/bin/env bash
# Rebuild the release wasm receiver and sync it into docs/, the GitHub Pages
# publishing source (Jekyll site on main, served at web.wisp.mooo.com). The
# receiver is committed there — GitHub builds Pages from main:/docs, so the wasm
# can't be built on the fly.
#
# Run after any change to crates/web-receiver, crates/wire, or
# web/{index.html,app.js,style.css}, then `git add docs/ && commit`. The receiver
# lands at web.wisp.mooo.com/ (relative asset paths, no Jekyll front matter, so
# it's copied verbatim and unaffected by the site's baseurl); the privacy policy
# stays at /privacy-policy/.
set -euo pipefail
cd "$(dirname "$0")/.."   # repo root

web/build.sh release

# Single source of truth for the displayed version is flutter/pubspec.yaml
# (`version: 1.12.0+15` → "1.12.0"). Stamp it into web/index.html so the brand
# header never drifts from the app version; the copy into docs/ inherits it.
VERSION="$(sed -n 's/^version: *\([0-9][0-9.]*\).*/\1/p' flutter/pubspec.yaml)"
if [[ -z "$VERSION" ]]; then
  echo "error: could not read version from flutter/pubspec.yaml" >&2
  exit 1
fi
sed -i "s|<span id=\"app-version\">[^<]*</span>|<span id=\"app-version\">v${VERSION}</span>|" web/index.html
echo "Stamped version v${VERSION} into web/index.html"

# Keep the service worker's cache version in step with the app version so a
# deploy invalidates the old app-shell cache (see web/sw.js activate handler).
sed -i "s|^const CACHE_VERSION = '[^']*';|const CACHE_VERSION = 'v${VERSION}';|" web/sw.js
echo "Stamped CACHE_VERSION v${VERSION} into web/sw.js"

cp web/index.html web/app.js web/style.css docs/
# PWA assets: manifest, service worker, and installable icons.
cp web/manifest.webmanifest web/sw.js docs/
mkdir -p docs/icons
cp web/icons/icon-192.png web/icons/icon-512.png web/icons/icon-maskable-512.png docs/icons/
mkdir -p docs/vendor
cp web/vendor/alpine.esm.js docs/vendor/
mkdir -p docs/pkg
# Only the two runtime artifacts — skip the .d.ts typings wasm-bindgen also emits.
cp web/pkg/wisp_web_receiver.js web/pkg/wisp_web_receiver_bg.wasm docs/pkg/

echo "Synced receiver into docs/. Review 'git status docs/' and commit."
