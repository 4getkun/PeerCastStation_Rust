param(
    [Parameter(Mandatory = $true)][string]$PlayerSource,
    [string]$Destination = (Join-Path $PSScriptRoot '..\target\windows-x64'),
    [ValidateSet('debug', 'release', 'win11-x64')][string]$Profile = 'release',
    [switch]$SkipBuild,
    [string]$YouTubeBundle = ''
)
$ErrorActionPreference = 'Stop'
$packageRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$releaseVersion = (Get-Content -LiteralPath (Join-Path $packageRoot 'crates/peercast-desktop/tauri.conf.json') -Raw | ConvertFrom-Json).version
$sourceRoot = (Resolve-Path -LiteralPath $PlayerSource).Path
$outputRoot = [IO.Path]::GetFullPath($Destination)
if ($outputRoot.StartsWith($sourceRoot.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase) -or $outputRoot -eq $sourceRoot) {
    throw 'The package destination must be separate from the player source.'
}
foreach ($required in @('PCRPlayer64.exe', 'LICENSE.txt', 'skin')) {
    if (-not (Test-Path -LiteralPath (Join-Path $sourceRoot $required))) { throw "Missing player file: $required" }
}
if (-not $SkipBuild) {
    Push-Location $packageRoot
    try {
        $cargoProfile = if ($Profile -eq 'debug') { 'dev' } else { $Profile }
        # This workspace has previously exhausted memory at higher parallelism.
        cargo build -p peercast-desktop --profile $cargoProfile -j 2
        if ($LASTEXITCODE -ne 0) { throw 'Desktop build failed.' }
    }
    finally { Pop-Location }
}
New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null
$youtubeRoot = if ($YouTubeBundle) { [IO.Path]::GetFullPath($YouTubeBundle) } else { Join-Path $packageRoot 'target/youtube-helper/bundle' }
if (-not (Test-Path -LiteralPath (Join-Path $youtubeRoot 'YouTube/yt-dlp.exe'))) { throw 'Run tools/prepare-youtube-helper.ps1 before packaging.' }
$ytSpec = Get-Content -LiteralPath (Join-Path $packageRoot 'tools/yt-dlp/lock.json') -Raw | ConvertFrom-Json
if ((Get-FileHash -LiteralPath (Join-Path $youtubeRoot 'YouTube/yt-dlp.exe') -Algorithm SHA256).Hash.ToLowerInvariant() -cne $ytSpec.binary.sha256) { throw 'yt-dlp helper hash mismatch.' }
Copy-Item -LiteralPath (Join-Path $youtubeRoot 'YouTube') -Destination $outputRoot -Recurse -Force
Copy-Item -LiteralPath (Join-Path $youtubeRoot 'licenses') -Destination $outputRoot -Recurse -Force
$playerRoot = Join-Path $outputRoot 'PCRPlayer'
New-Item -ItemType Directory -Path $playerRoot -Force | Out-Null
$binary = Join-Path $packageRoot "target\$Profile\peercast-desktop.exe"
if ($SkipBuild -and -not (Test-Path -LiteralPath $binary)) {
    # Tauri renames the Cargo binary to mainBinaryName after bundling.
    $binary = Join-Path $packageRoot "target\$Profile\PeerCast-RT.exe"
}
Copy-Item -LiteralPath $binary -Destination (Join-Path $outputRoot 'Peercast-RT.exe') -Force
foreach ($name in @('PCRPlayer64.exe', 'LICENSE.txt')) {
    Copy-Item -LiteralPath (Join-Path $sourceRoot $name) -Destination (Join-Path $playerRoot $name) -Force
}
# Keep runtime settings/history out of a fresh package, and preserve existing settings on updates.
foreach ($file in Get-ChildItem -LiteralPath (Join-Path $sourceRoot 'skin') -Recurse -File) {
    if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Player skin symlinks are not supported.' }
    $relative = [IO.Path]::GetRelativePath($sourceRoot, $file.FullName)
    $target = Join-Path $playerRoot $relative
    New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
    Copy-Item -LiteralPath $file.FullName -Destination $target -Force
}
$playerLicenseFiles = @()
$playerLicenseRoot = Join-Path $sourceRoot 'licenses'
if (Test-Path -LiteralPath $playerLicenseRoot) {
    foreach ($file in Get-ChildItem -LiteralPath $playerLicenseRoot -Recurse -File) {
        if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Player license symlinks are not supported.' }
        $relative = [IO.Path]::GetRelativePath($sourceRoot, $file.FullName)
        $target = Join-Path $playerRoot $relative
        New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
        Copy-Item -LiteralPath $file.FullName -Destination $target -Force
        $playerLicenseFiles += $relative
    }
}
Copy-Item -LiteralPath (Join-Path $packageRoot 'crates\peercast-desktop\frontend\fonts\OFL.txt') -Destination (Join-Path $outputRoot 'NotoSansJP-OFL.txt') -Force
Copy-Item -LiteralPath (Join-Path $packageRoot 'crates\peercast-desktop\frontend\assets\tabler\LICENSE.txt') -Destination (Join-Path $outputRoot 'Tabler-Icons-MIT.txt') -Force
Copy-Item -LiteralPath (Join-Path $packageRoot 'crates\peercast-desktop\icons\LICENSE.txt') -Destination (Join-Path $outputRoot 'LICENSE-GPL-3.0.txt') -Force
Copy-Item -LiteralPath (Join-Path $packageRoot 'docs\LICENSE_AUDIT.md') -Destination (Join-Path $outputRoot 'LICENSE_AUDIT.md') -Force
@(
    'PeerCast RT for Windows x64'
    ''
    'Peercast-RT.exe を起動してください。視聴用の PCRPlayer は同梱されています。'
    'この配布物は GNU General Public License version 3 or later の条件で公開しています。'
    ''
    'ソースコード: https://github.com/4getkun/PeerCastStation_Rust'
) | Set-Content -LiteralPath (Join-Path $outputRoot 'README-JA.txt') -Encoding utf8
@(
    'PeerCast RT source code:'
    'https://github.com/4getkun/PeerCastStation_Rust'
    ''
    'This binary is distributed under GNU General Public License version 3 or later.'
    ''
    'Bundled modified PCRPlayer corresponding source and build inputs:'
    "https://peercast-rt-downloads.fourgetkun.com/releases/$releaseVersion/PCRPlayer-PeerCastRT-$releaseVersion-sources.7z"
) | Set-Content -LiteralPath (Join-Path $outputRoot 'SOURCE.txt') -Encoding utf8
$packagedFiles = @('PCRPlayer64.exe', 'LICENSE.txt') + @(Get-ChildItem -LiteralPath (Join-Path $sourceRoot 'skin') -Recurse -File | ForEach-Object { [IO.Path]::GetRelativePath($sourceRoot, $_.FullName) }) + $playerLicenseFiles
$manifest = foreach ($relative in $packagedFiles) {
    $sourceHash = (Get-FileHash -LiteralPath (Join-Path $sourceRoot $relative) -Algorithm SHA256).Hash
    $copyHash = (Get-FileHash -LiteralPath (Join-Path $playerRoot $relative) -Algorithm SHA256).Hash
    if ($sourceHash -ne $copyHash) { throw "Player copy verification failed: $relative" }
    [ordered]@{ path = 'PCRPlayer/' + $relative.Replace('\', '/'); sha256 = $copyHash }
}
[ordered]@{ defaultPlayer = 'PCRPlayer/PCRPlayer64.exe'; personalConfigCopied = $false; files = @($manifest) } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $outputRoot 'player-bundle.json') -Encoding utf8
Write-Output "Packaged Windows x64 application with PCRPlayer ($($manifest.Count) verified files): $outputRoot"
