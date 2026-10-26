#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$PlayerArchive,
    [string]$Version = '1.1.1',
    [string]$ReleaseNotes = '自動更新と新しいインストーラーデザインを追加しました。',
    [string]$WorkDirectory = '',
    [string]$ReleaseDirectory = '',
    [switch]$SkipBuild,
    [switch]$Publish
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-updater-artifacts.ps1')
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$desktopRoot = Join-Path $repoRoot 'crates/peercast-desktop'
$tauriPath = Join-Path $desktopRoot 'tauri.conf.json'
$cargoPath = Join-Path $desktopRoot 'Cargo.toml'
$tauri = Get-Content -LiteralPath $tauriPath -Raw | ConvertFrom-Json
$cargoText = Get-Content -LiteralPath $cargoPath -Raw
if ($cargoText -notmatch '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"') { throw 'Desktop Cargo version could not be read.' }
$cargoVersion = $Matches[1]
if ([string]$tauri.version -cne $Version -or $cargoVersion -cne $Version) {
    throw "Version mismatch: requested=$Version, tauri=$($tauri.version), cargo=$cargoVersion"
}

$workRoot = if ([string]::IsNullOrWhiteSpace($WorkDirectory)) {
    Join-Path $repoRoot "target/release-$Version"
} else {
    [IO.Path]::GetFullPath($WorkDirectory)
}
$releaseRoot = if ([string]::IsNullOrWhiteSpace($ReleaseDirectory)) {
    Join-Path $repoRoot "target/github-release-v$Version"
} else {
    [IO.Path]::GetFullPath($ReleaseDirectory)
}
$bundleInstaller = Join-Path $repoRoot "target/release/bundle/nsis/PeerCast RT_${Version}_x64-setup.exe"
$bundleSignature = "$bundleInstaller.sig"
$installerName = "PeercastRT_${Version}_x64-setup.exe"
$zipName = "PeerCastRT-${Version}-windows-x64-installer.7z"
$installerAsset = Join-Path $releaseRoot $installerName
$zipAsset = Join-Path $releaseRoot $zipName
$licenseRoot = Join-Path $releaseRoot 'zip-root/licenses'

if (-not $SkipBuild) {
    & node (Join-Path $desktopRoot 'icons/scripts/generate_nsis_branding.cjs')
    if ($LASTEXITCODE -ne 0) { throw 'Installer branding generation failed.' }
    & (Join-Path $PSScriptRoot 'build-installer.ps1') -PlayerArchive $PlayerArchive -Version $Version -WorkDirectory $workRoot
    if ($LASTEXITCODE -ne 0) { throw 'Installer build failed.' }
    & (Join-Path $PSScriptRoot 'package-release-assets.ps1') `
        -InstallerPath $bundleInstaller `
        -PackageStage (Join-Path $workRoot 'package-stage') `
        -OutputDirectory $releaseRoot `
        -Version $Version
    if ($LASTEXITCODE -ne 0) { throw 'Release packaging failed.' }
}

foreach ($path in @($bundleInstaller, $bundleSignature, $installerAsset, $zipAsset, "$installerAsset.sha256", "$zipAsset.sha256")) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Release artifact is missing: $path" }
}
$signature = (Get-Content -LiteralPath $bundleSignature -Raw).Trim()
$null = Assert-UpdaterSignatureVersion -SignatureText $signature -ExpectedVersion $Version
$renamedSignature = "$installerAsset.sig"
Copy-Item -LiteralPath $bundleSignature -Destination $renamedSignature -Force

$dedicatedBase = 'https://peercast-rt-downloads.fourgetkun.com'
$legacyBase = 'https://downloads.misaka.fourgetkun.com'
$dedicatedArtifactKey = "releases/$Version/$installerName"
$legacyArtifactKey = "peercast-rt/releases/$Version/$installerName"
$dedicatedVersionedKey = "updates/windows-x64/PeercastRT_$Version.json"
$legacyVersionedKey = "peercast-rt/updates/windows-x64/PeercastRT_$Version.json"
$dedicatedLatestKey = 'updates/stable/windows-x64/latest.json'
$legacyLatestKey = 'peercast-rt/updates/stable/windows-x64/latest.json'
$dedicatedManifestPath = Join-Path $releaseRoot "PeercastRT_${Version}_updater.json"
$legacyManifestPath = Join-Path $releaseRoot "PeercastRT_${Version}_legacy-updater.json"
$dedicatedLatestPath = Join-Path $releaseRoot 'latest.json'
$legacyLatestPath = Join-Path $releaseRoot 'latest-legacy.json'
$publishedAt = [DateTime]::UtcNow.ToString("yyyy-MM-dd'T'HH:mm:ss'Z'", [Globalization.CultureInfo]::InvariantCulture)

function Write-UpdaterManifest([string]$Path, [string]$ArtifactUrl) {
    [ordered]@{
        version = $Version
        notes = $ReleaseNotes
        pub_date = $publishedAt
        platforms = [ordered]@{
            'windows-x86_64' = [ordered]@{ url = $ArtifactUrl; signature = $signature }
        }
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $Path -Encoding utf8
}

Write-UpdaterManifest $dedicatedManifestPath "$dedicatedBase/$dedicatedArtifactKey"
Write-UpdaterManifest $legacyManifestPath "$legacyBase/$legacyArtifactKey"
Copy-Item -LiteralPath $dedicatedManifestPath -Destination $dedicatedLatestPath -Force
Copy-Item -LiteralPath $legacyManifestPath -Destination $legacyLatestPath -Force

$installerHash = (Get-FileHash -LiteralPath $installerAsset -Algorithm SHA256).Hash.ToLowerInvariant()
$zipHash = (Get-FileHash -LiteralPath $zipAsset -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Output "Version: $Version"
Write-Output "Installer: $installerAsset"
Write-Output "Installer SHA256: $installerHash"
Write-Output "7z: $zipAsset"
Write-Output "7z SHA256: $zipHash"
Write-Output "Updater signature version: $Version"
if (-not $Publish) {
    Write-Output 'Dry run complete. No remote objects were changed.'
    return
}

$wrangler = 'C:\Github\fourgetkun-hub\node_modules\.bin\wrangler.cmd'
if (-not (Test-Path -LiteralPath $wrangler -PathType Leaf)) { throw "Project Wrangler is missing: $wrangler" }
$credentialNames = @('CLOUDFLARE_API_TOKEN', 'CLOUDFLARE_API_KEY', 'CF_API_TOKEN', 'CF_API_KEY')

function Invoke-WranglerPut([string]$Bucket, [string]$Key, [string]$File, [string]$ContentType, [string]$CacheControl = '') {
    $saved = @{}
    foreach ($name in $credentialNames) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        Remove-Item "Env:$name" -ErrorAction SilentlyContinue
    }
    try {
        $arguments = @('r2', 'object', 'put', "$Bucket/$Key", "--file=$File", '--remote', "--content-type=$ContentType")
        if ($CacheControl) { $arguments += "--cache-control=$CacheControl" }
        $output = & $wrangler @arguments 2>&1
        $exit = $LASTEXITCODE
    }
    finally {
        foreach ($name in $credentialNames) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    }
    if ($exit -ne 0) {
        $safe = ($output | ForEach-Object { [string]$_ -replace '(?i)(api[_ -]?token|authorization|bearer)\s*[:=]?\s*\S+', '$1 [redacted]' }) -join [Environment]::NewLine
        throw "R2 upload failed for $Bucket/$Key (exit $exit): $safe"
    }
    Write-Output "Uploaded: r2://$Bucket/$Key"
}

function Test-PublicObject([string]$Url, [string]$File, [string]$ContentType) {
    $temporary = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString('N') + '.bin')
    try {
        $verifyUrl = "${Url}?release-check=$([Guid]::NewGuid().ToString('N'))"
        $response = Invoke-WebRequest -Uri $verifyUrl -OutFile $temporary -PassThru -HttpVersion 2.0 -TimeoutSec 120 -SkipHttpErrorCheck
        if ([int]$response.StatusCode -lt 200 -or [int]$response.StatusCode -ge 300) { throw "Public object returned HTTP $([int]$response.StatusCode): $Url" }
        $expected = Get-Item -LiteralPath $File
        $actual = Get-Item -LiteralPath $temporary
        $expectedHash = (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash
        $actualHash = (Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash
        if ($expected.Length -ne $actual.Length -or $expectedHash -ne $actualHash) { throw "Public object content mismatch: $Url" }
        if ($response.Headers['Content-Type'] -notlike "$($ContentType.Split(';')[0])*") { throw "Unexpected Content-Type for ${Url}: $($response.Headers['Content-Type'])" }
        Write-Output "Verified HTTP/$($response.BaseResponse.Version) $([int]$response.StatusCode): $Url"
    }
    finally { Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue }
}

$immutable = @(
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key=$dedicatedArtifactKey; File=$installerAsset; Type='application/octet-stream' },
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key="$dedicatedArtifactKey.sig"; File=$renamedSignature; Type='text/plain; charset=utf-8' },
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key="releases/$Version/$zipName"; File=$zipAsset; Type='application/x-7z-compressed' },
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key="releases/$Version/$installerName.sha256"; File="$installerAsset.sha256"; Type='text/plain; charset=utf-8' },
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key="releases/$Version/$zipName.sha256"; File="$zipAsset.sha256"; Type='text/plain; charset=utf-8' },
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key=$dedicatedVersionedKey; File=$dedicatedManifestPath; Type='application/json' },
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key='licenses/LICENSE-GPL-3.0.txt'; File=(Join-Path $licenseRoot 'LICENSE-GPL-3.0.txt'); Type='text/plain; charset=utf-8' },
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key='licenses/LICENSE_AUDIT.md'; File=(Join-Path $licenseRoot 'LICENSE_AUDIT.md'); Type='text/markdown; charset=utf-8' },
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key='licenses/PCRPlayer-LICENSE.txt'; File=(Join-Path $licenseRoot 'PCRPlayer-LICENSE.txt'); Type='text/plain; charset=utf-8' },
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key='licenses/NotoSansJP-OFL.txt'; File=(Join-Path $licenseRoot 'NotoSansJP-OFL.txt'); Type='text/plain; charset=utf-8' },
    @{ Bucket='peercast-rt-downloads'; Base=$dedicatedBase; Key='licenses/Tabler-Icons-MIT.txt'; File=(Join-Path $licenseRoot 'Tabler-Icons-MIT.txt'); Type='text/plain; charset=utf-8' },
    @{ Bucket='misaka-downloads'; Base=$legacyBase; Key=$legacyArtifactKey; File=$installerAsset; Type='application/octet-stream' },
    @{ Bucket='misaka-downloads'; Base=$legacyBase; Key="$legacyArtifactKey.sig"; File=$renamedSignature; Type='text/plain; charset=utf-8' },
    @{ Bucket='misaka-downloads'; Base=$legacyBase; Key=$legacyVersionedKey; File=$legacyManifestPath; Type='application/json' }
)

$ytSpec = Get-Content -LiteralPath (Join-Path $repoRoot 'tools/yt-dlp/lock.json') -Raw | ConvertFrom-Json
$ytSourceName = "yt-dlp-$($ytSpec.version)-sources.7z"
$ytSourceFile = Join-Path $repoRoot "target/youtube-helper/$ytSourceName"
foreach ($source in @(@{File=$ytSourceFile;Name=$ytSourceName;Type='application/x-7z-compressed'}, @{File="$ytSourceFile.sha256";Name="$ytSourceName.sha256";Type='text/plain; charset=utf-8'})) {
    if (-not (Test-Path -LiteralPath $source.File)) { throw "Missing yt-dlp source asset: $($source.File)" }
    $immutable += @{Bucket='peercast-rt-downloads';Base=$dedicatedBase;Key="releases/$Version/$($source.Name)";File=$source.File;Type=$source.Type}
}
foreach ($notice in Get-ChildItem -LiteralPath (Join-Path $licenseRoot 'yt-dlp') -File) {
    $noticeType = if ($notice.Extension -eq '.json') { 'application/json' } else { 'text/plain; charset=utf-8' }
    $immutable += @{Bucket='peercast-rt-downloads';Base=$dedicatedBase;Key="releases/$Version/licenses/yt-dlp/$($notice.Name)";File=$notice.FullName;Type=$noticeType}
}

foreach ($item in $immutable) { Invoke-WranglerPut $item.Bucket $item.Key $item.File $item.Type }
foreach ($item in $immutable) { Test-PublicObject "$($item.Base)/$($item.Key)" $item.File $item.Type }

# Mutable stable pointers are switched only after every immutable object is verified.
Invoke-WranglerPut 'peercast-rt-downloads' $dedicatedLatestKey $dedicatedLatestPath 'application/json' 'no-store'
Invoke-WranglerPut 'misaka-downloads' $legacyLatestKey $legacyLatestPath 'application/json' 'no-store'
Test-PublicObject "$dedicatedBase/$dedicatedLatestKey" $dedicatedLatestPath 'application/json'
Test-PublicObject "$legacyBase/$legacyLatestKey" $legacyLatestPath 'application/json'
Write-Output 'R2 publication completed; both stable pointers were verified last over HTTP/2.'
