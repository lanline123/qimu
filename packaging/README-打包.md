# 打包说明（第一期 · 服务器与客户端分开）

2026-11-23 起，启幕的 Windows 分发是**两个独立安装包**，不再是一个包装两个组件：

| 包 | 脚本 | 产物 | 内容 | 安装目录（默认，D 盘存在时） | 卸载登记 |
|---|---|---|---|---|---|
| 客户端（软件本体） | `启幕-客户端.nsi` | `启幕客户端-Setup.exe` | qimu-client.exe + qimu.exe（CLI）+ 使用说明 | `D:\Program Files\启幕客户端` | `Uninstall\QimuClient` |
| 服务器 | `启幕-服务器.nsi` | `启幕服务器-Setup.exe` | qimu-server.exe（+ 预建 qimu-data、开机自启） | `D:\Program Files\启幕服务器` | `Uninstall\QimuServer` |

- 两包互不依赖：同事只装客户端，在「设置 → 服务器」填服务器地址（或局域网自动发现）即可连接；服务器装在一台常开机器上，不必装客户端。
- 旧的合一脚本 `启幕.nsi` 已停用（文件头有标注），仅备查。
- 便携版仍在 `portable/`（启服务器/启客户端两个 bat 本就分开），与安装包口径一致。

## 构建路径

- 工具链与旧包一致：Linux 上 mingw 交叉编译 `x86_64-pc-windows-gnu`（release）得三个 exe → 拷入 `packaging/win/` → makensis 出两个包。NSIS 脚本头有中文文件名需先拷 ASCII 名的注意事项。
- **字体**：客户端内嵌普惠体需构建前把 `AlibabaPuHuiTi-3-55-Regular.ttf` 放到 `crates/qimu-client/assets/`（来源与 SHA-256 见该目录 README）；缺失时回退系统字体，不影响功能。
- GitHub Actions 自动化见 `.github/workflows/build-windows.yml`（手动触发：交叉编译 → 两次 makensis → 两个安装包作为 artifact 上传）。
