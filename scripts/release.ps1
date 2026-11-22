# Publish a new STLR version to GitHub Releases; installed copies pick it up via the in-app updater.
# Usage:  powershell -File scripts/release.ps1 -Version 0.3.0 [-Notes "What changed"]
# Needs: gh (logged in), the signing key at ~/.tauri/stlr-updater.key (never commit it; back it up:
# losing it means installed copies can no longer verify, i.e. receive, updates).
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$Notes = "See the commit history for changes.",
    [string]$Key = "$HOME\.tauri\stlr-updater.key"
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot\.."
Set-Location $root
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version must look like 1.2.3" }
if (git status --porcelain) { throw "Working tree not clean: commit first" }
if (-not (Test-Path $Key)) { throw "Signing key not found: $Key" }

# 1. Bump the version (tauri.conf.json is what the updater compares against). UTF-8 without BOM.
$conf = "src-tauri\tauri.conf.json"
$json = [IO.File]::ReadAllText("$root\$conf")
$json = $json -replace '"version":\s*"[^"]+"', "`"version`": `"$Version`""
[IO.File]::WriteAllText("$root\$conf", $json, (New-Object Text.UTF8Encoding $false))

# 2. Signed build (updater artifacts only here, so plain `npm run tauri build` needs no key).
$env:TAURI_SIGNING_PRIVATE_KEY = [IO.File]::ReadAllText($Key)
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
$extra = Join-Path $env:TEMP "stlr-updater-conf.json" # a file: inline JSON quoting breaks in PowerShell 5.1
[IO.File]::WriteAllText($extra, '{"bundle":{"createUpdaterArtifacts":true}}')
npm run tauri build -- --config $extra
if ($LASTEXITCODE) { throw "build failed" }
powershell -File scripts\smoke-release.ps1
if ($LASTEXITCODE) { throw "smoke test failed" }

# 3. Assets: installer (no spaces: GitHub renames them), its signature inside latest.json.
$nsis = "src-tauri\target\release\bundle\nsis"
$setup = "$nsis\STLR Search_${Version}_x64-setup.exe"
$asset = "STLR-Search_${Version}_x64-setup.exe"
$out = Join-Path $env:TEMP "stlr-release-$Version"
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item $setup "$out\$asset"
$latest = [ordered]@{
    version   = $Version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = @{
        "windows-x86_64" = @{
            signature = [IO.File]::ReadAllText("$root\$setup.sig")
            url       = "https://github.com/vrouzbitek/STLR-SearchEngine/releases/download/v$Version/$asset"
        }
    }
}
[IO.File]::WriteAllText("$out\latest.json", ($latest | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding $false))

# 4. Commit, tag, publish.
git commit -q -m "release: v$Version" -- $conf
git tag "v$Version"
git push origin HEAD "v$Version"
gh release create "v$Version" "$out\$asset" "$out\latest.json" --title "STLR Search $Version" --notes $Notes
if ($LASTEXITCODE) { throw "gh release failed" }
Write-Host "Released v$Version"
