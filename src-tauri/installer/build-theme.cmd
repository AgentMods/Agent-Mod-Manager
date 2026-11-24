@echo off
setlocal
rem The Unicode NSIS wizard is 32-bit, including when the installed app is x64.
for /f "usebackq delims=" %%I in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "ammVs=%%I"
if not defined ammVs exit /b 1
call "%ammVs%\Common7\Tools\VsDevCmd.bat" -no_logo -arch=x86 -host_arch=x64 >nul
if errorlevel 1 exit /b 1
if not exist amm-theme-build mkdir amm-theme-build
pushd amm-theme-build || exit /b 1
rc /nologo /i "%~dp0." /fo amm-theme.res "%~dp0theme.rc"
if errorlevel 1 exit /b 1
cl /nologo /LD /O2 /W4 /MT /utf-8 /DUNICODE /D_UNICODE "%~dp0theme.c" "%~dp0bridge.c" amm-theme.res /link /out:AmmTheme.dll comctl32.lib gdi32.lib user32.lib advapi32.lib dwmapi.lib uxtheme.lib
exit /b %errorlevel%
