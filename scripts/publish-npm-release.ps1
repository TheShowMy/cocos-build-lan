[CmdletBinding()]
param([ValidateSet('next', 'latest')][string]$Tag = 'next')
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$package = Get-Content (Join-Path $projectRoot 'npm/package.json') -Raw | ConvertFrom-Json
$registry = $package.publishConfig.registry
$spec = "$($package.name)@$($package.version)"
$tarball = Join-Path $projectRoot "dist/tsixi-cocos-build-lan-$($package.version).tgz"
if (-not (Test-Path -LiteralPath $tarball)) { throw 'Run build-npm-release.ps1 first' }
if ($Tag -eq 'next') {
    npm publish $tarball --tag next --registry $registry
} else {
    $remoteIntegrity = npm view $spec dist.integrity --registry $registry --json --prefer-online
    if ($LASTEXITCODE -ne 0) { throw 'Cannot verify candidate package' }
    $sha = [Security.Cryptography.SHA512]::Create()
    try { $localIntegrity = 'sha512-' + [Convert]::ToBase64String($sha.ComputeHash([IO.File]::ReadAllBytes($tarball))) } finally { $sha.Dispose() }
    if (($remoteIntegrity | ConvertFrom-Json) -ne $localIntegrity) { throw 'Registry candidate differs from the tested local tarball' }
    npm dist-tag add $spec latest --registry $registry
}
if ($LASTEXITCODE -ne 0) { throw 'npm publish/dist-tag failed' }
npm view "$($package.name)@$Tag" version dist.integrity --registry $registry --json --prefer-online
if ($LASTEXITCODE -ne 0) { throw 'Registry verification failed' }
