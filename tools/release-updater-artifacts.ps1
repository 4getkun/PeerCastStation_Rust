function Get-UpdaterSignatureVersion {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$SignatureText)

    try {
        $document = [Text.UTF8Encoding]::new($false, $true).GetString([Convert]::FromBase64String($SignatureText.Trim()))
    }
    catch { throw 'Updater signature is not valid base64-encoded UTF-8.' }
    $comment = @($document -split "`r?`n" | Where-Object { $_.StartsWith('trusted comment: ', [StringComparison]::Ordinal) })
    if ($comment.Count -ne 1) { throw 'Updater signature must contain exactly one trusted comment.' }
    $version = @($comment[0].Substring('trusted comment: '.Length) -split "`t" | Where-Object { $_.StartsWith('version:', [StringComparison]::Ordinal) })
    if ($version.Count -ne 1 -or $version[0].Length -le 'version:'.Length) { throw 'Updater signature must contain exactly one version field.' }
    $version[0].Substring('version:'.Length)
}

function Assert-UpdaterSignatureVersion {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$SignatureText, [Parameter(Mandatory)][string]$ExpectedVersion)
    $actual = Get-UpdaterSignatureVersion -SignatureText $SignatureText
    if ($actual -cne $ExpectedVersion) { throw "Updater signature version '$actual' does not match '$ExpectedVersion'." }
    $actual
}

function Invoke-PeerCastTauriBuild {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][scriptblock]$BuildAction,
        [Parameter(Mandatory)][string]$ConfiguredPublicKey,
        [string]$SigningDirectory = (Join-Path $env:USERPROFILE '.tauri')
    )

    $privateName = 'TAURI_SIGNING_PRIVATE_KEY'
    $passwordName = 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD'
    $existingKey = [Environment]::GetEnvironmentVariable($privateName, 'Process')
    if (-not [string]::IsNullOrWhiteSpace($existingKey)) {
        & $BuildAction
        return
    }

    $keyPath = Join-Path $SigningDirectory 'peercast-rt.key'
    $publicPath = "$keyPath.pub"
    $passwordPath = Join-Path $SigningDirectory 'peercast-rt-password.xml'
    foreach ($path in @($keyPath, $publicPath)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Updater signing file is missing: $path" }
    }
    $filePublicKey = (Get-Content -LiteralPath $publicPath -Raw).Trim()
    if ([string]::IsNullOrWhiteSpace($ConfiguredPublicKey) -or $filePublicKey -cne $ConfiguredPublicKey.Trim()) {
        throw 'Updater signing public key does not match tauri.conf.json.'
    }

    $oldKey = [Environment]::GetEnvironmentVariable($privateName, 'Process')
    $oldPassword = [Environment]::GetEnvironmentVariable($passwordName, 'Process')
    $securePassword = $null
    $bstr = [IntPtr]::Zero
    try {
        $password = ''
        if (Test-Path -LiteralPath $passwordPath -PathType Leaf) {
            $securePassword = Import-Clixml -LiteralPath $passwordPath
            if ($securePassword -isnot [Security.SecureString]) { throw 'Updater password file is not a DPAPI SecureString.' }
            $bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePassword)
            $password = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr)
        }
        [Environment]::SetEnvironmentVariable($privateName, $keyPath, 'Process')
        [Environment]::SetEnvironmentVariable($passwordName, $password, 'Process')
        & $BuildAction
    }
    finally {
        [Environment]::SetEnvironmentVariable($privateName, $oldKey, 'Process')
        [Environment]::SetEnvironmentVariable($passwordName, $oldPassword, 'Process')
        if ($bstr -ne [IntPtr]::Zero) { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr) }
        if ($securePassword) { $securePassword.Dispose() }
    }
}
