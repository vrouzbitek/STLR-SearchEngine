# Release smoke test: the built exe must start and stay up (catches startup panics that only happen in
# release builds, e.g. a webview invoking a command before AppState is managed).
# Usage, after `npm run tauri build`:  powershell -File scripts/smoke-release.ps1
param(
    [string]$Exe = "$PSScriptRoot\..\src-tauri\target\release\stlr-search.exe",
    [int]$Seconds = 5
)
$ErrorActionPreference = "Stop"
$err = Join-Path $env:TEMP "stlr-smoke-stderr.txt"
$p = Start-Process -FilePath $Exe -PassThru -RedirectStandardError $err -RedirectStandardOutput "$err.out"
$null = $p.Handle # keep the handle, otherwise ExitCode is empty once the process is gone
Start-Sleep -Seconds $Seconds
$alive = -not $p.HasExited
$stderr = Get-Content $err -Raw -ErrorAction SilentlyContinue
if ($alive) { Stop-Process -Id $p.Id -Force } # only the instance we started
if (-not $alive -or $stderr -match "panicked") {
    Write-Host "FAIL: exited=$(-not $alive) code=$($p.ExitCode)`n$stderr"
    exit 1
}
Write-Host "OK: release exe ran ${Seconds}s without a panic"
