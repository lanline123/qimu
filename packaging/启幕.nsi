; 【已停用 · 2026-11-23 第一期分开打包】本脚本是客户端+服务器合一的旧包,
; 已被同目录的「启幕-客户端.nsi」与「启幕-服务器.nsi」两个独立安装包取代
; (服务器一个程序、客户端一个程序, 各自安装/卸载)。保留仅备查, 勿再用它出包。
;
; 启幕 Windows 安装包 (NSIS)
; 构建 (Linux): cp 启幕.nsi qimu-setup.nsi && LC_ALL=C.UTF-8 makensis qimu-setup.nsi
;   - 中文脚本文件名会让 makensis 的 argv 转换失败, 故先拷成 ASCII 名再构建;
;     产物名仍由 OutFile 决定 (启幕-Setup.exe)。
;   - SetCompressor 用普通 lzma: /SOLID lzma 在本机 makensis 3.09 上 SIGBUS。
; 产物: 启幕-Setup.exe
; 二进制来源: win\ 子目录 (windows-gnu release 构建的三件套 + 使用说明)

Unicode true
SetCompressor lzma

!include "MUI2.nsh"
!include "LogicLib.nsh"

Name "启幕"
OutFile "启幕-Setup.exe"
; 静态默认走系统 Program Files; .onInit 在 D 盘存在时改到 D:\Program Files\启幕
InstallDir "$PROGRAMFILES64\启幕"
InstallDirRegKey HKLM "Software\Qimu" "InstallDir"
RequestExecutionLevel admin

!define PRODUCT "启幕"
!define VERSION "0.1.0"
!define PUBLISHER "启幕"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Function .onInit
  ; 默认安装路径: D:\Program Files\启幕 (D 盘不存在则保留 $PROGRAMFILES64)
  ${If} ${FileExists} "D:\*.*"
    StrCpy $INSTDIR "D:\Program Files\启幕"
  ${EndIf}
FunctionEnd

; ---------------- 客户端 (必选) ----------------
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
  ; 卸载登记
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKLM "Software\Qimu" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Qimu" "DisplayName" "${PRODUCT}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Qimu" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Qimu" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Qimu" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Qimu" "UninstallString" "$INSTDIR\uninstall.exe"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Qimu" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Qimu" "NoRepair" 1
SectionEnd

; ---------------- 服务器 (可选, 默认勾选) ----------------
Section "启幕服务器" SEC_SERVER
  SetOutPath "$INSTDIR"
  File "win/qimu-server.exe"
  ; 数据目录预建: 服务器 exe 同目录有 qimu-data 时优先用它
  CreateDirectory "$INSTDIR\qimu-data"
  CreateShortcut "$SMPROGRAMS\启幕\启幕服务器.lnk" "$INSTDIR\qimu-server.exe"
  ; 开机自启 (当前用户 Run 项)
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "QimuServer" "$INSTDIR\qimu-server.exe"
SectionEnd

LangString DESC_CLIENT ${LANG_SIMPCHINESE} "桌面客户端与命令行工具 (必选)。"
LangString DESC_SERVER ${LANG_SIMPCHINESE} "服务器组件: 装在一台常开的电脑上即可, 会随开机自动启动。只用别人服务器的同事可以不装。"
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_CLIENT} $(DESC_CLIENT)
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_SERVER} $(DESC_SERVER)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---------------- 卸载 ----------------
Section "Uninstall"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "QimuServer"
  Delete "$DESKTOP\启幕.lnk"
  Delete "$SMPROGRAMS\启幕\启幕.lnk"
  Delete "$SMPROGRAMS\启幕\启幕服务器.lnk"
  Delete "$SMPROGRAMS\启幕\使用说明.lnk"
  RMDir "$SMPROGRAMS\启幕"
  Delete "$INSTDIR\qimu-client.exe"
  Delete "$INSTDIR\qimu-server.exe"
  Delete "$INSTDIR\qimu.exe"
  Delete "$INSTDIR\使用说明.txt"
  Delete "$INSTDIR\uninstall.exe"
  ; 离线缓存可重建, 直接清掉
  RMDir /r "$INSTDIR\cache"
  ; 服务器数据默认保留, 询问后才删
  ${If} ${FileExists} "$INSTDIR\qimu-data\*.*"
    MessageBox MB_YESNO|MB_DEFBUTTON1 "保留服务器数据 (qimu-data) 吗?$\n$\n选「是」: 保留内容库数据 (推荐, 以后重装还能用)。$\n选「否」: 连同数据一起删除, 不可恢复。" IDYES keep_data
      RMDir /r "$INSTDIR\qimu-data"
    keep_data:
  ${EndIf}
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Qimu"
  DeleteRegKey HKLM "Software\Qimu"
SectionEnd
