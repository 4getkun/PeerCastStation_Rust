#requires -Version 7.0
param(
    [string]$CacheDirectory = '',
    [string]$OutputDirectory = '',
    [string]$ReleaseVersion = '1.1.2'
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$lockFile = Join-Path $PSScriptRoot 'yt-dlp/lock.json'
$spec = Get-Content -LiteralPath $lockFile -Raw | ConvertFrom-Json
$cache = if ($CacheDirectory) { [IO.Path]::GetFullPath($CacheDirectory) } else { Join-Path $repo "target/yt-dlp-$($spec.version)" }
$output = if ($OutputDirectory) { [IO.Path]::GetFullPath($OutputDirectory) } else { Join-Path $repo 'target/youtube-helper' }
$helper = Join-Path $output 'bundle/YouTube'
$licenses = Join-Path $output 'bundle/licenses/yt-dlp'
$sourceRoot = Join-Path $output 'source-package'
New-Item -ItemType Directory -Path $cache,$helper,$licenses,$sourceRoot -Force | Out-Null
function Get-VerifiedFile($Item, [string]$Destination) {
    if (-not (Test-Path -LiteralPath $Destination -PathType Leaf)) {
        New-Item -ItemType Directory -Path (Split-Path -Parent $Destination) -Force | Out-Null
        Invoke-WebRequest -Uri $Item.url -OutFile $Destination -HttpVersion 2.0 -TimeoutSec 180
    }
    if ((Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant() -cne $Item.sha256) {
        throw "yt-dlp artifact digest mismatch: $($Item.name)"
    }
}
Get-VerifiedFile $spec.binary (Join-Path $cache $spec.binary.name)
Get-VerifiedFile $spec.notices (Join-Path $cache $spec.notices.name)
Get-VerifiedFile $spec.unlicense (Join-Path $cache $spec.unlicense.name)
Copy-Item -LiteralPath (Join-Path $cache $spec.binary.name) -Destination (Join-Path $helper 'yt-dlp.exe') -Force
Copy-Item -LiteralPath (Join-Path $cache $spec.notices.name) -Destination $licenses -Force
Copy-Item -LiteralPath (Join-Path $cache $spec.unlicense.name) -Destination $licenses -Force
Copy-Item -LiteralPath (Join-Path $repo 'crates/peercast-desktop/icons/LICENSE.txt') -Destination (Join-Path $licenses 'GPL-3.0.txt') -Force
Copy-Item -LiteralPath $lockFile -Destination (Join-Path $licenses 'source-manifest.json') -Force
$sourceName = "yt-dlp-$($spec.version)-sources.7z"
$sourceUrl = "https://peercast-rt-downloads.fourgetkun.com/releases/$ReleaseVersion/$sourceName"
@(
    "yt-dlp $($spec.version) - unmodified official Windows x64 executable"
    "Binary SHA-256: $($spec.binary.sha256)"
    'Executable license: GPL-3.0-or-later; yt-dlp source: Unlicense.'
    'Bundled component notices: THIRD_PARTY_LICENSES.txt'
    'This is a separate command-line program, invoked for metadata only.'
    "Complete corresponding source archives and build inputs: $sourceUrl"
    'Exact component versions, upstream URLs and SHA-256: source-manifest.json'
    'No Google credentials, cookies or API keys are included or requested.'
) | Set-Content -LiteralPath (Join-Path $licenses 'SOURCE.txt') -Encoding utf8
foreach ($item in $spec.sources) {
    $file = Join-Path $cache "sources/$($item.name)"
    Get-VerifiedFile $item $file
    Copy-Item -LiteralPath $file -Destination $sourceRoot -Force
}
Copy-Item -LiteralPath $lockFile -Destination (Join-Path $sourceRoot 'source-manifest.json') -Force
Copy-Item -LiteralPath (Join-Path $licenses 'GPL-3.0.txt') -Destination $sourceRoot -Force
Copy-Item -LiteralPath (Join-Path $cache $spec.notices.name) -Destination $sourceRoot -Force
@(
    "Corresponding sources for the official yt-dlp $($spec.version) Windows x64 executable."
    "Binary SHA-256: $($spec.binary.sha256)"
    ''
    'The source distributions are preserved unmodified in their original archives.'
    'source-manifest.json records the versions, URLs, and SHA-256 of every input.'
    'The yt-dlp GitHub tag archive includes .github/workflows/build.yml, bundle/requirements,'
    'bundle/pyinstaller.py, and devscripts: follow its windows x64 CPython build procedure.'
    "Interpreter source: CPython $($spec.pythonVersion), with its PCbuild scripts and external sources."
    "Custom PyInstaller wheel upstream commit: $($spec.pyinstallerCommit)."
    'The Pyinstaller-Builds tag contains the matching bootloader build workflow.'
    "curl-cffi native library: curl-impersonate $($spec.curlImpersonateVersion)."
    'Its tag archive includes native patches/build recipes; all eight CMake source inputs'
    'and their upstream sources are included alongside the Python package sdists.'
    'The original package sdists include their own licenses and build instructions.'
    'Additional build-only package sdists are included for completeness.'
    'Microsoft system runtime components are system libraries and are not reproduced here.'
) | Set-Content -LiteralPath (Join-Path $sourceRoot 'BUILD-SOURCES.txt') -Encoding utf8
$archive = Join-Path $output $sourceName
if (-not (Test-Path -LiteralPath $archive)) {
    & 'C:\Program Files\7-Zip\7z.exe' a -t7z -mx=1 -mmt=2 $archive "$sourceRoot\*" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'yt-dlp source archive creation failed.' }
}
& 'C:\Program Files\7-Zip\7z.exe' t $archive | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'yt-dlp source archive integrity check failed.' }
$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $sourceName" | Set-Content -LiteralPath "$archive.sha256" -Encoding ascii
Write-Output "Verified yt-dlp $($spec.version), $($spec.sources.Count) source archives: $output"
