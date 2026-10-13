# Web2App v2 构建脚本（两 crate workspace）
# 用法：
#   .\build.ps1              # 完整构建：child → 测试 → master → 装配
#   .\build.ps1 -SkipTests   # 跳过单元测试
param(
    [switch]$SkipTests,
    [string]$OutDir = "dist"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "== Web2App v2 build ==" -ForegroundColor Cyan

# 1) 构建 child（release）— master 编译期内嵌 child.exe，必须先构建
Write-Host "[1/4] cargo build -p child --release" -ForegroundColor Cyan
cargo build -p child --release
if ($LASTEXITCODE -ne 0) { throw "child build failed" }

# 2) 单元测试（全 workspace：tail-core + child + master，共 50 个）
if (-not $SkipTests) {
    Write-Host "[2/4] cargo test --workspace" -ForegroundColor Cyan
    cargo test --workspace
    if ($LASTEXITCODE -ne 0) { throw "tests failed" }
} else {
    Write-Host "[2/4] skip tests" -ForegroundColor DarkGray
}

# 3) 构建 master（release）— build.rs 定位 child.exe 并内嵌为 CHILD_TEMPLATE
Write-Host "[3/4] cargo build -p master --release" -ForegroundColor Cyan
cargo build -p master --release
if ($LASTEXITCODE -ne 0) { throw "master build failed" }

# 4) 装配产物到 dist/
Write-Host "[4/4] assemble dist" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$artifact = Join-Path $OutDir "web2app.exe"
Copy-Item "target\release\web2app.exe" $artifact -Force

$size = (Get-Item $artifact).Length
Write-Host ("web2app: {0} ({1:N0} KB)" -f $artifact, ($size / 1KB))

Write-Host ""
Write-Host "usage: run web2app.exe -> input URL (+ optional icon) -> click pack" -ForegroundColor Green
Write-Host "       product exe opens URL in WebView2 window with user icon" -ForegroundColor Green
