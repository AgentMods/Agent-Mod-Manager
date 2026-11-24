[CmdletBinding()]
param(
    [string] $NsisExecutable
)

$ErrorActionPreference = 'Stop'

$installerDirectory = $PSScriptRoot
$configurationPath = Join-Path $installerDirectory '..\tauri.conf.json'
$localeIncludePath = Join-Path $installerDirectory 'locales.nsh'
$localeSource = [System.IO.File]::ReadAllText((Resolve-Path -LiteralPath $localeIncludePath))
$configuration = Get-Content -Raw -LiteralPath $configurationPath | ConvertFrom-Json

$locales = @(
    [pscustomobject]@{ Code = 'en'; Name = 'English' }
    [pscustomobject]@{ Code = 'zh-CN'; Name = 'SimpChinese' }
    [pscustomobject]@{ Code = 'ja'; Name = 'Japanese' }
    [pscustomobject]@{ Code = 'ko'; Name = 'Korean' }
    [pscustomobject]@{ Code = 'de'; Name = 'German' }
    [pscustomobject]@{ Code = 'fr'; Name = 'French' }
    [pscustomobject]@{ Code = 'es'; Name = 'Spanish' }
    [pscustomobject]@{ Code = 'pt-BR'; Name = 'PortugueseBR' }
    [pscustomobject]@{ Code = 'ru'; Name = 'Russian' }
    [pscustomobject]@{ Code = 'it'; Name = 'Italian' }
)

$configuredLanguages = @($configuration.bundle.windows.nsis.languages)
$expectedLanguages = @($locales | ForEach-Object { $_.Name })
if (($configuredLanguages -join "`n") -cne ($expectedLanguages -join "`n")) {
    throw "NSIS languages must be in this order: $($expectedLanguages -join ', ')"
}
if ($configuration.bundle.windows.nsis.displayLanguageSelector -ne $false) {
    throw 'NSIS must select the language automatically from Windows.'
}

if (-not $NsisExecutable) {
    $command = Get-Command makensis.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($command) {
        $NsisExecutable = $command.Source
    }
    else {
        $NsisExecutable = Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe'
    }
}
if (-not (Test-Path -LiteralPath $NsisExecutable -PathType Leaf)) {
    throw "NSIS compiler not found: $NsisExecutable"
}
$NsisExecutable = (Resolve-Path -LiteralPath $NsisExecutable).Path
$languageDirectory = Join-Path (Split-Path -Parent $NsisExecutable) 'Contrib\Language files'

$languageIds = @{}
foreach ($locale in $locales) {
    $languageFile = Join-Path $languageDirectory ($locale.Name + '.nlf')
    if (-not (Test-Path -LiteralPath $languageFile -PathType Leaf)) {
        throw "NSIS language definition missing for $($locale.Code): $languageFile"
    }
    $languageLines = Get-Content -LiteralPath $languageFile
    $languageId = $null
    for ($index = 0; $index -lt ($languageLines.Count - 1); $index++) {
        if ($languageLines[$index].Trim() -ceq '# Language ID' -and
            $languageLines[$index + 1].Trim() -match '^\d+$') {
            $languageId = [int] $languageLines[$index + 1].Trim()
            break
        }
    }
    if ($null -eq $languageId) {
        throw "Could not read the NSIS language ID from $languageFile"
    }
    $languageIds[$locale.Name] = $languageId
}

$languageNameById = @{}
foreach ($locale in $locales) {
    $languageNameById[[int] $languageIds[$locale.Name]] = $locale.Name
}

$translations = @{}
$stringMatches = [regex]::Matches(
    $localeSource,
    '(?m)^\s*LangString\s+(?<key>AmmWelcomeKicker|AmmWelcomeBody)\s+(?<id>\d+)\s+"(?<value>[^"]*)"\s*$'
)
foreach ($match in $stringMatches) {
    $languageId = [int] $match.Groups['id'].Value
    if (-not $languageNameById.ContainsKey($languageId)) {
        throw "Unexpected NSIS language ID $languageId in locales.nsh"
    }
    $key = "$($languageNameById[$languageId])/$($match.Groups['key'].Value)"
    if ($translations.ContainsKey($key)) {
        throw "Duplicate NSIS translation: $key"
    }
    $translations[$key] = $match.Groups['value'].Value
}

$expectedTranslationCount = $locales.Count * 2
if ($translations.Count -ne $expectedTranslationCount) {
    throw "Expected $expectedTranslationCount welcome strings, found $($translations.Count)."
}

$fonts = @{}
$fontMatches = [regex]::Matches(
    $localeSource,
    '(?m)^\s*SetFont\s+/LANG=(?<id>\d+)\s+"(?<font>[^"]+)"\s+\d+\s*$'
)
foreach ($match in $fontMatches) {
    $languageId = [int] $match.Groups['id'].Value
    if (-not $languageNameById.ContainsKey($languageId)) {
        throw "Unexpected NSIS font language ID $languageId in locales.nsh"
    }
    if ($fonts.ContainsKey($languageId)) {
        throw "Duplicate NSIS font declaration for language ID $languageId"
    }
    $fonts[$languageId] = $match.Groups['font'].Value
}
if ($fonts.Count -ne $locales.Count) {
    throw "Expected fonts for all $($locales.Count) locales, found $($fonts.Count)."
}

$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$tempDirectory = Join-Path $tempRoot ('amm-nsis-locales-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $tempDirectory
$passed = $false
try {
    foreach ($locale in $locales) {
        $languageId = $languageIds[$locale.Name]
        $testIncludePath = Join-Path $tempDirectory ("locales-$($locale.Code).nsh")
        $testIncludeLines = [System.Collections.Generic.List[string]]::new()
        $testIncludeLines.Add(('SetFont /LANG={0} "{1}" 9' -f $languageId, $fonts[$languageId]))
        foreach ($key in @('AmmWelcomeKicker', 'AmmWelcomeBody')) {
            $value = $translations["$($locale.Name)/$key"]
            if ($value.Contains('$') -or $value.Contains('"') -or $value.Contains("`r") -or $value.Contains("`n")) {
                throw "Translation cannot be represented safely in the NSIS verifier: $($locale.Name)/$key"
            }
            $testIncludeLines.Add(('LangString {0} {1} "{2}"' -f $key, $languageId, $value))
        }
        [System.IO.File]::WriteAllLines($testIncludePath, $testIncludeLines.ToArray(), [System.Text.UTF8Encoding]::new($true))

        $scriptLines = [System.Collections.Generic.List[string]]::new()
        foreach ($line in @(
            'Unicode true'
            'Name "Agent Mod Manager locale verifier"'
            ('OutFile "locale-verifier-{0}.exe"' -f $locale.Code)
            'SilentInstall silent'
            'RequestExecutionLevel user'
            '!include "MUI2.nsh"'
            ('!include "' + $testIncludePath + '"')
            '!insertmacro MUI_PAGE_WELCOME'
            ('!insertmacro MUI_LANGUAGE "{0}"' -f $locale.Name)
            'Section "Verify localized strings"'
        )) {
            $scriptLines.Add($line)
        }
        foreach ($key in @('AmmWelcomeKicker', 'AmmWelcomeBody')) {
            $value = $translations["$($locale.Name)/$key"]
            $scriptLines.Add(('  StrCmp "$({0})" "{1}" 0 locale_fail' -f $key, $value))
        }
        $scriptLines.Add(('  FileOpen $0 "$EXEDIR\locale-{0}.pass" w' -f $locale.Code))
        $scriptLines.Add('  FileWrite $0 "PASS"')
        $scriptLines.Add('  FileClose $0')
        $scriptLines.Add('  SetErrorLevel 0')
        $scriptLines.Add('  Goto locale_done')
        $scriptLines.Add('locale_fail:')
        $scriptLines.Add('  SetErrorLevel 1')
        $scriptLines.Add('  Abort')
        $scriptLines.Add('locale_done:')
        $scriptLines.Add('SectionEnd')

        $sourcePath = Join-Path $tempDirectory ("locale-verifier-$($locale.Code).nsi")
        [System.IO.File]::WriteAllLines($sourcePath, $scriptLines.ToArray(), [System.Text.UTF8Encoding]::new($true))

        Push-Location $tempDirectory
        try {
            $compilerOutput = & $NsisExecutable /V2 $sourcePath 2>&1
            $compilerExitCode = $LASTEXITCODE
        }
        finally {
            Pop-Location
        }
        $compilerOutput | ForEach-Object { Write-Output $_ }
        if ($compilerExitCode -ne 0) {
            throw "makensis failed for locale $($locale.Code) with exit code $compilerExitCode."
        }

        $verifierPath = Join-Path $tempDirectory ("locale-verifier-$($locale.Code).exe")
        $process = Start-Process -FilePath $verifierPath -ArgumentList '/S' -WorkingDirectory $tempDirectory -WindowStyle Hidden -Wait -PassThru
        if ($process.ExitCode -ne 0) {
            throw "Compiled NSIS string mismatch for locale $($locale.Code)."
        }
        $passMarker = Join-Path $tempDirectory ("locale-$($locale.Code).pass")
        if (-not (Test-Path -LiteralPath $passMarker -PathType Leaf) -or
            [System.IO.File]::ReadAllText($passMarker) -cne 'PASS') {
            throw "Compiled NSIS locale verifier did not report success for $($locale.Code)."
        }
        Write-Output "PASS: compiled NSIS strings matched their source for $($locale.Code) ($($locale.Name))."
    }
    $passed = $true
    Write-Output 'PASS: automatic Windows language selection and English-first fallback are configured.'
}
finally {
    if ($passed -and (Test-Path -LiteralPath $tempDirectory -PathType Container)) {
        Get-ChildItem -LiteralPath $tempDirectory -File | Remove-Item -Force
        Remove-Item -LiteralPath $tempDirectory
    }
}
