; This include is loaded by Tauri before the MUI pages are declared. The stock
; template still owns directory selection, upgrades, /P, /S and installation.
!define AMM_SKIN_DIR "${__FILEDIR__}"
!include "${AMM_SKIN_DIR}\locales.nsh"
!ifdef NSIS_WIN32_MAKENSIS
  !system '"${AMM_SKIN_DIR}\build-theme.cmd"' = 0
!else
  ; For cross-bundling, use the 32-bit MinGW tools on the build host.
  !system 'mkdir -p amm-theme-build' = 0
  !system 'i686-w64-mingw32-windres -I "${AMM_SKIN_DIR}" "${AMM_SKIN_DIR}/theme.rc" -O coff -o amm-theme-build/theme-res.o' = 0
  !system 'i686-w64-mingw32-gcc -shared -Os -DUNICODE -D_UNICODE "${AMM_SKIN_DIR}/theme.c" "${AMM_SKIN_DIR}/bridge.c" amm-theme-build/theme-res.o -o amm-theme-build/AmmTheme.dll -lcomctl32 -lgdi32 -luser32 -ladvapi32 -ldwmapi -luxtheme' = 0
!endif
!addplugindir /x86-unicode "amm-theme-build"

!define MUI_WELCOMEFINISHPAGE_BITMAP "${AMM_SKIN_DIR}\sidebar-light.bmp"
!define MUI_CUSTOMFUNCTION_GUIINIT AmmInitializeWizard
!define MUI_PAGE_CUSTOMFUNCTION_SHOW AmmShowWelcome
; MUI expands this in the page declaration, after Tauri defines VERSION.
!define MUI_WELCOMEPAGE_TITLE "v${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "$(AmmWelcomeBody)"

; Declare the MUI handles before referring to them in our show callback.
!define MUI_PAGE_UNINSTALLER_PREFIX ""
!insertmacro MUI_WELCOMEPAGE_INTERFACE
!undef MUI_PAGE_UNINSTALLER_PREFIX

Var AmmHeadingFont
Var AmmBodyFont
Var AmmSmallFont

Function AmmInitializeWizard
  AmmTheme::Initialize /NOUNLOAD
  CreateFont $AmmHeadingFont "Segoe UI" 22 700
  CreateFont $AmmBodyFont "$(^Font)" 11 400
  CreateFont $AmmSmallFont "$(^Font)" 9 400
FunctionEnd

; Position the two existing MUI labels in the same dialog units as nsDialogs.
!macro AmmPlaceLabel hwnd x y w h
  System::Call '*(i ${x}, i ${y}, i ${w}, i ${h}) p .r8'
  System::Call 'user32::MapDialogRect(p $HWNDPARENT, p r8)'
  System::Call '*$8(i .r4, i .r5, i .r6, i .r7)'
  System::Free $8
  System::Call 'user32::MoveWindow(p ${hwnd}, i r4, i r5, i r6, i r7, i 0)'
!macroend

Function AmmShowWelcome
  System::Store S
  ; All visible copy stays in native static controls for screen readers.
  !insertmacro AmmPlaceLabel $mui.WelcomePage.Title 127 161 184 16
  !insertmacro AmmPlaceLabel $mui.WelcomePage.Text 127 91 184 64
  SendMessage $mui.WelcomePage.Title ${WM_SETFONT} $AmmSmallFont 0
  SendMessage $mui.WelcomePage.Text ${WM_SETFONT} $AmmBodyFont 0

  ${NSD_CreateLabel} 127u 29u 14u 2u ""
  Pop $0
  ${NSD_CreateLabel} 127u 39u 184u 15u "$(AmmWelcomeKicker)"
  Pop $1
  SendMessage $1 ${WM_SETFONT} $AmmSmallFont 0
  ${NSD_CreateLabel} 126u 56u 185u 30u "Agent Mod Manager"
  Pop $2
  ${NSD_AddStyle} $2 ${SS_LEFTNOWORDWRAP}
  SendMessage $2 ${WM_SETFONT} $AmmHeadingFont 0
  AmmTheme::Welcome /NOUNLOAD $mui.WelcomePage.Image $2 $mui.WelcomePage.Text $1 $mui.WelcomePage.Title $0
  System::Store L
FunctionEnd
