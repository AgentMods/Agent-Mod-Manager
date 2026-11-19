#Requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter(Mandatory)] [ValidateSet('nsis', 'msi')] [string] $Kind,
    [Parameter(Mandatory)] [string] $Installer,
    [Parameter(Mandatory)] [string] $EvidenceDir,
    [switch] $DisposableMachine
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $IsWindows -or -not $DisposableMachine) {
    throw 'Run with PowerShell 7 on a disposable Windows machine and profile only. Pass -DisposableMachine to confirm.'
}

if ($Kind -eq 'msi') {
    $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'The Tauri MSI is per-machine. Run from elevated PowerShell in the same disposable profile.'
    }
}

function Get-ExistingItem([string] $Path) {
    try {
        Get-Item -LiteralPath $Path -Force
    } catch [System.Management.Automation.ItemNotFoundException] {
        return
    }
}

function Assert-Missing([string] $Path) {
    if (Get-ExistingItem $Path) {
        throw "Refusing to use an existing path: $Path"
    }
}

function Assert-LocalUnlinkedPath([string] $Path) {
    if ($Path -notmatch '^[A-Za-z]:\\[^:]*$') {
        throw "Expected an absolute local drive path: $Path"
    }
    for ($current = $Path; $null -ne $current; $current = [IO.Path]::GetDirectoryName($current)) {
        $item = Get-ExistingItem $current
        if ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Refusing to traverse a reparse point: $current"
        }
    }
}

function Assert-FreshProfiles {
    Assert-Missing $appData
    Assert-Missing $webviewData
}

function Assert-FreshInstall([string] $PackagePath) {
    # The stock NSIS can uninstall a matching WiX product, and WiX can perform major upgrades.
    $arpPath = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall'
    $productPath = 'SOFTWARE\agentmodmanager\Mod Manager'
    $edgePath = 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    $views = @('Registry32')
    if ([Environment]::Is64BitOperatingSystem) { $views += 'Registry64' }
    $webviewVersion = $null

    foreach ($hive in @('CurrentUser', 'LocalMachine')) {
        foreach ($view in $views) {
            $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, $view)
            try {
                $keys = @($base.OpenSubKey($productPath), $base.OpenSubKey("$arpPath\Mod Manager"))
                $previous = $false
                foreach ($key in $keys) {
                    if ($key) { $key.Dispose(); $previous = $true }
                }
                if ($previous) {
                    throw "Existing Mod Manager install metadata in $hive ($view); use a fresh machine/profile."
                }

                $arp = $base.OpenSubKey($arpPath)
                if ($arp) {
                    try {
                        foreach ($name in $arp.GetSubKeyNames()) {
                            $entry = $arp.OpenSubKey($name)
                            if (-not $entry) { continue }
                            try {
                                if ($entry.GetValue('DisplayName') -eq 'Mod Manager') {
                                    throw "Existing Mod Manager uninstall entry in $hive ($view): $name"
                                }
                            } finally { $entry.Dispose() }
                        }
                    } finally { $arp.Dispose() }
                }

                # The locked NSIS checks the 32-bit machine registration or the current user.
                if ($view -eq 'Registry32' -or $hive -eq 'CurrentUser') {
                    $edge = $base.OpenSubKey($edgePath)
                    if ($edge) {
                        try {
                            $version = $null
                            if ([version]::TryParse([string] $edge.GetValue('pv'), [ref] $version) -and
                                $version -gt [version] '0.0.0.0') {
                                $webviewVersion = $version
                            }
                        } finally { $edge.Dispose() }
                    }
                }
            } finally { $base.Dispose() }
        }
    }

    # Verified with the locked CLI's `tauri inspect wix-upgrade-code`.
    $upgradeCode = '{883767B3-B7BA-5592-A23A-14FAE571E470}'
    if (-not ('AmmSmoke.WindowsInstaller' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
namespace AmmSmoke {
    public static class WindowsInstaller {
        [DllImport("msi.dll", ExactSpelling = true, CharSet = CharSet.Unicode)]
        public static extern uint MsiEnumRelatedProductsW(string upgradeCode, uint reserved,
            uint index, StringBuilder productCode);
        [DllImport("msi.dll", CharSet = CharSet.Unicode)]
        private static extern uint MsiOpenDatabase(string path, IntPtr persist, out uint database);
        [DllImport("msi.dll", CharSet = CharSet.Unicode)]
        private static extern uint MsiDatabaseOpenView(uint database, string query, out uint view);
        [DllImport("msi.dll")] private static extern uint MsiViewExecute(uint view, uint parameters);
        [DllImport("msi.dll")] private static extern uint MsiViewFetch(uint view, out uint record);
        [DllImport("msi.dll", CharSet = CharSet.Unicode)]
        private static extern uint MsiRecordGetString(uint record, uint field, StringBuilder value, ref uint size);
        [DllImport("msi.dll")] private static extern uint MsiCloseHandle(uint handle);

        public static string ReadUpgradeCode(string path) {
            uint database = 0, view = 0, record = 0;
            try {
                Check(MsiOpenDatabase(path, IntPtr.Zero, out database)); // MSIDBOPEN_READONLY
                Check(MsiDatabaseOpenView(database, "SELECT `Value` FROM `Property` WHERE `Property` = 'UpgradeCode'", out view));
                Check(MsiViewExecute(view, 0));
                Check(MsiViewFetch(view, out record));
                uint size = 39;
                var value = new StringBuilder((int)size);
                Check(MsiRecordGetString(record, 1, value, ref size));
                return value.ToString();
            } finally {
                if (record != 0) MsiCloseHandle(record);
                if (view != 0) MsiCloseHandle(view);
                if (database != 0) MsiCloseHandle(database);
            }
        }
        private static void Check(uint code) {
            if (code != 0) throw new Win32Exception((int)code);
        }
    }
}
'@
    }
    if ($Kind -eq 'msi' -and [guid][AmmSmoke.WindowsInstaller]::ReadUpgradeCode($PackagePath) -ne [guid]$upgradeCode) {
        throw "MSI does not have the expected Tauri Mod Manager upgrade code: $PackagePath"
    }
    $productCode = [Text.StringBuilder]::new(39)
    $result = [AmmSmoke.WindowsInstaller]::MsiEnumRelatedProductsW($upgradeCode, 0, 0, $productCode)
    if ($result -eq 0) { throw "Existing Mod Manager MSI product: $productCode" }
    if ($result -ne 259) { throw "Could not verify absence of an installed Mod Manager MSI (error $result)." }

    foreach ($folder in @($localData, $env:ProgramFiles, ${env:ProgramFiles(x86)}, $env:ProgramW6432)) {
        if ($folder) { Assert-Missing (Join-Path $folder 'Mod Manager') }
    }
    if (Get-Process | Where-Object ProcessName -In @('agent-mod-manager', 'Mod Manager')) {
        throw 'Refusing to install while Mod Manager is running. No processes were stopped.'
    }
    if (-not $webviewVersion) {
        throw 'A registered WebView2 Runtime is required before the smoke test; it will not be provisioned by this wrapper.'
    }
    return $webviewVersion
}

function Write-RunLog([string] $Message) {
    Write-Host $Message
    Add-Content -LiteralPath $runLog -Value $Message
}

function Start-LoggedProcess([string] $Executable, [string] $Arguments, [string] $Directory, [string] $Label, [switch] $Wait) {
    Start-Process -FilePath $Executable -ArgumentList $Arguments -WorkingDirectory $Directory `
        -RedirectStandardOutput (Join-Path $evidence "$Label.log") `
        -RedirectStandardError (Join-Path $evidence "$Label.stderr.log") `
        -NoNewWindow -PassThru -Wait:$Wait
}

$repo = Split-Path -Parent $PSScriptRoot
$smoke = Join-Path $PSScriptRoot 'smoke-desktop-webview.mjs'
$picker = Join-Path $PSScriptRoot 'select-windows-smoke-archive.ps1'
$powerShell = Join-Path $PSHOME 'pwsh.exe'
$package = Get-Item -LiteralPath $Installer -Force
$extension = @{ nsis = '.exe'; msi = '.msi' }[$Kind]
if ($package -isnot [IO.FileInfo] -or $package.Extension -ne $extension -or
    ($Kind -eq 'nsis' -and $package.VersionInfo.ProductName -ne 'Mod Manager')) {
    throw "Expected a Tauri Mod Manager $Kind release installer ($extension): $Installer"
}
Assert-LocalUnlinkedPath $package.FullName

# Tauri uses the Windows known folders, not overrides of APPDATA/LOCALAPPDATA in the shell.
$roamingData = [Environment]::GetFolderPath('ApplicationData')
$localData = [Environment]::GetFolderPath('LocalApplicationData')
Assert-LocalUnlinkedPath $roamingData
Assert-LocalUnlinkedPath $localData
$appData = Join-Path $roamingData 'app.agentmodmanager.desktop'
$webviewData = Join-Path $localData 'app.agentmodmanager.desktop'
Assert-FreshProfiles

$evidence = [IO.Path]::GetFullPath($EvidenceDir, $PWD.ProviderPath).TrimEnd('\')
Assert-LocalUnlinkedPath $evidence
foreach ($profilePath in @($appData, $webviewData)) {
    if ($evidence -eq $profilePath -or $evidence.StartsWith("$profilePath\", [StringComparison]::OrdinalIgnoreCase)) {
        throw 'EvidenceDir must be outside the app data directories.'
    }
}
$evidenceItem = Get-ExistingItem $evidence
if ($evidenceItem) {
    if ($evidenceItem -isnot [IO.DirectoryInfo] -or (Get-ChildItem -LiteralPath $evidence -Force | Select-Object -First 1)) {
        throw "EvidenceDir must be a new or empty directory: $evidence"
    }
} else {
    $null = [IO.Directory]::CreateDirectory($evidence)
}
$runLog = Join-Path $evidence 'run.log'
[IO.File]::Open($runLog, [IO.FileMode]::CreateNew).Dispose()

try {
    $node = (Get-Command node.exe -CommandType Application | Select-Object -First 1).Source
    $nodeVersion = & $node --version
    if ($LASTEXITCODE -ne 0 -or $nodeVersion -notmatch '^v24\.\d+\.\d+$') {
        throw "Node 24 is required for the shared SQLite smoke check; found '$nodeVersion'."
    }
    if (-not (Test-Path -LiteralPath $smoke -PathType Leaf) -or
        -not (Test-Path -LiteralPath $picker -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $repo 'node_modules\@playwright\test\package.json') -PathType Leaf)) {
        throw 'The shared smoke script, native picker helper and Yarn-installed Playwright dependencies are required.'
    }

    $webviewVersion = Assert-FreshInstall $package.FullName
    $tempRoot = [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetTempPath())
    Assert-LocalUnlinkedPath $tempRoot
    $sandbox = Join-Path $tempRoot ('amm-package-smoke-' + [guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Path $sandbox
    $target = Join-Path $sandbox 'install'
    $work = Join-Path $sandbox 'work'
    $null = New-Item -ItemType Directory -Path $work
    Assert-Missing $target

    Write-RunLog "Installing $Kind package: $($package.FullName)"
    Write-RunLog "Installer SHA-256: $((Get-FileHash -LiteralPath $package.FullName).Hash)"
    Write-RunLog "Install target: $target; Node: $nodeVersion; registered WebView2: $webviewVersion"
    if ($Kind -eq 'nsis') {
        # NSIS consumes the remainder of the raw command line after /D=, unquoted.
        $installed = Start-LoggedProcess $package.FullName "/S /D=$target" $work 'installer' -Wait
    } else {
        $msiexec = Join-Path ([Environment]::SystemDirectory) 'msiexec.exe'
        $msiLog = Join-Path $evidence 'msiexec.log'
        $arguments = '/i "{0}" /qn /norestart /L*v "{1}" INSTALLDIR="{2}"' -f $package.FullName, $msiLog, $target
        $installed = Start-LoggedProcess $msiexec $arguments $work 'installer' -Wait
    }
    try {
        Write-RunLog "Installer exit code: $($installed.ExitCode)"
        if ($installed.ExitCode -ne 0) { throw 'Installer did not complete with exit code 0.' }
    } finally { $installed.Dispose() }

    $app = Join-Path $target 'agent-mod-manager.exe'
    Assert-LocalUnlinkedPath $app
    $appItem = Get-ExistingItem $app
    if ($appItem -isnot [IO.FileInfo] -or $appItem.Length -eq 0) {
        throw "Installer did not produce the expected application under the fresh target: $app"
    }
    Assert-FreshProfiles

    $screenshot = Join-Path $evidence 'native-smoke.png'
    $smokeArguments = '"{0}" "{1}" "{2}"' -f $smoke, $app, $screenshot
    Write-RunLog "Running native smoke: $app"
    $savedProfile = $env:AMM_WINDOWS_SMOKE_PROFILE
    $savedPowerShell = $env:AMM_WINDOWS_SMOKE_POWERSHELL
    $savedTemp = $env:TEMP
    $savedTmp = $env:TMP
    try {
        $env:AMM_WINDOWS_SMOKE_PROFILE = $appData
        $env:AMM_WINDOWS_SMOKE_POWERSHELL = $powerShell
        $env:TEMP = $work
        $env:TMP = $work
        $test = Start-LoggedProcess $node $smokeArguments $repo 'smoke'
    } finally {
        $env:AMM_WINDOWS_SMOKE_PROFILE = $savedProfile
        $env:AMM_WINDOWS_SMOKE_POWERSHELL = $savedPowerShell
        $env:TEMP = $savedTemp
        $env:TMP = $savedTmp
    }
    try {
        # The shared script owns the app and WebView2 session and verifies SQLite read-only.
        $test.WaitForExit()
        Write-RunLog "Smoke exit code: $($test.ExitCode)"
        if ($test.ExitCode -ne 0) { throw 'The shared desktop smoke failed. See smoke.log and smoke.stderr.log.' }
    } finally { $test.Dispose() }

    $image = Get-ExistingItem $screenshot
    if ($image -isnot [IO.FileInfo] -or $image.Length -eq 0) {
        throw "Desktop smoke did not produce the expected screenshot: $screenshot"
    }
    Write-RunLog "PASS: installed $Kind package smoke. Leaving installation and app data for runner disposal. Evidence: $evidence"
} catch {
    Write-RunLog "FAIL: $($_.Exception.Message)"
    throw
}
