; Ferrocord Windows installer (NSIS 3.x).
;
; Per-user install: no administrator rights or UAC prompt, installs to
; %LOCALAPPDATA%\Programs\Ferrocord and registers under HKCU (the same model
; as the official Discord client). Machine-wide deployments can use the
; portable .zip instead.
;
; Built by packaging/windows/package.sh (works with makensis on Linux or Windows):
;
;   makensis -DVERSION=0.1.0 -DVERSION_NUMERIC=0.1.0.0 -DARCH=x86_64 \
;            -DSRCDIR=<dir with ferrocord.exe, LICENSE.txt, README.txt> \
;            -DICON=<path to ferrocord.ico> -DOUTFILE=<setup.exe> installer.nsi
;
; Command line of the resulting installer (standard NSIS switches):
;   /S          silent install (no UI; WebView2 prompt is skipped)
;   /D=<dir>    install directory (must be last, no quotes)
; Uninstaller: "%LOCALAPPDATA%\Programs\Ferrocord\uninstall.exe" /S

!ifndef VERSION | VERSION_NUMERIC | ARCH | SRCDIR | ICON | OUTFILE
  !error "Define VERSION, VERSION_NUMERIC, ARCH (x86_64|arm64), SRCDIR, ICON and OUTFILE"
!endif

Unicode true
ManifestDPIAware true
ManifestSupportedOS all
SetCompressor /SOLID lzma
RequestExecutionLevel user

!define APPNAME "Ferrocord"
!define APPID "dev.ferrocord.Ferrocord"
!define PUBLISHER "Ferrocord contributors"
!define HOMEPAGE "https://github.com/ferrocord/ferrocord"
!define EXE "ferrocord.exe"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPID}"
; Evergreen WebView2 Runtime client GUID (Microsoft-documented detection key).
!define WEBVIEW2_GUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
!define WEBVIEW2_URL "https://developer.microsoft.com/microsoft-edge/webview2/"

Name "${APPNAME}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\${APPNAME}"
InstallDirRegKey HKCU "${UNINSTKEY}" "InstallLocation"
BrandingText "${APPNAME} ${VERSION} (${ARCH})"
ShowInstDetails nevershow
ShowUninstDetails nevershow

VIProductVersion "${VERSION_NUMERIC}"
VIFileVersion "${VERSION_NUMERIC}"
VIAddVersionKey "ProductName" "${APPNAME}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${APPNAME} ${VERSION} installer (${ARCH})"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "Copyright (c) ${PUBLISHER}. BSD-3-Clause license."

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
!include "WinVer.nsh"
!include "FileFunc.nsh"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Start ${APPNAME}"
!define MUI_FINISHPAGE_LINK "${APPNAME} on GitHub"
!define MUI_FINISHPAGE_LINK_LOCATION "${HOMEPAGE}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${SRCDIR}\LICENSE.txt"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Function .onInit
  SetShellVarContext current
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "${APPNAME} requires Windows 10 or later." /SD IDOK
    Abort
  ${EndIf}
!if "${ARCH}" == "arm64"
  ${IfNot} ${IsNativeARM64}
    MessageBox MB_OK|MB_ICONSTOP "This installer is for Windows on ARM64. Download the x86_64 installer instead." /SD IDOK
    Abort
  ${EndIf}
!else
  ; x86_64 builds also run on Windows on ARM through emulation; only 32-bit Windows is refused.
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "${APPNAME} requires 64-bit Windows." /SD IDOK
    Abort
  ${EndIf}
!endif
FunctionEnd

Function un.onInit
  SetShellVarContext current
FunctionEnd

; WebView2 is only needed for the captcha window during login. It ships with
; Windows 11 and current Windows 10, so we only point to it when missing.
Function CheckWebView2
  ; A 32-bit installer reads HKLM\SOFTWARE\WOW6432Node here, which is where the runtime registers.
  ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    ReadRegStr $0 HKCU "Software\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  ${EndIf}
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    MessageBox MB_YESNO|MB_ICONINFORMATION "The Microsoft Edge WebView2 Runtime was not found.$\r$\n$\r$\n${APPNAME} works without it, but it is needed to solve captchas during login.$\r$\n$\r$\nOpen the WebView2 download page now?" /SD IDNO IDNO done
    ExecShell "open" "${WEBVIEW2_URL}"
  ${EndIf}
  done:
FunctionEnd

Section "${APPNAME}" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${SRCDIR}\${EXE}"
  File "${SRCDIR}\LICENSE.txt"
  File "${SRCDIR}\README.txt"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${APPNAME}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0 SW_SHOWNORMAL "" "${APPNAME}"

  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${APPNAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\${EXE},0"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTKEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKCU "${UNINSTKEY}" "URLInfoAbout" "${HOMEPAGE}"
  WriteRegStr HKCU "${UNINSTKEY}" "HelpLink" "${HOMEPAGE}/issues"
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINSTKEY}" "EstimatedSize" "$0"

  Call CheckWebView2
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\${APPNAME}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0 SW_SHOWNORMAL "" "${APPNAME}"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "The ${APPNAME} application and a Start menu shortcut."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Put a ${APPNAME} shortcut on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; Settings, plugins, caches (%APPDATA%\ferrocord, %LOCALAPPDATA%\ferrocord) and the
; saved login in Windows Credential Manager are kept, as with most chat clients.
Section "Uninstall"
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APPNAME}.lnk"
  Delete "$DESKTOP\${APPNAME}.lnk"

  DeleteRegKey HKCU "${UNINSTKEY}"
SectionEnd
