[CmdletBinding()]
param(
    [string] $GccRoot,
    [string] $BashPath,
    [string] $SourceArchive,
    [string] $SevenZipPath,
    [string] $BuildRoot,
    [ValidateRange(1, 64)] [int] $Jobs = 8
)

$ErrorActionPreference = 'Stop'
$expectedSha256 = '8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e'
$scriptRoot = $PSScriptRoot
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptRoot '..\..\..'))

if (-not $SourceArchive) {
    $candidates = @(
        (Join-Path $scriptRoot 'runtime-source\ffmpeg-9.0.2.tar.xz'),
        (Join-Path $scriptRoot 'ffmpeg-9.0.2.tar.xz'),
        (Join-Path $repoRoot 'target\live-runtime-build\runtime-bundle\ffmpeg-9.0.2.tar.xz')
    )
    $SourceArchive = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
if (-not $SourceArchive -or -not (Test-Path -LiteralPath $SourceArchive -PathType Leaf)) {
    throw 'FFmpeg 9.0.2 source archive not found. Bundle it as runtime-source\ffmpeg-9.0.2.tar.xz or pass -SourceArchive.'
}
$SourceArchive = (Resolve-Path -LiteralPath $SourceArchive).Path
$actualSha256 = (Get-FileHash -LiteralPath $SourceArchive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualSha256 -ne $expectedSha256) {
    throw "FFmpeg source SHA-256 mismatch: expected $expectedSha256, got $actualSha256"
}

if (-not $GccRoot) {
    $defaultGcc = 'C:\Program Files\mingw64'
    if (Test-Path -LiteralPath (Join-Path $defaultGcc 'bin\gcc.exe')) { $GccRoot = $defaultGcc }
    else { throw 'Pass -GccRoot pointing to a MinGW-w64 installation with gcc and mingw32-make.' }
}
$GccRoot = [System.IO.Path]::GetFullPath($GccRoot)
foreach ($tool in @('gcc.exe', 'ar.exe', 'ranlib.exe', 'windres.exe', 'mingw32-make.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $GccRoot "bin\$tool") -PathType Leaf)) {
        throw "Required MinGW tool missing: $GccRoot\bin\$tool"
    }
}

if (-not $BashPath) {
    $defaultBash = 'C:\Program Files\Git\bin\bash.exe'
    if (Test-Path -LiteralPath $defaultBash) { $BashPath = $defaultBash }
    else {
        $bashCommand = Get-Command bash.exe -ErrorAction SilentlyContinue
        if ($bashCommand) { $BashPath = $bashCommand.Source }
        else { throw 'Pass -BashPath pointing to Git for Windows bash.exe.' }
    }
}
$BashPath = (Resolve-Path -LiteralPath $BashPath).Path

if (-not $SevenZipPath) {
    $default7z = 'C:\Program Files\7-Zip\7z.exe'
    if (Test-Path -LiteralPath $default7z) { $SevenZipPath = $default7z }
    else {
        $sevenZipCommand = Get-Command 7z.exe -ErrorAction SilentlyContinue
        if ($sevenZipCommand) { $SevenZipPath = $sevenZipCommand.Source }
        else { throw 'Pass -SevenZipPath pointing to 7z.exe for first-time source extraction.' }
    }
}
$SevenZipPath = (Resolve-Path -LiteralPath $SevenZipPath).Path

if (-not $BuildRoot) { $BuildRoot = Join-Path $repoRoot 'target\live-runtime-build\rebuild' }
$BuildRoot = [System.IO.Path]::GetFullPath($BuildRoot)
$sourceDir = Join-Path $BuildRoot 'ffmpeg-9.0.2'
if (-not (Test-Path -LiteralPath (Join-Path $sourceDir 'configure') -PathType Leaf)) {
    New-Item -ItemType Directory -Path $BuildRoot -Force | Out-Null
    $unpackRoot = Join-Path $BuildRoot 'source-unpack'
    $tarRoot = Join-Path $BuildRoot 'xz-unpack'
    New-Item -ItemType Directory -Path $unpackRoot, $tarRoot -Force | Out-Null
    & $SevenZipPath x -y "-o$tarRoot" $SourceArchive | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "7-Zip failed extracting xz (exit $LASTEXITCODE)." }
    $tarArchive = Join-Path $tarRoot 'ffmpeg-9.0.2.tar'
    if (-not (Test-Path -LiteralPath $tarArchive -PathType Leaf)) { throw '7-Zip did not produce ffmpeg-9.0.2.tar.' }
    & $SevenZipPath x -y "-o$unpackRoot" $tarArchive | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "7-Zip failed extracting tar (exit $LASTEXITCODE)." }
    $extracted = Join-Path $unpackRoot 'ffmpeg-9.0.2'
    if (-not (Test-Path -LiteralPath (Join-Path $extracted 'configure') -PathType Leaf)) {
        throw 'The source archive did not contain the expected ffmpeg-9.0.2 source tree.'
    }
    Move-Item -LiteralPath $extracted -Destination $sourceDir
}

# MinGW's windres wrapper mishandles GCC paths with spaces; prefer a short path.
if (-not ('FfmpegBuild.NativePath' -as [type])) {
    Add-Type -Namespace FfmpegBuild -Name NativePath -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode, SetLastError=true)]
public static extern uint GetShortPathName(string longPath, System.Text.StringBuilder shortPath, uint bufferLength);
'@
}
$shortBuffer = [System.Text.StringBuilder]::new(4096)
$shortLength = [FfmpegBuild.NativePath]::GetShortPathName($GccRoot, $shortBuffer, [uint32]$shortBuffer.Capacity)
$gccBin = if ($shortLength -gt 0 -and $shortLength -lt $shortBuffer.Capacity) { Join-Path $shortBuffer.ToString() 'bin' } else { Join-Path $GccRoot 'bin' }

$env:FFMPEG_GCC_BIN = $gccBin
$env:FFMPEG_SOURCE_DIR = $sourceDir
$env:FFMPEG_BUILD_ROOT = $BuildRoot
$env:FFMPEG_BUILD_JOBS = [string]$Jobs
$env:FFMPEG_RECIPE = Join-Path $scriptRoot 'build-ffmpeg.sh'
$command = 'export PATH="$(cygpath -u "$FFMPEG_GCC_BIN"):$PATH"; bash "$(cygpath -u "$FFMPEG_RECIPE")" "$FFMPEG_SOURCE_DIR" "$FFMPEG_BUILD_ROOT" "$FFMPEG_BUILD_JOBS"'
& $BashPath -lc $command
if ($LASTEXITCODE -ne 0) { throw "FFmpeg configure/build failed with exit $LASTEXITCODE." }

$exe = Join-Path $sourceDir 'ffmpeg.exe'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Build finished without executable: $exe" }
Write-Output "Built: $exe"
Write-Output "SHA-256: $((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant())"
