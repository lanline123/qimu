@echo off
cd /d %~dp0
echo 启幕服务器启动中（数据保存在 qimu-data 文件夹）...
echo 保持此窗口打开；关闭窗口即停止服务器。
qimu-server.exe
pause
