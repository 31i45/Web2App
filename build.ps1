# Web2App 一键构建脚本
# 用法：
#   .\build.ps1              # 完整构建：测试 + Release 编译 + 母版装配
#   .\build.ps1 -SkipTests   # 跳过单元测试
param(
    [switch]$SkipTests,
    [string]$OutDir = "dist"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "== Web2App build ==" -ForegroundColor Cyan

# 1) 单元测试
if (-not $SkipTests) {
    Write-Host "[1/3] cargo test" -ForegroundColor Cyan
    cargo test --bin web2app
    if ($LASTEXITCODE -ne 0) { throw "tests failed" }
} else {
    Write-Host "[1/3] skip tests" -ForegroundColor DarkGray
}

# 2) Release 编译（GUI 子系统，双击无控制台）
Write-Host "[2/3] cargo build --release" -ForegroundColor Cyan
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "build failed" }

# 3) 母版装配
Write-Host "[3/3] assemble master" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$master = Join-Path $OutDir "Web2App.exe"
Copy-Item "target\release\web2app.exe" $master -Force

$size = (Get-Item $master).Length
Write-Host ("master: {0} ({1:N0} KB)" -f $master, ($size / 1KB))
if ($size -gt 10MB) { Write-Host "WARN: exceeds 10MB limit!" -ForegroundColor Red }
Write-Host ""
Write-Host "usage: run Web2App.exe -> input URL -> click pack" -ForegroundColor Green
Write-Host "       product --master -> reopen packer (self-reproduction)" -ForegroundColor Green
