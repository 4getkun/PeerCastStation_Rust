param([string]$Version = '1.1.1', [string]$RenderedScript = '')
$ErrorActionPreference = 'Stop'
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$renderedPath = if ($RenderedScript) { (Resolve-Path -LiteralPath $RenderedScript).Path } else { Join-Path $repoRoot 'target\release\nsis\x64\installer.nsi' }
$source = Get-Content -LiteralPath $renderedPath -Raw
if ($source -notmatch ('!define VERSION "' + [regex]::Escape($Version) + '"')) {
    throw 'Bundle the current version before generating a layout preview.'
}
# Use the exact rendered UI and payload resources, but replace installation
# sections and final launch/shortcut actions with a side-effect-free preview.
$source = [regex]::Replace($source, '(?ms)^Section (EarlyChecks|WebView2|Install)\r?\n.*?^SectionEnd', '')
$source = [regex]::Replace($source, '(?ms)^Function PageReinstall\r?\n.*?^FunctionEnd', "Function PageReinstall`n  Abort`nFunctionEnd")
$source = [regex]::Replace($source, '(?ms)^Function PageFinishLeave\r?\n.*?^FunctionEnd', "Function PageFinishLeave`nFunctionEnd")
$source = [regex]::Replace($source, '(?ms)^Function \.onInstSuccess\r?\n.*?^FunctionEnd', "Function .onInstSuccess`nFunctionEnd")
$source = [regex]::Replace($source, '(?ms)^Section Uninstall\r?\n.*?^SectionEnd', "Section Uninstall`n  DetailPrint `"Layout preview - no application files or settings are removed.`"`n  Sleep 10000`nSectionEnd")
$previewExe = Join-Path $repoRoot 'target\installer-ui-preview.exe'
$source = [regex]::Replace($source, '(?m)^!define OUTFILE .*$', ('!define OUTFILE "' + $previewExe + '"'))
$source += "`nSection LayoutPreview`n  WriteUninstaller `"`$EXEDIR\uninstaller-ui-preview.exe`"`n  DetailPrint `"Layout preview - no application files or registry entries are installed.`"`n  Sleep 10000`n  StrCpy `$InstallSucceeded 1`nSectionEnd`n"
$previewScript = Join-Path (Split-Path $renderedPath) 'installer-ui-preview.nsi'
[IO.File]::WriteAllText($previewScript, $source, [Text.UTF8Encoding]::new($true))
& (Join-Path $repoRoot 'target\.tauri\NSIS\makensis.exe') /V2 $previewScript
if ($LASTEXITCODE -ne 0) { throw 'Layout preview compilation failed.' }
Write-Output "Layout preview: $previewExe"
