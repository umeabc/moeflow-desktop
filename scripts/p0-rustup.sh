#!/usr/bin/env bash
set -x
mkdir -p /c/Users/Administrator/tools
cd /c/Users/Administrator/tools
curl -fL -o rustup-init.exe https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe
./rustup-init.exe -y --default-toolchain stable-x86_64-pc-windows-msvc --profile default --no-modify-path
export PATH="$USERPROFILE/.cargo/bin:$PATH"
rustc -vV
cargo -V
