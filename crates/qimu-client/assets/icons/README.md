# 界面图标（IconPark）

界面图标取自 IconPark 官方开源图标库。整稿页右上角五个动作图标
（play/share/export/download/back）为第一批；2026-11-16 补第二批外壳图标
（工具行与导航面板，统一线性风格、烘焙为白色、运行时以 tint 着色，
静止 #B4B4B4 = EffectCraft 的 icon 色）：

- `home`（Base/home）→ 工具行 Home 槽
- `refresh`（Base/refresh）→ 工具行刷新槽（演示厅）
- `power`（Base/power）→ 工具行退出槽
- `globe`（Travel/globe）→ 演示厅子区·广场
- `bookshelf`（Office/bookshelf）→ 演示厅子区·我的库
- `inbox`（Office/inbox）→ 演示厅子区·收到的分享
- `peoples-two`（Peoples/peoples-two）→ 演示厅子区·群组空间
- `all-application`（Base/all-application）→ 文件夹树·全部
- `folder-close` / `folder-open`（Office）→ 文件夹行（未选/已选）
- `folder-plus`（Office/folder-plus）→ 导航面板页脚·新建文件夹
- `split`（Edit/split）→ 工具行·智能分块导入（内容页）
- `upload`（Arrows/upload）→ 工具行·导入 PPT（内容页）
- `clipboard`（Office/clipboard）→ 工具行·粘贴新建（内容页）
- `doc-add`（Office/doc-add）→ 工具行·新建索引（索引页）

第二批的着色转换即 IconPark 官方「线性」主题规则：主填充 #2F88FF 置空、
次填充 #43CCF8 转实色、black 描边与 white 细节线统一转目标色；
upload 的源文件带全幅 alpha mask（栅格器不渲染），已去 mask 取其三条描边路径。

- 来源仓库：https://github.com/bytedance/IconPark （`source/` 目录原始 SVG）
- 许可证：Apache-2.0（IconPark / ByteDance）
- 取用日期：2026-11-15
- 图标（官方名 / 仓库路径 / 用途）：
  - `play`（播放，source/Music/play.svg）→ 开始演示
  - `share`（分享，source/Base/share.svg）→ 分享
  - `export`（导出，source/Abstract/export.svg）→ 导出
  - `download`（下载，source/Arrows/download.svg）→ 缓存到本地（2026-11-15 补加的第五个图标）
  - `back`（上一步/返回，source/Edit/back.svg）→ 返回

处理方式：按 IconPark 官方「线性（outline）」主题约定重新着色——描边改目标色、
填充置空（play 的三角保留实心以保证小尺寸可读性）；play/share/export/download 用
#F2F2F2，back 用 #E5534B。着色后的 SVG 与 96×96 预渲染 PNG 一并存于本目录。

客户端内嵌的是同名 `.png.b64`（PNG 的 base64 文本），不是 PNG 原文件：
GitHub 快照推送通道只传 UTF-8 文本，二进制直传会被有损转码损坏。改了某个
PNG 后必须同步再生其 b64：`base64 -w0 <name>.png > <name>.png.b64`。
