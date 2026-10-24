#!/bin/sh
# The converter in the browser: web/pkg/convert_web.js and convert_web_bg.wasm, which web/convert.html runs, and
# convert_web_jspi.js / _bg.wasm, the build that also writes into a folder the user picked (JSPI; the worker loads
# it where the browser has JSPI). Serve with `target/release/serve 8770 web`. Needs the wasm32 target and
# wasm-bindgen-cli 0.2.129 (installed into target/tools on first use).
set -e
cd "$(dirname "$0")/.."
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
[ -x target/tools/bin/wasm-bindgen ] || cargo install wasm-bindgen-cli --version 0.2.129 --root target/tools
wasm=target/wasm32-unknown-unknown/web/convert_web.wasm
cargo build --profile web --target wasm32-unknown-unknown -p convert_web
target/tools/bin/wasm-bindgen --target web --no-typescript --out-dir web/pkg --out-name convert_web_jspi "$wasm"
cargo build --profile web --target wasm32-unknown-unknown -p convert_web --no-default-features
target/tools/bin/wasm-bindgen --target web --no-typescript --out-dir web/pkg "$wasm"
