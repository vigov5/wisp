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

cp web/index.html web/app.js web/style.css docs/
mkdir -p docs/pkg
# Only the two runtime artifacts — skip the .d.ts typings wasm-bindgen also emits.
cp web/pkg/wisp_web_receiver.js web/pkg/wisp_web_receiver_bg.wasm docs/pkg/

echo "Synced receiver into docs/. Review 'git status docs/' and commit."
