# assets

`AlibabaPuHuiTi-3-55-Regular.ttf` — 阿里巴巴普惠体 3.0 Regular（Version 3.01），
从阿里官方 CDN（fonts.alibabadesign.com）下载，SHA-256
`be33bc8d45ced30fa5c542f6f056565baf85003d4e079522110d76016f8a04e2`。
普惠体为全社会永久免费商用字体；客户端在构建时内嵌它作为默认界面字体
（见 ../build.rs）。该文件是二进制资产，不随源码推送到远程仓库；
缺失时客户端回退到系统 CJK 字体，不影响编译与功能。

## logo.png.b64 — 品牌 logo（2026-11-23 落仓）

- **来源**：产品原型《启幕原型-内容索引页.html》顶栏内嵌的
  `data:image/png;base64` 图片（`.tb-logo`），逐字节提取，未做任何
  改动。128×128 PNG，蓝色渐变「Q」缎带标，透明底，
  SHA-256 `40729996967703234922b52541e857bae448e4fa2a46ea95721e42fbb70ec144`。
- **用途（单一事实源）**：客户端壳体顶栏 / 登录弹窗 / 关于页渲染
  （`LOGO_PNG`）；窗口与任务栏图标（`app_icon()`）；Windows 可执行
  文件图标（`build.rs` 解码后包成 .ico 经 winresource 嵌入）；
  安装包图标（CI 由本文件生成 `packaging/qimu.ico` 与
  `packaging/win/qimu.ico`，两个 .nsi 的 MUI_ICON 与快捷方式引用）。
- 换 logo 只改这一个文件，上述各处自动跟随。
