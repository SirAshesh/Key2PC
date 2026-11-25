param(
    [string]$Version = "",
    [string]$BinaryPath = "",
    [switch]$SkipCargoBuild
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    if ([string]::IsNullOrWhiteSpace($Version)) {
        $versionLine = Select-String -Path "crates\key2pc\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
        if (-not $versionLine) {
            throw "Failed to read version from crates\key2pc\Cargo.toml"
        }
        $Version = $versionLine.Matches[0].Groups[1].Value
    }

    if (-not $SkipCargoBuild) {
        cargo build --release --workspace
        if ($LASTEXITCODE -ne 0) {
            exit $LASTEXITCODE
        }
    }

    if ([string]::IsNullOrWhiteSpace($BinaryPath)) {
        $BinaryPath = Join-Path $repoRoot "target\release\key2pc.exe"
    }
    if (-not (Test-Path -LiteralPath $BinaryPath)) {
        throw "Release binary not found at $BinaryPath"
    }

    $wixCmd = Get-Command wix -ErrorAction SilentlyContinue
    if ($wixCmd) {
        $wixExe = $wixCmd.Source
    } else {
        $dotnetToolWix = Join-Path $env:USERPROFILE ".dotnet\tools\wix.exe"
        $wingetWix = Get-ChildItem -Path "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Filter "wix.exe" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
        if (Test-Path -LiteralPath $dotnetToolWix) {
            $wixExe = $dotnetToolWix
        } elseif ($wingetWix -and (Test-Path -LiteralPath $wingetWix)) {
            $wixExe = $wingetWix
        } else {
            throw "WiX Toolset CLI ('wix') not found on PATH or in standard tool directories."
        }
    }

    $distDir = Join-Path $repoRoot "dist"
    New-Item -ItemType Directory -Force -Path $distDir | Out-Null
    $msiOut = Join-Path $distDir "key2pc-windows-x86_64.msi"

    $wxsPath = Join-Path $repoRoot "installer\wix\Key2PC.wxs"
    $configPath = Join-Path $repoRoot "config.json"
    $readmePath = Join-Path $repoRoot "README.md"
    $licensePath = Join-Path $repoRoot "LICENSE"
    $iconPath = Join-Path $repoRoot "installer\wix\key2pc.ico"

    & $wixExe build $wxsPath `
        -arch x64 `
        -d "Version=$Version" `
        -d "BinaryPath=$BinaryPath" `
        -d "ConfigPath=$configPath" `
        -d "ReadmePath=$readmePath" `
        -d "LicensePath=$licensePath" `
        -d "IconPath=$iconPath" `
        -o $msiOut
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    Write-Host "Built MSI installer: $msiOut"
}
finally {
    Pop-Location
}
