<#
.SYNOPSIS
    启幕 (qimu-client) 本地增量编译 + 自动重启预览脚本（兼容 PowerShell 5.1 / 7+）。

.DESCRIPTION
    1. 将 CARGO_TARGET_DIR 定向到纯英文路径（默认 D:\qimu-target），绕开 w64devkit ld.exe 的中文路径缺陷。
    2. 监听 crates/ 与根 Cargo.* 文件变更（400ms 防抖）。
    3. 若当前已有预览窗口运行，先执行快速 cargo check；通过后再关闭旧进程、增量链接并重启 qimu-client.exe。
       若检查失败，会保留当前预览窗口，并在终端打印编译错误。

.EXAMPLE
    pwsh -ExecutionPolicy Bypass -File tools/dev_preview.ps1
    pwsh -ExecutionPolicy Bypass -File tools/dev_preview.ps1 -Page content
    pwsh -ExecutionPolicy Bypass -File tools/dev_preview.ps1 -Page content -Open "unit:<UNIT_ID>"
    pwsh -ExecutionPolicy Bypass -File tools/dev_preview.ps1 -CheckOnly
#>
[CmdletBinding()]
param(
    [string]$TargetDir = "D:\qimu-target",
    [ValidateSet("dev", "staging")]
    [string]$Profile = "dev",
    [string]$Page = "",
    [string]$Open = "",
    [string]$Zone = "",
    [int]$DebounceMs = 400,
    [switch]$CheckOnly,
    [switch]$NoInitialLaunch
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$TargetDirFull = [System.IO.Path]::GetFullPath($TargetDir)
if ($TargetDirFull -match '[^\x00-\x7F]') {
    throw "TargetDir 必须为纯 ASCII/英文路径以兼容 w64devkit ld.exe，当前值: $TargetDirFull"
}
if (-not (Test-Path $TargetDirFull)) {
    New-Item -ItemType Directory -Path $TargetDirFull -Force | Out-Null
}

$ProfileSubDir = if ($Profile -eq "staging") { "staging" } else { "debug" }
$ExePath = Join-Path $TargetDirFull "$ProfileSubDir\qimu-client.exe"

$env:CARGO_TARGET_DIR = $TargetDirFull
if ($Page) { $env:QIMU_PAGE = $Page }
if ($Open) { $env:QIMU_OPEN = $Open }
if ($Zone) { $env:QIMU_ZONE = $Zone }

function Get-RunningPreviewProcesses {
    param([string]$ExpectedExe)
    $norm = [System.IO.Path]::GetFullPath($ExpectedExe)
    Get-CimInstance Win32_Process -Filter "Name = 'qimu-client.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.ExecutablePath -and ([System.IO.Path]::GetFullPath($_.ExecutablePath) -ieq $norm) }
}

function Stop-PreviewProcess {
    param([string]$ExpectedExe)
    $procs = Get-RunningPreviewProcesses -ExpectedExe $ExpectedExe
    foreach ($p in $procs) {
        try {
            Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue
        } catch {}
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(3)
    while ([DateTime]::UtcNow -lt $deadline) {
        $alive = Get-RunningPreviewProcesses -ExpectedExe $ExpectedExe
        if (-not $alive) { break }
        Start-Sleep -Milliseconds 100
    }
}

function Invoke-BuildAndRestart {
    param([switch]$Launch)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $ts = Get-Date -Format "HH:mm:ss"

    Push-Location $RepoRoot
    try {
        $hasLiveWindow = $null -ne (Get-RunningPreviewProcesses -ExpectedExe $ExePath)

        # 仅在需要保活旧预览窗口或 -CheckOnly 时先跑 cargo check
        if ($CheckOnly -or $hasLiveWindow) {
            Write-Host "`n[$ts] >>> 正在检查改动: qimu-client ($Profile)..." -ForegroundColor Cyan
            if ($Profile -eq "staging") {
                & cargo check --profile staging -p qimu-client
            } else {
                & cargo check -p qimu-client
            }
            if ($LASTEXITCODE -ne 0) {
                $failSec = [math]::Round($sw.Elapsed.TotalSeconds, 1)
                if ($hasLiveWindow) {
                    Write-Host "[$ts] [!] check 失败（${failSec}s），已保留当前预览窗口；修复并保存即可自动重试。" -ForegroundColor Red
                } else {
                    Write-Host "[$ts] [!] check 失败（${failSec}s）。" -ForegroundColor Red
                }
                return $false
            }
            if ($CheckOnly) {
                Write-Host "[$ts] [OK] check 通过（耗时 $([math]::Round($sw.Elapsed.TotalSeconds, 1))s）。" -ForegroundColor Green
                return $true
            }
            Stop-PreviewProcess -ExpectedExe $ExePath
        }

        Write-Host "[$ts] >>> 正在增量编译并链接 qimu-client.exe ($Profile)..." -ForegroundColor Cyan
        if ($Profile -eq "staging") {
            & cargo build --profile staging -p qimu-client
        } else {
            & cargo build -p qimu-client
        }
        if ($LASTEXITCODE -ne 0) {
            Write-Host "[$ts] [!] cargo build 失败（耗时 $([math]::Round($sw.Elapsed.TotalSeconds, 1))s）。" -ForegroundColor Red
            return $false
        }
    } finally {
        Pop-Location
    }

    $elapsed = [math]::Round($sw.Elapsed.TotalSeconds, 1)
    if ($Launch -and (Test-Path $ExePath)) {
        $proc = Start-Process -FilePath $ExePath -WorkingDirectory $RepoRoot -PassThru
        Write-Host "[$ts] [OK] 编译并启动预览成功（耗时 ${elapsed}s, PID=$($proc.Id)）-> $ExePath" -ForegroundColor Green
    } else {
        Write-Host "[$ts] [OK] 编译完成（耗时 ${elapsed}s）-> $ExePath" -ForegroundColor Green
    }
    return $true
}

Write-Host "=====================================================" -ForegroundColor DarkGray
Write-Host " 启幕 (qimu-client) 本地增量编译 / 自动重启预览" -ForegroundColor White
Write-Host " 仓库目录 : $RepoRoot" -ForegroundColor Gray
Write-Host " 缓存目录 : $TargetDirFull" -ForegroundColor Gray
Write-Host " 构建模式 : $Profile  (仅检查=$CheckOnly)" -ForegroundColor Gray
if ($Page -or $Open) {
    Write-Host " 启动视图 : QIMU_PAGE='$Page' QIMU_OPEN='$Open'" -ForegroundColor Gray
}
Write-Host " 操作提示 : 保存 .rs / Cargo.toml / assets 自动重编；按 R 手动重编；Ctrl+C 退出。" -ForegroundColor Gray
Write-Host "=====================================================" -ForegroundColor DarkGray

if (-not $NoInitialLaunch) {
    [void](Invoke-BuildAndRestart -Launch:(-not $CheckOnly))
}

$WatchState = [hashtable]::Synchronized(@{
    Pending    = $false
    LastChange = [DateTime]::MinValue
    LastPath   = ""
})

$RootCargo = New-Object System.IO.FileSystemWatcher
$RootCargo.Path = $RepoRoot
$RootCargo.Filter = "Cargo.*"
$RootCargo.IncludeSubdirectories = $false
$RootCargo.NotifyFilter = [System.IO.NotifyFilters]'FileName,LastWrite,Size,CreationTime'
$RootCargo.EnableRaisingEvents = $true

$Watchers = @($RootCargo)
$CratesDir = Join-Path $RepoRoot "crates"
if (Test-Path $CratesDir) {
    $w = New-Object System.IO.FileSystemWatcher
    $w.Path = $CratesDir
    $w.IncludeSubdirectories = $true
    $w.NotifyFilter = [System.IO.NotifyFilters]'FileName,LastWrite,Size,CreationTime'
    $w.EnableRaisingEvents = $true
    $Watchers += $w
}

$handler = {
    $st = $Event.MessageData
    $path = $Event.SourceEventArgs.FullPath
    if (-not $path) { return }
    if ($path -match '\\(target|\.git|vendor)\\') { return }
    if ($path -notmatch '\.(rs|toml|tsv|b64|ico|png)$') { return }
    $st.Pending = $true
    $st.LastChange = [DateTime]::UtcNow
    $st.LastPath = $path
}

$Subs = @()
foreach ($w in $Watchers) {
    foreach ($evt in @("Changed", "Created", "Renamed", "Deleted")) {
        $Subs += Register-ObjectEvent -InputObject $w -EventName $evt -MessageData $WatchState -Action $handler
    }
}

try {
    while ($true) {
        Start-Sleep -Milliseconds 120
        if (-not [Console]::IsInputRedirected) {
            try {
                if ([Console]::KeyAvailable) {
                    $key = [Console]::ReadKey($true)
                    if ($key.Key -eq [ConsoleKey]::R) {
                        Write-Host "`n[$(Get-Date -Format 'HH:mm:ss')] 手动触发重编..." -ForegroundColor Yellow
                        [void](Invoke-BuildAndRestart -Launch:(-not $CheckOnly))
                        $WatchState.Pending = $false
                        continue
                    }
                }
            } catch {}
        }

        if ($WatchState.Pending) {
            $quietMs = ([DateTime]::UtcNow - $WatchState.LastChange).TotalMilliseconds
            if ($quietMs -ge $DebounceMs) {
                $WatchState.Pending = $false
                $rel = $WatchState.LastPath
                if ($rel.StartsWith($RepoRoot + "\")) {
                    $rel = $rel.Substring($RepoRoot.Length + 1)
                }
                Write-Host "`n[$(Get-Date -Format 'HH:mm:ss')] 检测到变更: $rel" -ForegroundColor Yellow
                [void](Invoke-BuildAndRestart -Launch:(-not $CheckOnly))
                $WatchState.Pending = $false
            }
        }
    }
} finally {
    foreach ($sub in $Subs) {
        Unregister-Event -SubscriptionId $sub.Id -ErrorAction SilentlyContinue
        Remove-Job -Id $sub.Id -Force -ErrorAction SilentlyContinue
    }
    foreach ($w in $Watchers) {
        $w.EnableRaisingEvents = $false
        $w.Dispose()
    }
    if (-not $CheckOnly) {
        Stop-PreviewProcess -ExpectedExe $ExePath
    }
    Write-Host "`n已停止本地预览监听。" -ForegroundColor Gray
}
