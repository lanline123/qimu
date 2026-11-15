# 界面图标（IconPark）

整稿页右上角五个动作图标，取自 IconPark 官方开源图标库：

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
