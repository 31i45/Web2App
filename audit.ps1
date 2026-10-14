# audit.ps1 — 结构性不可繁殖静态审计（纯静态，不启动 GUI；与 e2e.ps1 分工）
#
# 目标：把「child 产物不含打包能力」从「靠 release 优化器/运气剔除」升级为
#       「编译期结构性 + CI 机械可守」。
#
# 检查项：
#   1. child.exe 不含 master 专有标记（写资源 / 签名相关 API 名与字符串）；
#      覆盖 UTF-8（Rust 字面量/导入名）与 UTF-16LE（宽字符串）两种落盘编码。
#   2. 正向对照：master.exe 必含这些标记——证明「扫描器 + 字节提取」有效，
#      否则「child 不含」可能是扫描空转的假阳性。
#      注：**不用**「Web2App 打包器」做对照——child 的致命错误文案里合法引用了该名称。
#   3. child.exe 中**合法 PE 映像**恰为 1 个（证明没有内嵌第二个可执行体）。
#      不用裸 'PE\0\0' 计数——机器码里 0x50 0x45 00 00（push rax; add [rax],al）
#      会偶然命中，实测 child.exe 裸计数=3 而真映像仅 1；合法映像判据（MZ 头 +
#      e_lfanew 指向 PE 签名）才与「无内嵌可执行体」同义。
#   4. cargo tree 依赖边界：child 依赖图不含 master 与 windows-sys；
#      master 依赖图含自身与 windows-sys（反向对照，证明 tree 抓取有效）。
#
# 口径：release `strip = true` 已删除符号名，故审计范围是**字符串字面量与导入表 API 名**
#      （UpdateResourceW 属导入名，必然保留）；字符串比较显式用 [StringComparison]::Ordinal。
#
# 用法：pwsh .\audit.ps1 [-ChildPath target\release\child.exe] [-MasterPath dist\web2app.exe]
# 退出码：0 全过；1 任一失败。
param(
  [string]$ChildPath = "target\release\child.exe",
  [string]$MasterPath = "dist\web2app.exe"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if (-not (Test-Path $ChildPath)) { Write-Host "FAIL: child not found: $ChildPath"; exit 1 }
if (-not (Test-Path $MasterPath)) { Write-Host "FAIL: master not found: $MasterPath"; exit 1 }

$failed = $false
function Assert($cond, $name) {
  if ($cond) { Write-Host "  PASS  $name" }
  else { Write-Host "  FAIL  $name"; $script:failed = $true }
}

# Latin1（ISO-8859-1）：字节 ↔ 字符一一映射，使 .NET 字符串的 Ordinal 搜索
# 等价于原始字节流子串搜索（无任何编码规范化/损失）。
$latin1 = [System.Text.Encoding]::GetEncoding(28591)
function Read-Latin1([string]$path) {
  return $latin1.GetString([System.IO.File]::ReadAllBytes($path))
}
function Test-Marker([string]$text, [string]$marker) {
  $u8 = $latin1.GetString([System.Text.Encoding]::UTF8.GetBytes($marker))
  $u16 = $latin1.GetString([System.Text.Encoding]::Unicode.GetBytes($marker))
  return ($text.IndexOf($u8, [StringComparison]::Ordinal) -ge 0) -or
         ($text.IndexOf($u16, [StringComparison]::Ordinal) -ge 0)
}

# 合法 PE 映像计数：MZ 头（0x4D 0x5A）+ e_lfanew(DWORD@+0x3C) 指向 'PE\0\0'。
# 实测：child.exe = 1（仅自身）；master.exe = 2（自身 + 内嵌 child 模板），
# 后者即扫描器的阳性对照——证明本函数能识别真实内嵌可执行体。
function Get-PeImageCount([byte[]]$bytes) {
  $count = 0
  for ($i = 0; $i -le $bytes.Length - 0x40; $i++) {
    if ($bytes[$i] -ne 0x4D -or $bytes[$i+1] -ne 0x5A) { continue }
    $e = [BitConverter]::ToUInt32($bytes, $i + 0x3C)
    if ($e -lt 0x40 -or ($i + $e + 4) -gt $bytes.Length) { continue }
    if ($bytes[$i+$e]   -eq 0x50 -and $bytes[$i+$e+1] -eq 0x45 -and
        $bytes[$i+$e+2] -eq 0    -and $bytes[$i+$e+3] -eq 0) { $count++ }
  }
  return $count
}

Write-Host "== Web2App 结构性不可繁殖审计 ==" -ForegroundColor Cyan
$childBytes = [System.IO.File]::ReadAllBytes($ChildPath)
$masterBytes = [System.IO.File]::ReadAllBytes($MasterPath)
$childText = Read-Latin1 $ChildPath
$masterText = Read-Latin1 $MasterPath

# ---- 1. child 不含 master 专有标记 ----
Write-Host "`n[1] child.exe 不含打包 / 写资源 / 签名能力"
$forbidden = @(
  "UpdateResourceW", "BeginUpdateResourceW", "EndUpdateResourceW",
  "signtool", "WEB2APP_SIGN_CERT"
)
foreach ($m in $forbidden) {
  Assert (-not (Test-Marker $childText $m)) "child lacks '$m'"
}

# ---- 2. 正向对照：master 必含（证明扫描有效） ----
Write-Host "`n[2] positive control (master.exe proves the scan works)"
foreach ($m in @("UpdateResourceW", "BeginUpdateResourceW", "EndUpdateResourceW", "WEB2APP_SIGN_CERT")) {
  Assert (Test-Marker $masterText $m) "master has '$m' (scan is effective)"
}
# master 内嵌 child 模板 = 第 2 个合法 PE 映像 → 证明 PE 扫描器识别得出真实内嵌体
$masterPe = Get-PeImageCount $masterBytes
Assert ($masterPe -ge 2) "master has >= 2 valid PE images (embedded child detected; proves scanner) (found $masterPe)"

# ---- 3. 无内嵌第二个 PE ----
Write-Host "`n[3] child.exe 无内嵌可执行体"
$childPe = Get-PeImageCount $childBytes
Assert ($childPe -eq 1) "child has exactly 1 valid PE image (no embedded executable) (found $childPe)"

# ---- 4. 依赖图边界 ----
Write-Host "`n[4] cargo tree 依赖边界"
$childTree = ((cargo tree -p child --locked -e normal,build --prefix none 2>$null) -join "`n")
Assert ($childTree.Length -gt 0) "child dependency tree captured"
Assert ($childTree.IndexOf("master", [StringComparison]::Ordinal) -lt 0) "child tree excludes 'master'"
Assert ($childTree.IndexOf("windows-sys", [StringComparison]::Ordinal) -lt 0) "child tree excludes 'windows-sys'"
$masterTree = ((cargo tree -p master --locked --prefix none 2>$null) -join "`n")
Assert ($masterTree.Length -gt 0) "master dependency tree captured"
Assert ($masterTree.IndexOf("master", [StringComparison]::Ordinal) -ge 0) "master tree includes itself"
Assert ($masterTree.IndexOf("windows-sys", [StringComparison]::Ordinal) -ge 0) "master tree includes 'windows-sys'"

Write-Host ""
if ($failed) { Write-Host "AUDIT: FAILED" -ForegroundColor Red; exit 1 }
else { Write-Host "AUDIT: ALL PASS" -ForegroundColor Green; exit 0 }
