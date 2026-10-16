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
#   3. child 产物中**合法映像**恰为 1 个（证明没有内嵌第二个可执行体）。判据按
#      产物格式取：Windows=PE（MZ 头 + e_lfanew 指向 PE 签名）、Linux=ELF（\x7fELF
#      魔数 + e_shoff 指向文件内完整节头表，T-1-5）。不用裸魔数计数——机器码里会
#      偶然命中（实测 child.exe 裸 'PE\0\0' 计数=3 而真映像仅 1）；「魔数 + 指向
#      结构的指针落界」才与「无内嵌可执行体」同义。
#   4. cargo tree 依赖边界：child 依赖图不含 master 与 windows-sys；
#      master 依赖图含自身与 windows-sys（反向对照，证明 tree 抓取有效）。
#
# 口径：release `strip = true` 已删除符号名，故审计范围是**字符串字面量与导入表 API 名**
#      （UpdateResourceW 属导入名，必然保留）；字符串比较显式用 [StringComparison]::Ordinal。
#
# 平台分支：标记扫描（1/2 项）与 PE 计数属 Windows 专属——UpdateResource/signtool
# 只在 Windows 编译进 master（Linux master 无这些符号面，正向对照无从谈起）；
# Linux 分支跑 ELF 计数（T-1-5），正向对照 = master ELF 含内嵌 child 模板。
# 第 4 项依赖树两平台共用，唯 windows-sys 断言仅 Windows 有意义（Linux 目标本不编译）。
# 平台判定用 .NET OSVersion 而非 pwsh 的 $IsWindows：Windows PowerShell 5.1 未定义
# 该变量（引用静默得 $null），会无声走错分支。
#
# 用法：pwsh .\audit.ps1 [-ChildPath target\release\child.exe] [-MasterPath dist\web2app.exe]
# 退出码：0 全过；1 任一失败。
param(
  [string]$ChildPath = "target\release\child.exe",
  [string]$MasterPath = "dist\web2app.exe"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$isWin = [System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT

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

# 合法 ELF 映像计数（T-1-5，与 PE 判据同构）：\x7fELF 魔数 + 节头表指针
# （e_shoff QWORD@+0x28）指向文件内的完整节头表（e_shnum × e_shentsize 全部落界）。
# 裸魔数会偶然出现在机器码/数据里，指针落界校验才与「真实映像」同义。
# 仅 ELF64 小端（本仓库产物 x86_64/aarch64 均为此；头字段读取范围 0x00-0x3F；
# 头字段为小端，与 BitConverter 原生序一致——同 Get-PeImageCount 的既有前提）。
function Get-ElfImageCount([byte[]]$bytes) {
  $count = 0
  for ($i = 0; $i -le $bytes.Length - 0x40; $i++) {
    if ($bytes[$i]   -ne 0x7F -or $bytes[$i+1] -ne 0x45 -or
        $bytes[$i+2] -ne 0x4C -or $bytes[$i+3] -ne 0x46) { continue }
    $shoff     = [BitConverter]::ToUInt64($bytes, $i + 0x28)
    $shentsize = [BitConverter]::ToUInt16($bytes, $i + 0x3A)
    $shnum     = [BitConverter]::ToUInt16($bytes, $i + 0x3C)
    if ($shoff -lt 0x40 -or $shoff -gt [uint64]$bytes.Length) { continue }
    if ($shentsize -eq 0 -or $shnum -eq 0) { continue }
    if ($shoff + [uint64]$shnum * [uint64]$shentsize -gt [uint64]$bytes.Length) { continue }
    $count++
  }
  return $count
}

Write-Host "== Web2App 结构性不可繁殖审计 ==" -ForegroundColor Cyan
$childBytes = [System.IO.File]::ReadAllBytes($ChildPath)
$masterBytes = [System.IO.File]::ReadAllBytes($MasterPath)
$childText = Read-Latin1 $ChildPath
$masterText = Read-Latin1 $MasterPath

# ---- 1. child 不含 master 专有标记（Windows 专属能力名；Linux 无此符号面）----
if ($isWin) {
  Write-Host "`n[1] child.exe 不含打包 / 写资源 / 签名能力"
  $forbidden = @(
    "UpdateResourceW", "BeginUpdateResourceW", "EndUpdateResourceW",
    "signtool", "WEB2APP_SIGN_CERT"
  )
  foreach ($m in $forbidden) {
    Assert (-not (Test-Marker $childText $m)) "child lacks '$m'"
  }
}

# ---- 2. 正向对照：master 必含目标（证明扫描器有效） ----
if ($isWin) {
  Write-Host "`n[2] positive control (master.exe proves the scan works)"
  foreach ($m in @("UpdateResourceW", "BeginUpdateResourceW", "EndUpdateResourceW", "WEB2APP_SIGN_CERT")) {
    Assert (Test-Marker $masterText $m) "master has '$m' (scan is effective)"
  }
  # master 内嵌 child 模板 = 第 2 个合法 PE 映像 → 证明 PE 扫描器识别得出真实内嵌体
  $masterPe = Get-PeImageCount $masterBytes
  Assert ($masterPe -ge 2) "master has >= 2 valid PE images (embedded child detected; proves scanner) (found $masterPe)"
} else {
  Write-Host "`n[2] positive control (master ELF proves the ELF scan works)"
  $masterElf = Get-ElfImageCount $masterBytes
  Assert ($masterElf -ge 2) "master has >= 2 valid ELF images (embedded child detected; proves scanner) (found $masterElf)"
}

# ---- 3. 无内嵌第二个映像（判据按产物格式取，见头注第 3 项）----
if ($isWin) {
  Write-Host "`n[3] child.exe 无内嵌可执行体"
  $childPe = Get-PeImageCount $childBytes
  Assert ($childPe -eq 1) "child has exactly 1 valid PE image (no embedded executable) (found $childPe)"
} else {
  Write-Host "`n[3] child 无内嵌可执行体 (ELF)"
  $childElf = Get-ElfImageCount $childBytes
  Assert ($childElf -eq 1) "child has exactly 1 valid ELF image (no embedded executable) (found $childElf)"
}

# ---- 4. 依赖图边界 ----
Write-Host "`n[4] cargo tree 依赖边界"
$childTree = ((cargo tree -p child --locked -e normal,build --prefix none 2>$null) -join "`n")
Assert ($childTree.Length -gt 0) "child dependency tree captured"
Assert ($childTree.IndexOf("master", [StringComparison]::Ordinal) -lt 0) "child tree excludes 'master'"
Assert ($childTree.IndexOf("windows-sys", [StringComparison]::Ordinal) -lt 0) "child tree excludes 'windows-sys'"
$masterTree = ((cargo tree -p master --locked --prefix none 2>$null) -join "`n")
Assert ($masterTree.Length -gt 0) "master dependency tree captured"
Assert ($masterTree.IndexOf("master", [StringComparison]::Ordinal) -ge 0) "master tree includes itself"
if ($isWin) {
  # windows-sys 是 winicon/sign（UpdateResource/签名）的依赖，仅 Windows 目标编译——
  # Linux 的 master 依赖树里本就不出现，该断言只在 Windows 有意义。
  Assert ($masterTree.IndexOf("windows-sys", [StringComparison]::Ordinal) -ge 0) "master tree includes 'windows-sys'"
}

Write-Host ""
if ($failed) { Write-Host "AUDIT: FAILED" -ForegroundColor Red; exit 1 }
else { Write-Host "AUDIT: ALL PASS" -ForegroundColor Green; exit 0 }
