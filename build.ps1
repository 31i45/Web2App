# Web2App v2 构建脚本（三 crate workspace：tail-core + child + master）
# 用法：
#   .\build.ps1                          # 完整构建：child → 测试 → clippy → master → 装配
#   .\build.ps1 -SkipTests -SkipClippy   # 跳过单元测试与 lint（CI 已单跑）
param(
    [switch]$SkipTests,
    [switch]$SkipClippy,
    [string]$OutDir = "dist"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "== Web2App v2 build ==" -ForegroundColor Cyan

# 1) 构建 child（release）— master 编译期内嵌 child.exe，必须先构建
Write-Host "[1/5] cargo build -p child --release" -ForegroundColor Cyan
cargo build -p child --release
if ($LASTEXITCODE -ne 0) { throw "child build failed" }

# 2) 单元测试（全 workspace：tail-core + child + master）
if (-not $SkipTests) {
    Write-Host "[2/5] cargo test --workspace" -ForegroundColor Cyan
    cargo test --workspace
    if ($LASTEXITCODE -ne 0) { throw "tests failed" }
} else {
    Write-Host "[2/5] skip tests" -ForegroundColor DarkGray
}

# 3) clippy（零告警门禁，与 CI 同口径）— 本地默认跑，避免「本地过、CI 挂」
if (-not $SkipClippy) {
    Write-Host "[3/5] cargo clippy --workspace --all-targets -- -D warnings" -ForegroundColor Cyan
    cargo clippy --workspace --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw "clippy failed" }
} else {
    Write-Host "[3/5] skip clippy" -ForegroundColor DarkGray
}

# 4) 构建 master（release）— build.rs 定位 child.exe 并内嵌为 CHILD_TEMPLATE
Write-Host "[4/5] cargo build -p master --release" -ForegroundColor Cyan
cargo build -p master --release
if ($LASTEXITCODE -ne 0) { throw "master build failed" }

# 5) 装配产物到 dist/
Write-Host "[5/5] assemble dist" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$artifact = Join-Path $OutDir "web2app.exe"
Copy-Item "target\release\web2app.exe" $artifact -Force

$size = (Get-Item $artifact).Length
Write-Host ("web2app: {0} ({1:N0} KB)" -f $artifact, ($size / 1KB))

Write-Host ""
Write-Host "usage: run web2app.exe -> input URL (+ optional icon) -> click pack" -ForegroundColor Green
Write-Host "       product exe opens URL in WebView2 window with user icon" -ForegroundColor Green
