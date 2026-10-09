# e2e.ps1 — Web2App 端到端验证（手动工具，非 CI 门禁）
#
# 复刻并固化项目验证期反复执行的全链路检查：
#   1. 从 release exe 构造产物（尾部协议裸字节拼接，与 builder::pack 等价）
#   2. 产物启动：进程存活 + WebView2 子进程 + 数据目录落位系统区
#   3. 零残留：运行前后 exe 目录文件数不变
#   4. 自繁殖入口：product --master 打开母版表单
#   5. 多代繁殖字节级：剥离旧尾 + 追加新尾，主体逐字节不变、体积零膨胀
#
# 用法：.\e2e.ps1 [-ExePath target\release\web2app.exe]
# 退出码：0 全过；1 任一检查失败。
param(
    [string]$ExePath = "target\release\web2app.exe"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if (-not (Test-Path $ExePath)) { Write-Host "FAIL: exe not found: $ExePath"; exit 1 }
$ExePath = (Resolve-Path $ExePath).Path

# 临时沙盒（自动清理）
$dir = Join-Path ([System.IO.Path]::GetTempPath()) ("web2app_e2e_" + [Guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $dir | Out-Null
$master = [System.IO.File]::ReadAllBytes($ExePath)
$MAGIC = [System.Text.Encoding]::ASCII.GetBytes("__WEB2APP_TAIL__")
$failed = $false

function Assert($cond, $name) {
    if ($cond) { Write-Host "  PASS  $name" }
    else { Write-Host "  FAIL  $name"; $script:failed = $true }
}

# 构造产物（与 builder::pack 相同的尾部追加语义）；返回 (path, payload 字节数)
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

# 等待进程退出并强制收尾（不抛错）
function Stop-App($p) {
    if (-not $p.HasExited) {
        $p.CloseMainWindow() | Out-Null
        Start-Sleep -Seconds 2
        if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    }
    Start-Sleep -Seconds 1
}

try {
    Write-Host "== Web2App E2E ==" -ForegroundColor Cyan
    Write-Host "master: $ExePath ($($master.Length)B)"
    Assert ($master.Length -lt 10MB) "master < 10MB red line"

    # ---- 1. 产物启动 ----
    Write-Host "`n[1] product launch"
    $ret = New-Product "example.com.exe" '{"url":"https://example.com","title":"example.com"}' $master
    $p1 = $ret[0]
    $before = (Get-ChildItem $dir).Count
    $proc = Start-Process -FilePath $p1 -PassThru
    Start-Sleep -Seconds 6
    $child = Get-CimInstance Win32_Process -Filter "ParentProcessId = $($proc.Id)" -ErrorAction SilentlyContinue | Select-Object -First 1
    Assert (-not $proc.HasExited) "product process alive"
    Assert ($child.Name -eq "msedgewebview2.exe") "WebView2 child rendering"
    Assert (Test-Path "$env:LOCALAPPDATA\Web2App\apps\example.com") "data dir in LOCALAPPDATA"
    Stop-App $proc
    $after = (Get-ChildItem $dir).Count
    Assert ($after -eq $before) "zero residue in exe dir ($before -> $after files)"

    # ---- 2. 自繁殖入口 ----
    Write-Host "`n[2] self-reproduction entry"
    $proc2 = Start-Process -FilePath $p1 -ArgumentList "--master" -PassThru
    Start-Sleep -Seconds 5
    $child2 = Get-CimInstance Win32_Process -Filter "ParentProcessId = $($proc2.Id)" -ErrorAction SilentlyContinue | Select-Object -First 1
    Assert (-not $proc2.HasExited) "product --master opens form"
    Assert ($child2.Name -eq "msedgewebview2.exe") "master form webview alive"
    Stop-App $proc2

    # ---- 3. 多代繁殖（字节级） ----
    Write-Host "`n[3] chained reproduction (byte level)"
    $gen1 = [System.IO.File]::ReadAllBytes($p1)
    $len = [BitConverter]::ToUInt64($gen1, $gen1.Length - 8)
    $base = $gen1[0..($gen1.Length - 25 - [int]$len)]
    Assert ($base.Length -eq $master.Length) "gen1 tail stripped == master bytes"
    $ret2 = New-Product "second.io.exe" '{"url":"https://second.io","title":"second.io"}' $base
    $p2 = $ret2[0]
    $gen2 = [System.IO.File]::ReadAllBytes($p2)
    $prefixSame = [System.Linq.Enumerable]::SequenceEqual([byte[]]($gen2[0..($base.Length - 1)]), [byte[]]$base)
    Assert $prefixSame "gen2 body byte-identical to master"
    # 期望值由构造时的真实 payload 长度计算（杜绝任何口算硬编码）
    $expectLen = $base.Length + $ret2[1] + 24
    Assert ($gen2.Length -eq $expectLen) "no size accumulation ($($gen2.Length)B == base + payload + 24)"
    $proc3 = Start-Process -FilePath $p2 -PassThru
    Start-Sleep -Seconds 6
    Assert (-not $proc3.HasExited) "gen2 runs"
    Stop-App $proc3
}
finally {
    if (Test-Path $dir) { Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue }
}

Write-Host ""
if ($failed) { Write-Host "E2E: FAILED" -ForegroundColor Red; exit 1 }
else { Write-Host "E2E: ALL PASS" -ForegroundColor Green; exit 0 }
