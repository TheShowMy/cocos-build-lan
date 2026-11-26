[CmdletBinding()]
param([switch]$SkipBuild)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$stageRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot 'dist/npm'))
$web = [IO.Path]::GetFullPath((Join-Path $projectRoot 'target/dx/cocos-build-lan/release/web/public'))
if (-not $stageRoot.StartsWith($projectRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid staging path' }
if (-not $web.StartsWith((Join-Path $projectRoot 'target') + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid Web output path' }
Push-Location $projectRoot
try {
    $package = Get-Content npm/package.json -Raw | ConvertFrom-Json
    $metadataText = cargo metadata --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $metadata = $metadataText | ConvertFrom-Json
    $server = $metadata.packages | Where-Object name -eq 'cocos-build-lan-server'
    if ($server.version -ne $package.version) { throw 'npm and Rust versions differ' }
    if (-not $SkipBuild) {
        pnpm --dir crates/tool-app/editor install --frozen-lockfile
        if ($LASTEXITCODE -ne 0) { throw 'Editor install failed' }
        pnpm --dir crates/tool-app/editor run build
        if ($LASTEXITCODE -ne 0) { throw 'Editor build failed' }
        # Dioxus retains old hashed assets across builds. Do not ship stale bundles.
        if (Test-Path -LiteralPath $web) { Remove-Item -LiteralPath $web -Recurse -Force }
        dx build --release --package cocos-build-lan-app --debug-symbols false
        if ($LASTEXITCODE -ne 0) { throw 'Web build failed' }
        cargo build --release -p cocos-build-lan-server --locked
        if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
    }
    $binary = Join-Path $projectRoot 'target/release/cocos-build-lan-server.exe'
    foreach ($required in @($binary, (Join-Path $web 'index.html'))) { if (-not (Test-Path -LiteralPath $required)) { throw "Missing input: $required" } }
    $binaryVersion = & $binary --version
    if ($LASTEXITCODE -ne 0 -or $binaryVersion -ne "cocos-build-lan $($package.version)") { throw 'Stale binary version' }
    if (Test-Path -LiteralPath $stageRoot) { Remove-Item -LiteralPath $stageRoot -Recurse -Force }
    New-Item -ItemType Directory -Path (Join-Path $stageRoot 'runtime/scripts') -Force | Out-Null
    Copy-Item -LiteralPath npm/package.json -Destination $stageRoot
    Copy-Item -LiteralPath npm/bin -Destination $stageRoot -Recurse
    Copy-Item -LiteralPath README.md -Destination $stageRoot
    Copy-Item -LiteralPath $binary -Destination (Join-Path $stageRoot 'runtime')
    Copy-Item -LiteralPath $web -Destination (Join-Path $stageRoot 'runtime/web') -Recurse
    Get-ChildItem -LiteralPath crates/tool-server/scripts -File | Where-Object { $_.Extension -in '.cjs', '.json' -and $_.Name -ne 'package.json' } | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $stageRoot 'runtime/scripts') }
    Push-Location $stageRoot
    try {
        $packText = npm pack --json --pack-destination ..
        if ($LASTEXITCODE -ne 0) { throw 'npm pack failed' }
        $pack = @($packText | ConvertFrom-Json)[0]
        foreach ($required in @('bin/cocos-build-lan.mjs', 'runtime/cocos-build-lan-server.exe', 'runtime/web/index.html', 'runtime/scripts/ast_obfuscate.cjs')) {
            if ($required -notin $pack.files.path) { throw "Missing packed file: $required" }
        }
        if ($pack.size -ge 50MB) { throw 'npm package exceeds the registry 50 MB limit' }
        if ($pack.files.path | Where-Object { $_ -match '(^|/)(config|logs|preps|\.git)/|tool-settings|runtime_state|active\.json' }) { throw 'Runtime data found in npm package' }
        Write-Output "Package: $(Join-Path $projectRoot "dist/$($pack.filename)")"
        Write-Output "Size: $($pack.size) bytes; integrity: $($pack.integrity)"
    } finally { Pop-Location }
} finally { Pop-Location }
