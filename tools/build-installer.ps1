param(
    [Parameter(Mandatory = $true)][string]$PlayerArchive,
    [string]$Version = '1.0.3',
    [string]$CliVersion = '2.12.4',
    [string]$WorkDirectory = '',
    [switch]$SkipPackageBuild
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-updater-artifacts.ps1')
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$archivePath = (Resolve-Path -LiteralPath $PlayerArchive).Path
$sevenZip = 'C:\Program Files\7-Zip\7z.exe'
if (-not (Test-Path -LiteralPath $sevenZip)) { throw '7-Zip was not found.' }

$workRoot = if ([string]::IsNullOrWhiteSpace($WorkDirectory)) {
    Join-Path $repoRoot "target\release-$Version"
} else {
    [IO.Path]::GetFullPath($WorkDirectory)
}
$playerSource = Join-Path $workRoot 'player-source'
$packageStage = Join-Path $workRoot 'package-stage'
$configPath = Join-Path $workRoot 'tauri.release.conf.json'
foreach ($path in @($workRoot, $playerSource, $packageStage, $configPath)) {
    if (Test-Path -LiteralPath $path) { throw "Release path already exists: $path" }
}
New-Item -ItemType Directory -Path $playerSource -Force | Out-Null

& $sevenZip x -y "-o$playerSource" $archivePath
if ($LASTEXITCODE -ne 0) { throw 'PCRPlayer extraction failed.' }

& (Join-Path $PSScriptRoot 'package-desktop.ps1') -PlayerSource $playerSource -Destination $packageStage -Profile release -SkipBuild:$SkipPackageBuild
if ($LASTEXITCODE -ne 0) { throw 'Desktop packaging failed.' }

$resources = [ordered]@{
    (Join-Path $packageStage 'PCRPlayer') = 'PCRPlayer/'
    (Join-Path $packageStage 'player-bundle.json') = 'player-bundle.json'
    (Join-Path $packageStage 'LICENSE-GPL-3.0.txt') = 'LICENSE-GPL-3.0.txt'
    (Join-Path $packageStage 'LICENSE_AUDIT.md') = 'LICENSE_AUDIT.md'
    (Join-Path $packageStage 'NotoSansJP-OFL.txt') = 'NotoSansJP-OFL.txt'
    (Join-Path $packageStage 'Tabler-Icons-MIT.txt') = 'Tabler-Icons-MIT.txt'
    (Join-Path $packageStage 'README-JA.txt') = 'README-JA.txt'
    (Join-Path $packageStage 'SOURCE.txt') = 'SOURCE.txt'
}
$config = [ordered]@{
    bundle = [ordered]@{
        active = $true
        createUpdaterArtifacts = $true
        targets = @('nsis')
        useLocalToolsDir = $true
        publisher = '4getkun'
        shortDescription = 'P2P video streaming and viewing application for Windows'
        license = 'GPL-3.0-or-later'
        licenseFile = (Join-Path $packageStage 'LICENSE-GPL-3.0.txt')
        resources = $resources
        windows = [ordered]@{
            allowDowngrades = $false
            bundleVCRuntime = $true
            nsis = [ordered]@{
                compression = 'lzma'
                installMode = 'currentUser'
                languages = @('Japanese', 'English')
                displayLanguageSelector = $false
                template = (Join-Path $repoRoot 'crates\peercast-desktop\nsis\installer.nsi')
                installerIcon = (Join-Path $repoRoot 'crates\peercast-desktop\icons\icon.ico')
                headerImage = (Join-Path $repoRoot 'crates\peercast-desktop\icons\installer-header.bmp')
                sidebarImage = (Join-Path $repoRoot 'crates\peercast-desktop\icons\installer-sidebar.bmp')
                installerHooks = (Join-Path $repoRoot 'crates\peercast-desktop\icons\installer-assets.nsh')
                startMenuFolder = 'PeerCast RT'
            }
        }
    }
}
$config | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $configPath -Encoding utf8

Push-Location (Join-Path $repoRoot 'crates\peercast-desktop')
try {
    $env:CARGO_BUILD_JOBS = '2'
    $tauri = Get-Content -LiteralPath (Join-Path $repoRoot 'crates\peercast-desktop\tauri.conf.json') -Raw | ConvertFrom-Json
    Invoke-PeerCastTauriBuild -ConfiguredPublicKey ([string]$tauri.plugins.updater.pubkey) -BuildAction {
        & npx.cmd --yes "@tauri-apps/cli@$CliVersion" build --bundles nsis --ci --config $configPath
        if ($LASTEXITCODE -ne 0) { throw 'Tauri NSIS build failed.' }
    }
}
finally {
    Pop-Location
}

$installer = Join-Path $repoRoot "target\release\bundle\nsis\PeerCast RT_${Version}_x64-setup.exe"
if (-not (Test-Path -LiteralPath $installer)) { throw "Installer was not produced: $installer" }
$hash = Get-FileHash -LiteralPath $installer -Algorithm SHA256
Write-Output "Installer: $installer"
Write-Output "SHA256: $($hash.Hash)"
