function Get-PeerCastPlayerSourceAssets {
    param(
        [Parameter(Mandatory)][string]$RepoRoot,
        [Parameter(Mandatory)][string]$Version,
        [Parameter(Mandatory)][string]$WorkRoot,
        [Parameter(Mandatory)][string]$LicenseRoot,
        [Parameter(Mandatory)][string]$PublicBase
    )

    # Tie the modified player's corresponding source to the actual packaged EXE.
    $sourceRoot = Join-Path $RepoRoot "target/pcrplayer-source-release-$Version"
    $sourceName = "PCRPlayer-PeerCastRT-$Version-sources.7z"
    $sourceFile = Join-Path $sourceRoot $sourceName
    $manifestPath = Join-Path $sourceRoot 'source-release-manifest.json'
    foreach ($path in @($sourceFile, "$sourceFile.sha256", $manifestPath)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing PCRPlayer source asset: $path" }
    }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $binary = Join-Path $WorkRoot 'package-stage/PCRPlayer/PCRPlayer64.exe'
    $binaryHash = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    $sourceHash = (Get-FileHash -LiteralPath $sourceFile -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($manifest.packagedBinarySha256 -cne $binaryHash -or
        $manifest.sourceArchiveName -cne $sourceName -or
        $manifest.sourceArchiveSha256 -cne $sourceHash -or
        (Get-Content -LiteralPath "$sourceFile.sha256" -Raw).Trim() -cne "$sourceHash  $sourceName") {
        throw 'PCRPlayer binary/source release manifest or checksum mismatch.'
    }
    foreach ($source in @(
        @{ File=$sourceFile; Name=$sourceName; Type='application/x-7z-compressed' },
        @{ File="$sourceFile.sha256"; Name="$sourceName.sha256"; Type='text/plain; charset=utf-8' },
        @{ File=$manifestPath; Name='PCRPlayer-source-release-manifest.json'; Type='application/json' }
    )) {
        @{ Bucket='peercast-rt-downloads'; Base=$PublicBase; Key="releases/$Version/$($source.Name)"; File=$source.File; Type=$source.Type }
    }
    foreach ($notice in Get-ChildItem -LiteralPath (Join-Path $LicenseRoot 'PCRPlayer') -File -ErrorAction Stop) {
        @{ Bucket='peercast-rt-downloads'; Base=$PublicBase; Key="releases/$Version/licenses/PCRPlayer/$($notice.Name)"; File=$notice.FullName; Type='text/plain; charset=utf-8' }
    }
}
