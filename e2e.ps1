# e2e.ps1 — Web2App v2 端到端验证（手动工具，非 CI 门禁）
#
# v2 架构下的全链路检查：
#   1. 静态身份与资源：child/web2app 的 FileDescription=Web2App（任务管理器进程名
#      读此字段），且 PE 内含 RT_GROUP_ICON（Explorer 文件图标）；体积红线
#   2. 真实产物链路：child.exe 就是 master 编译期内嵌的模板（master/build.rs
#      逐字节拷贝），尾部追加语义与 builder::pack 等价
#   3. 产物启动：宿主存活 + msedgewebview2 子进程 + 数据目录落位系统区
#   4. 零残留：点 X 关闭后宿主与本应用的 WebView2 子进程全部退出，exe 目录零文件
#   5. 裸模板结构性不可运行：child.exe（无尾部）弹致命错误、不起 WebView、退出码 1
#   6. 母版表单：web2app.exe 启动 + 关闭零残留
#   7. 字节级：产物裸主体 == child.exe（模板复制，非自身繁殖）；覆盖尾部不膨胀
#
# 用法：.\e2e.ps1 [-MasterPath target\release\web2app.exe] [-ChildPath target\release\child.exe]
# 退出码：0 全过；1 任一检查失败。
param(
  [string]$MasterPath = "target\release\web2app.exe",
  [string]$ChildPath = "target\release\child.exe"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if (-not (Test-Path $MasterPath)) { Write-Host "FAIL: master not found: $MasterPath"; exit 1 }
if (-not (Test-Path $ChildPath)) { Write-Host "FAIL: child not found: $ChildPath (run build.ps1 first)"; exit 1 }
$MasterPath = (Resolve-Path $MasterPath).Path
$ChildPath = (Resolve-Path $ChildPath).Path

# RT_GROUP_ICON(14) 资源探测：LoadLibraryEx(AS_DATAFILE) + EnumResourceNames。
# ExtractAssociatedIcon 对无资源 exe 也会回退通用图标，无法区分——必须枚举资源。
if (-not ("ResCheck" -as [type])) {
  Add-Type -Language CSharp -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ResCheck {
  private delegate bool EnumResNameProc(IntPtr h, IntPtr type, IntPtr name, IntPtr p);
  [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
  private static extern IntPtr LoadLibraryEx(string f, IntPtr h, uint flags);
  [DllImport("kernel32.dll", SetLastError=true)]
  private static extern bool FreeLibrary(IntPtr h);
  [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
  private static extern bool EnumResourceNames(IntPtr h, IntPtr type, EnumResNameProc cb, IntPtr p);
  public static bool HasResource(string path, int typeId) {
    IntPtr h = LoadLibraryEx(path, IntPtr.Zero, 0x2);
    if (h == IntPtr.Zero) throw new Exception("LoadLibraryEx failed: " + Marshal.GetLastWin32Error());
    bool found = false;
    EnumResourceNames(h, (IntPtr)typeId, (m, t, n, p) => { found = true; return false; }, IntPtr.Zero);
    FreeLibrary(h);
    return found;
  }
}
'@
}

# 临时沙盒（自动清理）
$dir = Join-Path ([System.IO.Path]::GetTempPath()) ("web2app_e2e_" + [Guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $dir | Out-Null
$childBytes = [System.IO.File]::ReadAllBytes($ChildPath)
$MAGIC = [System.Text.Encoding]::ASCII.GetBytes("__WEB2APP_TAIL__")
$failed = $false

function Assert($cond, $name) {
  if ($cond) { Write-Host "  PASS  $name" }
  else { Write-Host "  FAIL  $name"; $script:failed = $true }
}

# 构造产物（与 builder::pack 相同的尾部追加语义）
function New-Product([string]$name, [string]$json, [byte[]]$base) {
  $payload = [System.Text.Encoding]::UTF8.GetBytes($json)
  $buf = New-Object byte[] ($base.Length + $payload.Length + 24)
  [Array]::Copy($base, 0, $buf, 0, $base.Length)
  [Array]::Copy($payload, 0, $buf, $base.Length, $payload.Length)
  [Array]::Copy($MAGIC, 0, $buf, ($base.Length + $payload.Length), 16)
  [BitConverter]::GetBytes([UInt64]$payload.Length).CopyTo($buf, ($base.Length + $payload.Length + 16))
  $p = Join-Path $dir $name
  [System.IO.File]::WriteAllBytes($p, $buf)
  return @($p, $payload.Length)
}

# 点 X 关闭（CloseMainWindow 对窗口与 MessageBox 都发 WM_CLOSE），等待退出；
# 超时强杀并返回 $false。
function Close-App($p, [int]$timeoutSec = 6) {
  if ($p.HasExited) { return $true }
  $p.CloseMainWindow() | Out-Null
  for ($i = 0; $i -lt $timeoutSec * 2; $i++) {
    Start-Sleep -Milliseconds 500
    if ($p.HasExited) { return $true }
  }
  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  return $false
}

# 本应用残留计数：宿主进程（按可执行路径精确匹配）+ 命令行含本应用数据目录的
# WebView2 进程（机器上可能有其他 msedgewebview2，必须按命令行隔离）。
function Count-Residue([string]$exePath, [string]$dataMarker) {
  $hosts = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ExecutablePath -and $_.ExecutablePath -ieq $exePath })
  $wv = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -and $_.CommandLine -like "*$dataMarker*" })
  return $hosts.Count + $wv.Count
}

function Wait-WebView([string]$dataMarker, [int]$sec = 6) {
  for ($i = 0; $i -lt $sec * 2; $i++) {
    Start-Sleep -Milliseconds 500
    $wv = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" -ErrorAction SilentlyContinue |
      Where-Object { $_.CommandLine -and $_.CommandLine -like "*$dataMarker*" })
    if ($wv.Count -gt 0) { return $true }
  }
  return $false
}

try {
  Write-Host "== Web2App v2 E2E ==" -ForegroundColor Cyan
  Write-Host "master: $MasterPath ($((Get-Item $MasterPath).Length) B)"
  Write-Host "child:  $ChildPath ($($childBytes.Length) B)"

  # ---- 1. 静态身份、资源、体积红线 ----
  Write-Host "`n[1] identity / resources / size"
  $mv = (Get-Item $MasterPath).VersionInfo
  $cv = (Get-Item $ChildPath).VersionInfo
  Assert ($mv.FileDescription -eq "Web2App") "master FileDescription=Web2App (got '$($mv.FileDescription)')"
  Assert ($mv.ProductName -eq "Web2App") "master ProductName=Web2App (got '$($mv.ProductName)')"
  Assert ($cv.FileDescription -eq "Web2App") "child FileDescription=Web2App — Task Manager 不再显示 child (got '$($cv.FileDescription)')"
  Assert ($cv.ProductName -eq "Web2App") "child ProductName=Web2App (got '$($cv.ProductName)')"
  Assert ([ResCheck]::HasResource($MasterPath, 14)) "master PE has RT_GROUP_ICON (Explorer file icon)"
  Assert ([ResCheck]::HasResource($ChildPath, 14)) "child PE has RT_GROUP_ICON (products inherit w2a default)"
  Assert ($childBytes.Length -lt 1MB) "child < 1MB promise ($([math]::Round($childBytes.Length/1KB)) KB)"
  Assert ((Get-Item $MasterPath).Length -lt 2MB) "web2app < 2MB target ($([math]::Round((Get-Item $MasterPath).Length/1KB)) KB)"

  # ---- 2/3. 产物启动（child.exe 即 master 内嵌模板） ----
  Write-Host "`n[2] product launch"
  $ret = New-Product "example.com.exe" '{"url":"https://example.com","title":"example.com"}' $childBytes
  $p1 = $ret[0]
  $before = (Get-ChildItem $dir).Count
  $proc = Start-Process -FilePath $p1 -PassThru
  $wvOk = Wait-WebView "Web2App\apps\example.com"
  Start-Sleep -Seconds 1
  Assert (-not $proc.HasExited) "product host alive"
  Assert $wvOk "msedgewebview2 rendering with scoped data dir"
  Assert (Test-Path "$env:LOCALAPPDATA\Web2App\apps\example.com") "data dir in LOCALAPPDATA (exe dir untouched)"

  # ---- 4. 零残留 ----
  Write-Host "`n[3] zero residue after close"
  $closed = Close-App $proc
  Assert $closed "product closed via X (no force kill)"
  Start-Sleep -Seconds 2
  Assert ((Count-Residue $p1 "Web2App\apps\example.com") -eq 0) "no host/webview process survives"
  $after = (Get-ChildItem $dir).Count
  Assert ($after -eq $before) "zero files dropped in exe dir ($before -> $after)"

  # ---- 5. 裸模板结构性不可运行 ----
  Write-Host "`n[4] bare child template is not runnable"
  $bare = Join-Path $dir "template.exe"
  Copy-Item $ChildPath $bare
  $pb = Start-Process -FilePath $bare -PassThru
  Start-Sleep -Seconds 3
  $bareWv = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -and $_.CommandLine -like "*Web2App\apps*" })
  # 注意：example.com 的 WebView 上一阶段已全部退出，此刻任何 apps 下的 webview 都属异常
  Assert ($bareWv.Count -eq 0) "bare template spawns no WebView"
  $bareClosed = Close-App $pb 5
  Assert $bareClosed "bare template fatal dialog dismissible (WM_CLOSE)"
  Assert ($pb.HasExited -and $pb.ExitCode -eq 1) "bare template exits code 1 (got $(if($pb.HasExited){$pb.ExitCode}else{'alive'}))"

  # ---- 6. 母版表单 ----
  Write-Host "`n[5] master form launch + cleanup"
  $pm = Start-Process -FilePath $MasterPath -PassThru
  $mvw = Wait-WebView "Web2App\master"
  Assert (-not $pm.HasExited) "master alive"
  Assert $mvw "master form webview alive"
  $mClosed = Close-App $pm
  Assert $mClosed "master closed via X"
  Start-Sleep -Seconds 2
  Assert ((Count-Residue $MasterPath "Web2App\master") -eq 0) "no master/webview residue"

  # ---- 7. 字节级：模板复制 + 覆盖尾部零膨胀 ----
  Write-Host "`n[6] byte-level template identity & overwrite"
  $gen1 = [System.IO.File]::ReadAllBytes($p1)
  $len = [BitConverter]::ToUInt64($gen1, $gen1.Length - 8)
  $base = $gen1[0..($gen1.Length - 25 - [int]$len)]
  Assert ($base.Length -eq $childBytes.Length) "product body length == child.exe (template copy)"
  $prefixSame = [System.Linq.Enumerable]::SequenceEqual([byte[]]$base, [byte[]]$childBytes)
  Assert $prefixSame "product body byte-identical to child.exe"
  Assert ($base.Length -ne (Get-Item $MasterPath).Length) "product derives from child, not master (structural non-reproduction)"

  $ret2 = New-Product "second.io.exe" '{"url":"https://second.io","title":"second.io"}' $base
  $p2 = $ret2[0]
  $gen2 = [System.IO.File]::ReadAllBytes($p2)
  $gen2Same = [System.Linq.Enumerable]::SequenceEqual([byte[]]($gen2[0..($base.Length - 1)]), [byte[]]$base)
  Assert $gen2Same "re-packed body stays byte-identical"
  $expectLen = $base.Length + $ret2[1] + 24
  Assert ($gen2.Length -eq $expectLen) "no size accumulation ($($gen2.Length) B == base + payload + 24)"
  $proc2 = Start-Process -FilePath $p2 -PassThru
  Assert (Wait-WebView "Web2App\apps\second.io") "re-packed product runs"
  Assert (-not $proc2.HasExited) "gen2 host alive"
  Assert (Close-App $proc2) "gen2 closed"
  Start-Sleep -Seconds 2
  Assert ((Count-Residue $p2 "Web2App\apps\second.io") -eq 0) "gen2 zero residue"
}
finally {
  # 兜底：沙盒中任何仍存活的进程不阻塞清理（Close-App 正常路径已处理）
  Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($dir, [StringComparison]::OrdinalIgnoreCase) } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  if (Test-Path $dir) { Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue }
}

Write-Host ""
if ($failed) { Write-Host "E2E: FAILED" -ForegroundColor Red; exit 1 }
else { Write-Host "E2E: ALL PASS" -ForegroundColor Green; exit 0 }
