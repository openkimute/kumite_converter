#!/bin/sh
# The release files, one standalone kumite_convert per platform, into dist/ (profile dist; .cargo/config.toml), to
# test locally: the releases come from .github/workflows/release.yml, which builds Windows natively (cross-built
# through cargo-xwin, it converts 2.7x slower: Microsoft's libraries cargo-xwin downloads):
#   linux-amd64, linux-arm64      static (musl, with mimalloc: musl's allocator serializes the threads): no libc or
#                                 other library needed
#   windows-amd64, windows-arm64  .exe with the C runtime linked in: only Windows' own DLLs
#   macos                         universal (arm64 + x86_64), macOS 11+: only libSystem and CoreFoundation (macOS
#                                 links them dynamically, always)
#   wasi                          kumite_convert.wasm, for a WASI runtime (wasmtime, Node's WASI)
#   web/                          the converter in the browser: convert.html, its worker and pkg/
# Runs on macOS (lipo and the Apple targets need it). Needs zig and cargo-zigbuild (brew install zig; cargo install
# cargo-zigbuild --locked) for Linux, and cargo-xwin (cargo install cargo-xwin --locked) for Windows, which downloads
# Microsoft's CRT and SDK on first use (accepting their license).
set -e
cd "$(dirname "$0")"
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-msvc \
    aarch64-pc-windows-msvc x86_64-apple-darwin aarch64-apple-darwin wasm32-wasip1 >/dev/null
build() { cargo build --profile dist -p convert --target "$@"; }
rm -rf dist && mkdir -p dist/web/pkg
for arch in x86_64:amd64 aarch64:arm64; do
    cargo zigbuild --profile dist -p convert --target ${arch%:*}-unknown-linux-musl
    cp target/${arch%:*}-unknown-linux-musl/dist/kumite_convert dist/kumite_convert-linux-${arch#*:}
done
for arch in x86_64 aarch64; do
    t=$arch-pc-windows-msvc
    env "CARGO_TARGET_$(echo $t | tr a-z- A-Z_)_LINKER=rust-lld" cargo xwin build --profile dist -p convert --target $t
done
cp target/x86_64-pc-windows-msvc/dist/kumite_convert.exe dist/kumite_convert-windows-amd64.exe
cp target/aarch64-pc-windows-msvc/dist/kumite_convert.exe dist/kumite_convert-windows-arm64.exe
build aarch64-apple-darwin && build x86_64-apple-darwin
lipo -create -output dist/kumite_convert-macos target/aarch64-apple-darwin/dist/kumite_convert \
    target/x86_64-apple-darwin/dist/kumite_convert
build wasm32-wasip1 && cp target/wasm32-wasip1/dist/kumite_convert.wasm dist/kumite_convert-wasi.wasm
sh convert_web/build.sh
cp web/convert.html web/convert_worker.js web/background.webp dist/web/ && cp web/pkg/convert_web* dist/web/pkg/
ls -l dist dist/web/pkg
