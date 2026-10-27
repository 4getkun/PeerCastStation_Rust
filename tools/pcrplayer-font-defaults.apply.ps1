param([Parameter(Mandatory=$true)][string]$PlayerSource)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$workspace = [IO.Path]::GetFullPath((Join-Path $repo 'target\pcrplayer-font-build-20261027'))
$source = [IO.Path]::GetFullPath($PlayerSource)
$allowed = [IO.Path]::GetFullPath((Join-Path $workspace 'source'))
if (-not $source.Equals($allowed, [StringComparison]::OrdinalIgnoreCase)) { throw "Refusing to edit outside the isolated source copy: $source" }
$player = Join-Path $source 'PCRPlayer'
$expectedBefore = @{
    'SerializeDisplay.h'='79c68340a86749b193d269f11c4b2472a5b8f908bcc6061a9b7f6f1449cc987e'
    'Serialize.h'='ad5761c38ffccaa842c89b757b44ba10dad71a99b2692cbe13b63721ccb3f238'
    'MainDlg.cpp'='b34355f74410b28509d6f4e655ec8ae169124c4eb035ee4b2c19ef2a6a71ded1'
    'MainDlgSub.cpp'='d4176d527de5f1519692634c862402fff4fd64691e49e97c0cf6faf977ef586d'
    'Util.h'='29766c8e894a074343386b578fe580d8cc2465772229da7a75eeabc63f363f27'
    'Util.cpp'='2b5b4322ec9bff85e4cb559c151b418fda15f60e383d82a36652ca3e85160da6'
    'SerializeDetail.h'='65ab2b22fa02d33069c0e005454834dff03d10ae10573d9edd9ba9b35fb18246'
    'BBSSkin.h'='d812473a786f581f9add9712dcbcc260b8cafa36bbec5cb8cb1162d97444d406'
    'ThreadDlg.cpp'='aa1e06daf1c8da1e6bbb2ff6920c778653dff6070567ec55073c05fbefd573a5'
    'SearchDlg.cpp'='d96445a978a4fd10ee82c23e73f0061bdbaef615bd4dc120895a4cc98b30bb35'
    'ExtractDlg.cpp'='14ad218c90bfea177598f2f51b9f7d286f4477ef6a109a5b5d4caab512c71b71'
    'PCRPlayer.rc'='403494bd75d57566eaf876a22c313af9b689d3b7bf278322c371b9ceaaff3b84'
}
foreach ($name in $expectedBefore.Keys) {
    $path = Join-Path $player $name
    if (-not (Test-Path -LiteralPath $path)) { throw "Missing source file: $name" }
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne $expectedBefore[$name]) { throw "Unexpected pre-edit SHA-256 for ${name}: $hash" }
}
if (Test-Path -LiteralPath (Join-Path $player 'CaptionFont.h')) { throw 'CaptionFont.h already exists in the isolated copy.' }
function Replace-Bytes([string]$file, [string]$old, [string]$new, [Text.Encoding]$encoding, [int]$expectedCount) {
    $path = Join-Path $player $file
    $bytes = [IO.File]::ReadAllBytes($path)
    $from = $encoding.GetBytes($old)
    $to = $encoding.GetBytes($new)
    $stream = [IO.MemoryStream]::new()
    $count = 0
    for ($i = 0; $i -lt $bytes.Length;) {
        $match = $i + $from.Length -le $bytes.Length
        if ($match) { for ($j = 0; $j -lt $from.Length; $j++) { if ($bytes[$i + $j] -ne $from[$j]) { $match = $false; break } } }
        if ($match) { $stream.Write($to, 0, $to.Length); $i += $from.Length; $count++ }
        else { $stream.WriteByte($bytes[$i]); $i++ }
    }
    if ($count -ne $expectedCount) { throw "Expected $expectedCount byte replacements in ${file}, found $count" }
    [IO.File]::WriteAllBytes($path, $stream.ToArray())
}
$ascii = [Text.Encoding]::ASCII
$sjis = [Text.Encoding]::GetEncoding(932)
$utf16 = [Text.Encoding]::Unicode
Replace-Bytes 'Util.h' 'int point = 90, const std::wstring& name = L"MS UI Gothic"' 'int point = 100, const std::wstring& name = L"Yu Gothic UI"' $ascii 1
$oldFont = @"
bool createFont(CFont& font, bool bold, bool underline, int point, const std::wstring& name)
{
	if (font.GetSafeHandle()) { font.DeleteObject(); }

	if (font.CreatePointFont(point, name.c_str()))
	{
		if (bold || underline)
		{
			LOGFONT lf = {};
			if (font.GetLogFont(&lf))
			{
				if (bold) { lf.lfWeight = FW_BOLD; }
				if (underline) { lf.lfUnderline = TRUE; }
				font.DeleteObject();
				font.CreateFontIndirect(&lf);
			}
		}
		return true;
	}
	return false;
"@
$newFont = @"
bool createFont(CFont& font, bool bold, bool underline, int point, const std::wstring& name)
{
	if (font.GetSafeHandle()) { font.DeleteObject(); }

	if (font.CreatePointFont(point, name.c_str()))
	{
		LOGFONT lf = {};
		if (font.GetLogFont(&lf))
		{
			if (bold) { lf.lfWeight = FW_BOLD; }
			if (underline) { lf.lfUnderline = TRUE; }
			lf.lfQuality = CLEARTYPE_QUALITY;
			font.DeleteObject();
			font.CreateFontIndirect(&lf);
		}
		return true;
	}
	return false;
"@
Replace-Bytes 'Util.cpp' $oldFont $newFont $sjis 1
Replace-Bytes 'SerializeDetail.h' ': name(L"MS UI Gothic")' ': name(L"Yu Gothic UI")' $ascii 1
Replace-Bytes 'SerializeDetail.h' ', point(9)' ', point(10)' $ascii 1
Replace-Bytes 'SerializeDetail.h' "cf.GetLogFont(&lf);`r`n`t`t`tcf.DeleteObject();" "cf.GetLogFont(&lf);`r`n`t`t`tlf.lfQuality = CLEARTYPE_QUALITY;`r`n`t`t`tcf.DeleteObject();" $ascii 1
Replace-Bytes 'BBSSkin.h' ': name(L"ＭＳ Ｐゴシック")' ': name(L"Yu Gothic UI")' $sjis 1
Replace-Bytes 'BBSSkin.h' ', point(9)' ', point(10)' $ascii 1
Replace-Bytes 'BBSSkin.h' ', pixel(12)' ', pixel(13)' $ascii 1
Replace-Bytes 'BBSSkin.h' "cf.GetLogFont(&lf);`r`n`t`t`tcf.DeleteObject();" "cf.GetLogFont(&lf);`r`n`t`t`tlf.lfQuality = CLEARTYPE_QUALITY;`r`n`t`t`tcf.DeleteObject();" $ascii 1
foreach ($name in @('ThreadDlg.cpp', 'SearchDlg.cpp', 'ExtractDlg.cpp')) {
    Replace-Bytes $name 'font_.CreatePointFont(point * 10, name.c_str());' 'utl::createFont(font_, false, false, point * 10, name);' $ascii 1
}
Replace-Bytes 'PCRPlayer.rc' 'FONT 9, "MS UI Gothic"' 'FONT 9, "Yu Gothic UI"' $utf16 2
Replace-Bytes 'PCRPlayer.rc' 'FONT 8, "MS Shell Dlg"' 'FONT 9, "Yu Gothic UI"' $utf16 65
# Update only the bottom-bar defaults, preserving the original source bytes.
$latin1 = [Text.Encoding]::GetEncoding(28591)
foreach ($config in @('StatusConfig', 'SeekConfig')) {
    $text = $latin1.GetString([IO.File]::ReadAllBytes((Join-Path $player 'SerializeDisplay.h')))
    $start = $text.IndexOf("struct ${config} {")
    $end = $text.IndexOf('LOGFONT font;', $start)
    if ($start -lt 0 -or $end -le $start) { throw "Cannot locate $config defaults." }
    $old = $text.Substring($start, $end - $start)
    $new = $old.Replace(', marginY(2)', ', marginY(3)').Replace('utl::createFont(f)', 'utl::createFont(f, true, false, 110)').Replace('f.GetLogFont(&font);', "f.GetLogFont(&font);`r`n`t`t`tfont.lfWeight = 600;")
    Replace-Bytes 'SerializeDisplay.h' $old $new $latin1 1
}
Replace-Bytes 'Serialize.h' ', edit(false)' ', edit(true)' $ascii 1
Replace-Bytes 'MainDlg.cpp' '#include "stdafx.h"' "#include `"stdafx.h`"`r`n#include `"CaptionFont.h`"" $ascii 1
Replace-Bytes 'MainDlg.cpp' "`tApplySubpane();" "`tApplySubpane();`r`n`tplayer_caption::Install(m_hWnd, gl_.value.window.show.frame);" $ascii 1
Replace-Bytes 'MainDlgSub.cpp' '#include "stdafx.h"' "#include `"stdafx.h`"`r`n#include `"CaptionFont.h`"" $ascii 1
$layout = "void CMainDlg::SizeHelper()`r`n{`r`n`tCRect rc;`r`n`tGetClientRect(&rc);"
$newLayout = $layout.Replace("()`r`n{`r`n", "()`r`n{`r`n`tplayer_caption::SetFrameVisible(m_hWnd, gl_.value.window.show.frame);`r`n")
Replace-Bytes 'MainDlgSub.cpp' $layout ($newLayout + "`r`n`tplayer_caption::AdjustContent(m_hWnd, rc);") $ascii 1
$captionHeader = Join-Path $PSScriptRoot 'pcrplayer/CaptionFont.h'
# Git checkouts/source archives may use LF; keep the pinned C++ output bytes stable.
$captionText = [IO.File]::ReadAllText($captionHeader).Replace("`r`n", "`n").Replace("`n", "`r`n")
[IO.File]::WriteAllText((Join-Path $player 'CaptionFont.h'), $captionText, [Text.UTF8Encoding]::new($false))

$expectedAfter = @{
    'CaptionFont.h'='3349f4202a38957c826e6f9b6fe84cf09b8dd1484f7d2660723848ec7d5eafc3'
    'SerializeDisplay.h'='1bf53721b7e4d3dfd563692f5d9a00c0734547cb49f5af82ba81d55d55b1070a'
    'Serialize.h'='ba93ea2b80d1e341496981db003d914da7ab734ada67bc9a4f5384ca0dd264e9'
    'MainDlg.cpp'='c5086f6eabbac9178ac8831e2ef3b589529c4cf8eb7d3a91a9cf9225ed487d63'
    'MainDlgSub.cpp'='bab7035be3bc6ebbf3c46c5c154bf51bcb5c0843df1af8d8feb339ea0b33af01'
    'Util.h'='809c734aa87c3f67090819879a26ebd401f8fce7f06c5b0815036feb908ab089'
    'Util.cpp'='d63b9f34c468284afb28cb8385569631ba101800c34f978d3cbbebe405199d83'
    'SerializeDetail.h'='e43b7a0a96bc059842f7d995115ea08b3872ad19e17b36f059c203d90f9bc0b5'
    'BBSSkin.h'='24865403be76e76857b0e654dca4467438e95a9a3e69c303943bfd8b4dfced43'
    'ThreadDlg.cpp'='e38dd3d3a643ace23ba3b59e51ecb67596d4b851c7b03cc8f8d7887c1d1775a2'
    'SearchDlg.cpp'='4c9106f2afe9505e48c765ff3267b0ddfd946b05f68cc7da55efde5c8cb7d49c'
    'ExtractDlg.cpp'='edf31d09112f2d33362124b6fd1a035568c3c1dbcc2de00119e23bb2453ae4d1'
    'PCRPlayer.rc'='3128b50c8317be4d96c8d02af7cdf0f053947a477fb3417f32a1b64ed9816267'
}
foreach ($name in $expectedAfter.Keys) {
    $hash = (Get-FileHash -LiteralPath (Join-Path $player $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne $expectedAfter[$name]) { throw "Unexpected post-edit SHA-256 for ${name}: $hash" }
    "${name} $hash"
}
