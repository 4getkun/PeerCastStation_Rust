#requires -Version 7.0
param(
    [Parameter(Mandatory)][string]$PlayerSource,
    [Parameter(Mandatory)][string]$FfmpegBundle,
    [string]$NodeBinary = '',
    [string]$Cache = (Join-Path $PSScriptRoot '../target/live-runtime-cache')
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $PlayerSource).Path
$ffmpeg = (Resolve-Path -LiteralPath $FfmpegBundle).Path
$assets = Join-Path $PSScriptRoot 'pcrplayer/live'
$lock = Get-Content -LiteralPath (Join-Path $assets 'runtime-lock.json') -Raw | ConvertFrom-Json
foreach ($file in @('PCRPlayer64.exe', 'LICENSE.txt', 'skin')) {
    if (-not (Test-Path -LiteralPath (Join-Path $root $file))) { throw "Missing player input: $file" }
}
foreach ($file in @('ffmpeg.exe', 'ffmpeg-9.0.2.tar.xz', 'COPYING.LGPLv2.1', 'build-config.txt')) {
    if (-not (Test-Path -LiteralPath (Join-Path $ffmpeg $file))) { throw "Missing FFmpeg input: $file" }
}
if ((Get-FileHash -LiteralPath (Join-Path $ffmpeg 'ffmpeg-9.0.2.tar.xz')).Hash.ToLowerInvariant() -cne $lock.ffmpeg.sourceSha256) { throw 'FFmpeg source checksum mismatch.' }
$license = & (Join-Path $ffmpeg 'ffmpeg.exe') -L 2>&1 | Out-String
if ($LASTEXITCODE -ne 0 -or $license -notmatch 'Lesser General Public License' -or $license -match '--enable-gpl|--enable-nonfree') { throw 'Expected the minimal LGPL FFmpeg build.' }
New-Item -ItemType Directory -Path $Cache -Force | Out-Null
$nodePath = if ($NodeBinary) { (Resolve-Path -LiteralPath $NodeBinary).Path } else { Join-Path $Cache 'node.exe' }
if (-not (Test-Path -LiteralPath $nodePath)) { Invoke-WebRequest $lock.node.url -OutFile $nodePath }
if ((Get-FileHash -LiteralPath $nodePath).Hash.ToLowerInvariant() -cne $lock.node.sha256) { throw 'Node binary checksum mismatch.' }
$nodeLicense = Join-Path $Cache 'Node-LICENSE.txt'
if (-not (Test-Path -LiteralPath $nodeLicense)) { Invoke-WebRequest $lock.node.licenseUrl -OutFile $nodeLicense }
$live = Join-Path $root 'Live'
$licenses = Join-Path $root 'licenses'
New-Item -ItemType Directory -Path $live,$licenses -Force | Out-Null
Copy-Item -LiteralPath $nodePath -Destination (Join-Path $live 'node.exe') -Force
Copy-Item -LiteralPath (Join-Path $ffmpeg 'ffmpeg.exe') -Destination (Join-Path $live 'ffmpeg.exe') -Force
Copy-Item -LiteralPath $nodeLicense -Destination (Join-Path $licenses 'Node-LICENSE.txt') -Force
Copy-Item -LiteralPath (Join-Path $ffmpeg 'COPYING.LGPLv2.1') -Destination (Join-Path $licenses 'FFmpeg-LGPL-2.1.txt') -Force
Copy-Item -LiteralPath (Join-Path $ffmpeg 'build-config.txt') -Destination (Join-Path $licenses 'FFmpeg-build-config.txt') -Force
Copy-Item -LiteralPath (Join-Path $assets 'pcr-live-bridge.cjs') -Destination (Join-Path $root 'pcr-live-bridge.cjs') -Force
$ytDlp = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'yt-dlp/lock.json') -Raw | ConvertFrom-Json
@{
    ytDlp = '../YouTube/yt-dlp.exe'
    ffmpeg = 'Live/ffmpeg.exe'
    sha256 = @{ ytDlp = $ytDlp.binary.sha256; ffmpeg = (Get-FileHash -LiteralPath (Join-Path $live 'ffmpeg.exe')).Hash.ToLowerInvariant() }
    maxHeight = 1080
} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $root 'pcr-live-config.json') -Encoding utf8NoBOM
Write-Output "Prepared portable live playback: $root"
