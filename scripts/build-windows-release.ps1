[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$distRoot = Join-Path $projectRoot "dist"
$stageRoot = Join-Path $distRoot "bootstrap"
$zipPath = Join-Path $distRoot "cocos-build-lan-x86_64-pc-windows-msvc-bootstrap.zip"

Push-Location $projectRoot
try {
    $hostLine = rustc -vV | Where-Object { $_ -like "host:*" }
    $hostTarget = $hostLine.Substring("host: ".Length).Trim()
    if ($hostTarget -ne "x86_64-pc-windows-msvc") {
        throw "This script requires x86_64-pc-windows-msvc; current host is $hostTarget."
    }

    $installedTargets = @(rustup target list --installed)
    if ($installedTargets -notcontains "wasm32-unknown-unknown") {
        throw "Missing wasm32-unknown-unknown. Run: rustup target add wasm32-unknown-unknown"
    }
    $installedToolchains = @(rustup toolchain list)
    if (-not ($installedToolchains | Where-Object { $_ -like "nightly-*" })) {
        throw "Missing nightly Rust toolchain. Run: rustup toolchain install nightly"
    }
    if (-not (Get-Command dx -ErrorAction SilentlyContinue)) {
        throw "Missing dioxus-cli. Run: cargo install dioxus-cli --locked"
    }
    if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
        throw "Missing pnpm. Install pnpm before building the release."
    }

    $env:CI = "true"
    pnpm --dir crates/tool-app/editor install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { throw "CodeMirror dependency installation failed." }
    pnpm --dir crates/tool-app/editor run build
    if ($LASTEXITCODE -ne 0) { throw "CodeMirror bundle build failed." }
    $editorBundle = Join-Path $projectRoot "crates\tool-app\assets\editor.bundle.js"
    if (-not (Test-Path -LiteralPath $editorBundle)) {
        throw "CodeMirror bundle is missing: $editorBundle"
    }

    $webSource = Join-Path $projectRoot "target\dx\cocos-build-lan\release\web\public"
    if (Test-Path -LiteralPath $webSource) {
        Remove-Item -LiteralPath $webSource -Recurse -Force
    }
    dx build --release --package cocos-build-lan-app --verbose
    if ($LASTEXITCODE -ne 0) { throw "Dioxus web build failed." }
    cargo build --release -p cocos-build-lan-server -p cocos-build-lan-control
    if ($LASTEXITCODE -ne 0) { throw "Server or control release build failed." }
    cargo build --release -p cocos-build-lan-launcher
    if ($LASTEXITCODE -ne 0) { throw "Launcher release build failed." }

    $scriptsSource = Join-Path $projectRoot "crates\tool-server\scripts"
    $typescriptSource = Join-Path $scriptsSource "node_modules\.pnpm\typescript@5.9.3\node_modules\typescript"
    foreach ($requiredPath in @(
        (Join-Path $projectRoot "target\release\cocos-build-lan.exe"),
        (Join-Path $projectRoot "target\release\cocos-build-lan-server.exe"),
        (Join-Path $projectRoot "target\release\cocos-build-lan-control.exe"),
        $webSource,
        $scriptsSource,
        $typescriptSource,
        (Join-Path $projectRoot "tool.json")
    )) {
        if (-not (Test-Path -LiteralPath $requiredPath)) {
            throw "Required release input is missing: $requiredPath"
        }
    }

    if (Test-Path -LiteralPath $stageRoot) {
        Remove-Item -LiteralPath $stageRoot -Recurse -Force
    }
    New-Item -ItemType Directory -Path $stageRoot -Force | Out-Null
    $binRoot = Join-Path $stageRoot "bin"
    New-Item -ItemType Directory -Path $binRoot -Force | Out-Null

    Copy-Item -LiteralPath (Join-Path $projectRoot "target\release\cocos-build-lan.exe") -Destination $stageRoot
    Copy-Item -LiteralPath (Join-Path $projectRoot "target\release\cocos-build-lan-server.exe") -Destination $binRoot
    Copy-Item -LiteralPath (Join-Path $projectRoot "target\release\cocos-build-lan-control.exe") -Destination $binRoot
    Copy-Item -LiteralPath (Join-Path $projectRoot "tool.json") -Destination $stageRoot
    Copy-Item -LiteralPath $webSource -Destination (Join-Path $binRoot "web") -Recurse
    Copy-Item -LiteralPath $scriptsSource -Destination (Join-Path $binRoot "scripts") -Recurse

    $stagedTypescript = Join-Path $binRoot "scripts\node_modules\typescript"
    if (Test-Path -LiteralPath $stagedTypescript) {
        Remove-Item -LiteralPath $stagedTypescript -Recurse -Force
    }
    Copy-Item -LiteralPath $typescriptSource -Destination $stagedTypescript -Recurse

    if (Test-Path -LiteralPath $zipPath) {
        Remove-Item -LiteralPath $zipPath -Force
    }
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zipStream = [System.IO.File]::Open($zipPath, [System.IO.FileMode]::CreateNew)
    $archive = [System.IO.Compression.ZipArchive]::new(
        $zipStream,
        [System.IO.Compression.ZipArchiveMode]::Create,
        $false
    )
    try {
        Get-ChildItem -LiteralPath $stageRoot -Recurse -File | ForEach-Object {
            $entryName = $_.FullName.Substring($stageRoot.Length + 1).Replace("\", "/")
            $entry = $archive.CreateEntry(
                $entryName,
                [System.IO.Compression.CompressionLevel]::Optimal
            )
            $sourceStream = [System.IO.File]::OpenRead($_.FullName)
            $entryStream = $entry.Open()
            try {
                $sourceStream.CopyTo($entryStream)
            }
            finally {
                $entryStream.Dispose()
                $sourceStream.Dispose()
            }
        }
    }
    finally {
        $archive.Dispose()
        $zipStream.Dispose()
    }

    $zip = Get-Item -LiteralPath $zipPath
    $hash = Get-FileHash -Algorithm SHA256 -LiteralPath $zipPath
    Write-Output "Bootstrap: $($zip.FullName)"
    Write-Output "Size: $($zip.Length) bytes"
    Write-Output "SHA256: $($hash.Hash.ToLowerInvariant())"
    Write-Output "LAN web: $(Join-Path $binRoot 'web')"
    Write-Output "LAN scripts: $(Join-Path $binRoot 'scripts')"
}
finally {
    Pop-Location
}
