; 启幕客户端 · 独立 Windows 安装包 (NSIS) —— 第一期分开打包 (2026-11-23)
; 与服务器安装包 (启幕-服务器.nsi) 完全独立: 各自安装目录、各自卸载项,
; 互不捆绑, 可单独安装/升级/卸载。客户端通过「设置 → 服务器」填写
; 服务器地址连接任意一台启幕服务器 (含局域网自动发现)。
; 构建 (Linux): cp 启幕-客户端.nsi qimu-client-setup.nsi && LC_ALL=C.UTF-8 makensis qimu-client-setup.nsi
;   - 中文脚本文件名会让 makensis 的 argv 转换失败, 故先拷成 ASCII 名再构建;
;     产物名仍由 OutFile 决定。
;   - SetCompressor 用普通 lzma: /SOLID lzma 在 makensis 3.09 上 SIGBUS (旧包教训)。
; 产物: 启幕客户端-Setup.exe
; 二进制来源: win\ 子目录 (windows-gnu release 构建的 qimu-client.exe + qimu.exe + 使用说明)

Unicode true
SetCompressor lzma

!include "MUI2.nsh"
!include "LogicLib.nsh"

Name "启幕客户端"
OutFile "启幕客户端-Setup.exe"
; 默认走系统 Program Files; .onInit 在 D 盘存在时改到 D:\Program Files\启幕客户端
InstallDir "$PROGRAMFILES64\启幕客户端"
InstallDirRegKey HKLM "Software\QimuClient" "InstallDir"
RequestExecutionLevel admin

!define PRODUCT "启幕客户端"
!define VERSION "0.1.0"
!define PUBLISHER "启幕"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Function .onInit
  ${If} ${FileExists} "D:\*.*"
    StrCpy $INSTDIR "D:\Program Files\启幕客户端"
  ${EndIf}
FunctionEnd

Section "启幕客户端" SEC_CLIENT
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "win/qimu-client.exe"
  File "win/qimu.exe"
  File "win/使用说明.txt"
  ; 离线缓存目录预建 (客户端默认 cache 路径 = 安装目录\cache)
  CreateDirectory "$INSTDIR\cache"
  ; 快捷方式: 桌面 + 开始菜单
  CreateShortcut "$DESKTOP\启幕.lnk" "$INSTDIR\qimu-client.exe"
  CreateDirectory "$SMPROGRAMS\启幕"
  CreateShortcut "$SMPROGRAMS\启幕\启幕.lnk" "$INSTDIR\qimu-client.exe"
  CreateShortcut "$SMPROGRAMS\启幕\使用说明.lnk" "$INSTDIR\使用说明.txt"
  ; 卸载登记 (独立于服务器包的 QimuServer 项)
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKLM "Software\QimuClient" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\QimuClient" "DisplayName" "${PRODUCT}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\QimuClient" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\QimuClient" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\QimuClient" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\QimuClient" "UninstallString" "$INSTDIR\uninstall.exe"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\QimuClient" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\QimuClient" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\启幕.lnk"
  Delete "$SMPROGRAMS\启幕\启幕.lnk"
  Delete "$SMPROGRAMS\启幕\使用说明.lnk"
  RMDir "$SMPROGRAMS\启幕"
  Delete "$INSTDIR\qimu-client.exe"
  Delete "$INSTDIR\qimu.exe"
  Delete "$INSTDIR\使用说明.txt"
  Delete "$INSTDIR\uninstall.exe"
  ; 离线缓存可重建, 直接清掉
  RMDir /r "$INSTDIR\cache"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\QimuClient"
  DeleteRegKey HKLM "Software\QimuClient"
SectionEnd
