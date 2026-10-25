param(
    [Parameter(Mandatory = $true)][string]$InstallerPath,
    [Parameter(Mandatory = $true)][string]$PackageStage,
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$Version = '1.0.0'
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
foreach ($name in @('LICENSE-GPL-3.0.txt', 'LICENSE_AUDIT.md', 'NotoSansJP-OFL.txt', 'Tabler-Icons-MIT.txt')) {
    Copy-Item -LiteralPath (Join-Path $stageRoot $name) -Destination (Join-Path $licenseRoot $name)
}
Copy-Item -LiteralPath (Join-Path $stageRoot 'PCRPlayer\LICENSE.txt') -Destination (Join-Path $licenseRoot 'PCRPlayer-LICENSE.txt')

$installerHash = (Get-FileHash -LiteralPath $installerAsset -Algorithm SHA256).Hash.ToLowerInvariant()
"$installerHash  $installerName" | Set-Content -LiteralPath (Join-Path $zipRoot 'SHA256SUMS.txt') -Encoding ascii
"$installerHash  $installerName" | Set-Content -LiteralPath "$installerAsset.sha256" -Encoding ascii

$zipName = "PeerCastRT-${Version}-windows-x64-installer.zip"
$zipAsset = Join-Path $outputRoot $zipName
Compress-Archive -LiteralPath (Get-ChildItem -LiteralPath $zipRoot | Select-Object -ExpandProperty FullName) -DestinationPath $zipAsset -CompressionLevel Optimal
$zipHash = (Get-FileHash -LiteralPath $zipAsset -Algorithm SHA256).Hash.ToLowerInvariant()
"$zipHash  $zipName" | Set-Content -LiteralPath "$zipAsset.sha256" -Encoding ascii

Write-Output "Installer SHA256: $installerHash"
Write-Output "ZIP SHA256: $zipHash"
Write-Output "Release assets: $outputRoot"
