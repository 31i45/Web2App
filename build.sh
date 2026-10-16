#!/usr/bin/env bash
# Web2App v2 构建脚本（build.ps1 的 Linux 镜像；三 crate workspace：tail-core + child + master）
# 用法：
#   ./build.sh                              # 完整构建：child → 测试 → clippy → master → 装配
#   ./build.sh --skip-tests --skip-clippy   # 跳过单元测试与 lint（CI 已单跑）
# 产物：dist/web2app（release 打包器；GUI 打出的 .deb 仅数百 KB——依赖系统 WebKitGTK，
# 不内嵌 webview。直接跑 target/debug/web2app 会得到含调试符号的大产物，勿用于分发）。
set -euo pipefail
cd "$(dirname "$0")"

SKIP_TESTS=0; SKIP_CLIPPY=0; OUT_DIR="dist"
for arg in "$@"; do
  case "$arg" in
    --skip-tests)  SKIP_TESTS=1 ;;
    --skip-clippy) SKIP_CLIPPY=1 ;;
    *) OUT_DIR="$arg" ;;
  esac
done

command -v cargo >/dev/null || { echo "cargo not found（先安装 Rust：https://rustup.rs）"; exit 1; }

echo "== Web2App v2 build =="

# 1) 构建 child（release）— master 编译期内嵌 child 模板，必须先构建
echo "[1/5] cargo build -p child --release"
cargo build -p child --release

# 2) 单元测试（全 workspace：tail-core + child + master）
if [ "$SKIP_TESTS" = 0 ]; then
  echo "[2/5] cargo test --workspace"
  cargo test --workspace
else
  echo "[2/5] skip tests"
fi

# 3) clippy（零告警门禁，与 CI 同口径）— 本地默认跑，避免「本地过、CI 挂」
if [ "$SKIP_CLIPPY" = 0 ]; then
  echo "[3/5] cargo clippy --workspace --all-targets -- -D warnings"
  cargo clippy --workspace --all-targets -- -D warnings
else
  echo "[3/5] skip clippy"
fi

# 4) 构建 master（release）— build.rs 定位 child 并编译期内嵌为 CHILD_TEMPLATE
echo "[4/5] cargo build -p master --release"
cargo build -p master --release

# 5) 装配产物到 dist/
echo "[5/5] assemble $OUT_DIR"
mkdir -p "$OUT_DIR"
cp target/release/web2app "$OUT_DIR/web2app"
SIZE=$(wc -c < "$OUT_DIR/web2app")
echo "web2app: $OUT_DIR/web2app ($(( SIZE / 1024 )) KB)"

echo
echo "usage: run ./web2app -> input URL (+ optional icon) -> click pack"
echo "       product .deb: sudo apt install ./<host>.deb （文件名 = 包名，卸载 sudo apt remove <host>；依赖系统 WebKitGTK，不内嵌 webview）"
