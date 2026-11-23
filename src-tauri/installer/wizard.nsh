; This include is loaded by Tauri before the MUI pages are declared. The stock
; template still owns directory selection, upgrades, /P, /S and installation.
!define AMM_SKIN_DIR "${__FILEDIR__}"
!ifdef NSIS_WIN32_MAKENSIS
  !system '"${AMM_SKIN_DIR}\build-theme.cmd"' = 0
!else
  ; For cross-bundling, use the 32-bit MinGW tools on the build host.
  !system 'i686-w64-mingw32-windres -I "${AMM_SKIN_DIR}" "${AMM_SKIN_DIR}/theme.rc" -O coff -o amm-theme-res.o' = 0
  !system 'i686-w64-mingw32-gcc -shared -Os -DUNICODE -D_UNICODE "${AMM_SKIN_DIR}/theme.c" "${AMM_SKIN_DIR}/bridge.c" amm-theme-res.o -o AmmTheme.dll -lcomctl32 -lgdi32 -luser32 -ladvapi32 -ldwmapi -luxtheme' = 0
!endif
!addplugindir /x86-unicode "."

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

LangString AmmInterfaceFont 1033 "Segoe UI"
LangString AmmInterfaceFont 2052 "Microsoft YaHei UI"
LangString AmmWelcomeKicker 1033 "Setup wizard"
LangString AmmWelcomeKicker 2052 "安装向导"
LangString AmmWelcomeBody 1033 "Choose an installation folder and set up Agent Mod Manager on this computer."
LangString AmmWelcomeBody 2052 "此向导将帮助你选择安装位置，并在这台电脑上安装 Agent Mod Manager。"

Var AmmHeadingFont
Var AmmBodyFont
Var AmmSmallFont

Function AmmInitializeWizard
  AmmTheme::Initialize /NOUNLOAD
  CreateFont $AmmHeadingFont "Segoe UI" 23 700
  CreateFont $AmmBodyFont "$(AmmInterfaceFont)" 10 400
  CreateFont $AmmSmallFont "$(AmmInterfaceFont)" 9 400
FunctionEnd

; Position the two existing MUI labels in the same dialog units as nsDialogs.
!macro AmmPlaceLabel hwnd x y w h
  System::Call '*(i ${x}, i ${y}, i ${w}, i ${h}) p .r8'
  System::Call 'user32::MapDialogRect(p $mui.WelcomePage, p r8)'
  System::Call '*$8(i .r4, i .r5, i .r6, i .r7)'
  System::Free $8
  System::Call 'user32::MoveWindow(p ${hwnd}, i r4, i r5, i r6, i r7, i 0)'
!macroend

Function AmmShowWelcome
  System::Store S
  ; All visible copy stays in native static controls for screen readers.
  !insertmacro AmmPlaceLabel $mui.WelcomePage.Title 127 151 184 16
  !insertmacro AmmPlaceLabel $mui.WelcomePage.Text 127 112 184 36
  SendMessage $mui.WelcomePage.Title ${WM_SETFONT} $AmmSmallFont 0
  SendMessage $mui.WelcomePage.Text ${WM_SETFONT} $AmmBodyFont 0

  ${NSD_CreateLabel} 127u 29u 14u 2u ""
  Pop $0
  ${NSD_CreateLabel} 127u 39u 184u 15u "$(AmmWelcomeKicker)"
  Pop $1
  SendMessage $1 ${WM_SETFONT} $AmmSmallFont 0
  ${NSD_CreateLabel} 126u 56u 185u 60u "Agent Mod$\r$\nManager"
  Pop $2
  SendMessage $2 ${WM_SETFONT} $AmmHeadingFont 0
  AmmTheme::Welcome /NOUNLOAD $mui.WelcomePage.Image $2 $mui.WelcomePage.Text $1 $mui.WelcomePage.Title $0
  System::Store L
FunctionEnd
