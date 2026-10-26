param(
    [Parameter(Mandatory = $true)][string]$InstallerPath,
    [Parameter(Mandatory = $true)][string]$PackageStage,
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$Version = '1.1.1'
)

$ErrorActionPreference = 'Stop'
$installerSource = (Resolve-Path -LiteralPath $InstallerPath).Path
$stageRoot = (Resolve-Path -LiteralPath $PackageStage).Path
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $outputRoot) { throw "Release asset directory already exists: $outputRoot" }

$zipRoot = Join-Path $outputRoot 'zip-root'
$licenseRoot = Join-Path $zipRoot 'licenses'
New-Item -ItemType Directory -Path $licenseRoot -Force | Out-Null

$installerName = "PeercastRT_${Version}_x64-setup.exe"
$installerAsset = Join-Path $outputRoot $installerName
Copy-Item -LiteralPath $installerSource -Destination $installerAsset
Copy-Item -LiteralPath $installerAsset -Destination (Join-Path $zipRoot $installerName)

foreach ($name in @('README-JA.txt', 'SOURCE.txt', 'player-bundle.json')) {
    Copy-Item -LiteralPath (Join-Path $stageRoot $name) -Destination (Join-Path $zipRoot $name)
}
@(
    "PeerCast RT $Version for Windows x64"
    ''
    "このフォルダー内の $installerName を実行してインストールしてください。"
    'PCRPlayer はセットアップに同梱されています。'
    'ライセンス文書は licenses フォルダーにまとめています。'
    '更新時は既存の設定と録画ファイルを保持します。'
    ''
    'ソースコード: https://github.com/4getkun/PeerCastStation_Rust'
) | Set-Content -LiteralPath (Join-Path $zipRoot 'README-JA.txt') -Encoding utf8
foreach ($name in @('LICENSE-GPL-3.0.txt', 'LICENSE_AUDIT.md', 'NotoSansJP-OFL.txt', 'Tabler-Icons-MIT.txt')) {
    Copy-Item -LiteralPath (Join-Path $stageRoot $name) -Destination (Join-Path $licenseRoot $name)
}
Copy-Item -LiteralPath (Join-Path $stageRoot 'PCRPlayer\LICENSE.txt') -Destination (Join-Path $licenseRoot 'PCRPlayer-LICENSE.txt')
Copy-Item -LiteralPath (Join-Path $stageRoot 'licenses/yt-dlp') -Destination $licenseRoot -Recurse

$installerHash = (Get-FileHash -LiteralPath $installerAsset -Algorithm SHA256).Hash.ToLowerInvariant()
"$installerHash  $installerName" | Set-Content -LiteralPath (Join-Path $zipRoot 'SHA256SUMS.txt') -Encoding ascii
"$installerHash  $installerName" | Set-Content -LiteralPath "$installerAsset.sha256" -Encoding ascii

$zipName = "PeerCastRT-${Version}-windows-x64-installer.7z"
$zipAsset = Join-Path $outputRoot $zipName
$sevenZip = 'C:\Program Files\7-Zip\7z.exe'
if (-not (Test-Path -LiteralPath $sevenZip)) { throw '7-Zip was not found.' }
Push-Location $zipRoot
try {
    & $sevenZip a -t7z -mx=7 -mmt=4 $zipAsset '.\*'
    if ($LASTEXITCODE -ne 0) { throw '7z release packaging failed.' }
    & $sevenZip t $zipAsset
    if ($LASTEXITCODE -ne 0) { throw '7z release integrity check failed.' }
}
finally { Pop-Location }
$zipHash = (Get-FileHash -LiteralPath $zipAsset -Algorithm SHA256).Hash.ToLowerInvariant()
"$zipHash  $zipName" | Set-Content -LiteralPath "$zipAsset.sha256" -Encoding ascii

Write-Output "Installer SHA256: $installerHash"
Write-Output "7z SHA256: $zipHash"
Write-Output "Release assets: $outputRoot"
