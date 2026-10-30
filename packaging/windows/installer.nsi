; YKPili Windows 安装包（NSIS 3）
;
; 由 scripts/package-windows.ps1 调用：
;   makensis /INPUTCHARSET UTF8 /DAPP_VERSION=0.1.0 /DSOURCE_DIR=... /DOUTFILE=... installer.nsi

Unicode true

!include "MUI2.nsh"

!ifndef APP_VERSION
  !define APP_VERSION "0.1.0"
!endif
!ifndef SOURCE_DIR
  !define SOURCE_DIR "..\..\dist\YKPili-${APP_VERSION}-windows-x64"
!endif
!ifndef OUTFILE
  !define OUTFILE "..\..\dist\YKPili-${APP_VERSION}-windows-x64-setup.exe"
!endif

Name "YKPili"
BrandingText "YKPili"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\YKPili"
InstallDirRegKey HKCU "Software\YKPili" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetCompressorDictSize 64

Icon "ykpili.ico"
UninstallIcon "ykpili.ico"

VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey /LANG=2052 "ProductName" "YKPili"
VIAddVersionKey /LANG=2052 "FileDescription" "YKPili 安装程序"
VIAddVersionKey /LANG=2052 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "CompanyName" "YKPili"
VIAddVersionKey /LANG=2052 "LegalCopyright" "GPL-2.0-or-later"

!define MUI_ICON "ykpili.ico"
!define MUI_UNICON "ykpili.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\ykpili.exe"
!define MUI_FINISHPAGE_RUN_TEXT "运行 YKPili"
!define MUI_FINISHPAGE_LINK "查看 README（Windows 版）"
!define MUI_FINISHPAGE_LINK_LOCATION "$INSTDIR\README-Windows.txt"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

; ================= 旧安装目录检测 =================
;
; 目标：升级时默认装回用户上一次选的目录，而不是又落回
; `%LOCALAPPDATA%\Programs\YKPili`。
;
; 优先级：
;   1. `InstallDirRegKey` 已经读到的 HKCU\Software\YKPili\InstallDir；
;   2. 卸载表里的 InstallLocation（HKCU / HKLM）；
;   3. 常见目录；
;   4. 扫描所有固定磁盘的 `X:\Program Files\YKPili` 等位置
;      （覆盖用户手动把绿色包放到 D:\Program Files 的情况）。
;
; 如果用户传了 `/D=...`，$INSTDIR 已不是默认值，这里直接跳过。

Function .onInit
  StrCmp $INSTDIR "$LOCALAPPDATA\Programs\YKPili" 0 init_done
  Call DetectPreviousInstall
  init_done:
FunctionEnd

Function DetectPreviousInstall
  StrCpy $R9 "0"

  ; ---- 1. 卸载表 ----
  ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "InstallLocation"
  Call TryInstallDir
  ReadRegStr $0 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "InstallLocation"
  Call TryInstallDir

  ; ---- 2. 应用自己的键 ----
  ReadRegStr $0 HKCU "Software\YKPili" "InstallDir"
  Call TryInstallDir
  ReadRegStr $0 HKLM "Software\YKPili" "InstallDir"
  Call TryInstallDir

  ; ---- 3. 常见目录 ----
  StrCpy $0 "$LOCALAPPDATA\Programs\YKPili"
  Call TryInstallDir
  StrCpy $0 "$PROGRAMFILES\YKPili"
  Call TryInstallDir
  StrCpy $0 "$PROGRAMFILES32\YKPili"
  Call TryInstallDir

  ; ---- 4. 固定磁盘扫描 ----
  StrCpy $R0 "CDEFGHIJKLMNOPQRSTUVWXYZ"
  StrCpy $R1 0
  scan_loop:
    StrCmp $R9 "1" scan_done
    StrCpy $R2 $R0 1 $R1
    StrCmp $R2 "" scan_done

    ; 只扫固定磁盘，跳过 U 盘 / 光驱 / 网络盘，避免弹盘或卡住。
    System::Call 'kernel32::GetDriveTypeW(w "$R2:\") i .r3'
    StrCmp $3 "3" 0 scan_next

    StrCpy $0 "$R2:\Program Files\YKPili"
    Call TryInstallDir
    StrCpy $0 "$R2:\Program Files (x86)\YKPili"
    Call TryInstallDir
    StrCpy $0 "$R2:\YKPili"
    Call TryInstallDir

  scan_next:
    IntOp $R1 $R1 + 1
    Goto scan_loop
  scan_done:
FunctionEnd

Function TryInstallDir
  StrCmp $R9 "1" done
  StrCmp $0 "" done
  IfFileExists "$0\ykpili.exe" 0 done
  StrCpy $INSTDIR "$0"
  StrCpy $R9 "1"
  done:
FunctionEnd


Section "YKPili（必需）" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; 整个暂存目录：ykpili.exe / libmpv-2.dll / LICENSES / README
  File /r "${SOURCE_DIR}\*.*"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateDirectory "$SMPROGRAMS\YKPili"
  CreateShortcut "$SMPROGRAMS\YKPili\YKPili.lnk" "$INSTDIR\ykpili.exe"
  CreateShortcut "$SMPROGRAMS\YKPili\卸载 YKPili.lnk" "$INSTDIR\uninstall.exe"
  CreateShortcut "$DESKTOP\YKPili.lnk" "$INSTDIR\ykpili.exe"

  WriteRegStr HKCU "Software\YKPili" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\YKPili" "Version" "${APP_VERSION}"

  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "DisplayName" "YKPili"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "Publisher" "YKPili"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "DisplayIcon" "$INSTDIR\ykpili.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\YKPili.lnk"
  Delete "$SMPROGRAMS\YKPili\YKPili.lnk"
  Delete "$SMPROGRAMS\YKPili\卸载 YKPili.lnk"
  RMDir "$SMPROGRAMS\YKPili"

  RMDir /r "$INSTDIR"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\YKPili"
  DeleteRegKey HKCU "Software\YKPili"
SectionEnd
