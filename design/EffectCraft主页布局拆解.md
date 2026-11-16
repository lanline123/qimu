# EffectCraft 主页布局拆解（源码为准）

> 2026-11-15 23:30 James 定版：布局以 EffectCraft **源码**为准，不再靠截图推测。
> 来源：`vendor/effectcraft`（storytold/effectcraft，HEAD d6c9ad66，v0.24.1）+ `vendor/craft-libs/crates/ui`（craft-ui 停靠系统）。
> 本文逐区给结构与真实取值，每条注明源文件。截图只作对照校验；与此前截图采样冲突处单列在文末，以代码为准。

## 0. 总览：主窗口纵向四段

宿主 `apps/effectcraft/src/main.rs` 只负责开窗与装配，真正的帧结构在
`crates/ui-egui/src/app/frame.rs` 的 `frame()`：

| 段 | 高度 | 底色 | 出处 |
|---|---|---|---|
| 菜单行（可关，默认开） | 24 | header_bg #1F1F1F | frame.rs（rect 高 24.0）；state.rs `show_menu_bar: true` |
| 工具行（header） | 40 | header_bg #1F1F1F，底部 1px 线用 app_bg 色 | frame.rs `header_h = 40.0`；header.rs `show()` |
| 主体 body（停靠区 / Home 屏） | 其余 | app_bg #121212 铺满全窗 | frame.rs |
| 状态栏 | **无** | 状态消息是左下浮动 toast（见 §6） | frame.rs `draw_toast()` |

- 窗口默认 **1680×1020**，最小 960×600（`apps/effectcraft/src/window.rs` `DEFAULT_SIZE` / `MIN_SIZE`）。
- body 矩形：左右各内缩 **4**，上 = 工具行底 **+2**，下 = 窗口底 **−4**（frame.rs）。
  1680×1020 时 body ≈ 1672×950（y 66..1016）。
- egui 布局值是逻辑点：窗口缩到 1440×900 时固定尺寸（24/40/300/76/30）**不变**，只有弹性区变小。本文与设计板均按此规则。

## 1. 菜单行（24px）

- 构成：egui `MenuBar`，标题为 Button（全局 Button 字样式 = **12px**，theme.rs `text_styles`），左起 6px 空（`crates/ui-egui/src/menus/bar.rs`）。
- 顶级菜单（非 macOS 顺序）：**File / Edit / Composition / Layer / Effect / Animation / View / Window / Help**；macOS 最前多一个 EffectCraft 应用菜单（`crates/engine/src/menus.rs` 的 `TREE` 常量，行 562 起逐个顶级菜单）。
- 打开中的标题取 open 态样式；按下即开、悬停可横扫切换（bar.rs 文件头注释）。

## 2. 工具行（40px，header.rs `show()`）

整行垂直居中（cy = 行中心），从左到右：

| 元素 | 尺寸/步进 | 细节 | 出处 |
|---|---|---|---|
| 左起点 | x=10（集成标题栏且无菜单行时 80） | | header.rs `show()` |
| 品牌 logo | 24×24，步进 32 | 紫色菱形标记，点击开 About 对话框 | header.rs `brand_and_home()` |
| Home 钮 | 26×26 图标钮，步进 30 | 切换 Home 屏（start_screen） | 同上 |
| 竖分隔线 | 高 20（cy±10），#343434，之后 +6 | | 同上 |
| 工具槽 ×16 | 每槽 26×26，步进 28 | 槽序：Selection/Hand/Zoom/Orbit/PanCamera/Dolly/Rotate/PanBehind/形状组/Pen组/Type组/Brush/Clone/Eraser/Roto组/Puppet组；槽 **2/5/7/10/13 后**加分隔（+4、线、+6）；多工具槽右下有小三角（长按/右键弹组内菜单） | state.rs `Tool::SLOTS`；header.rs `tool_slots()` |
| 工具钮状态 | 激活 = **accent #2D8CEB 填充、圆角 4、白图标**；hover = #303030 填充；常态图标 #B4B4B4；图标本体缩进 22%（≈14.5px） | | widgets.rs `icon_button()` |
| 工具选项区 | 前置 +10，内容随当前工具变 | 形状工具时是 Fill/Stroke 色块与 Tool Creates Shape/Mask 钮；Puppet/Roto 时换各自选项 | header.rs `tool_options_bar()` |
| Snapping | 复选框 20×20 + 标签 12px #9A9A9A + 选项钮 18×18 | 引擎状态镜像（View ▸ Snapping） | header.rs `snapping_toggle()` |

右侧（从右往左排，header.rs `right_side()`）：

| 元素 | 尺寸 | 细节 |
|---|---|---|
| Discord 胶囊 | 高 26、圆角 13、右边距 10 | 底 #5865F2（hover ×1.2 提亮）；内 14px Chat 图标 + "Discord" semibold 12 白字；宽度按文字自适应（左内边距 10、图标 14、间 6、右 12） |
| GitHub / 官网钮 | 各 26×26，间隔 4 | Code 图标 / Globe 图标 |
| 外观钮 | 26×26，间隔 4 | Monitor/Sun/Moon 图标，点击循环 Sync/Light/Dark，右键菜单 |
| 竖分隔线 | 前后各留 10 | 高 20，#343434 |
| 工作区 » 钮 | 20×22 | "»" 15px，点开全部 15 个工作区菜单 |
| 工作区页签 | 高 26，12px | 只显 5 个，视觉左→右：**Default / Review / Learn / Small Screen / Standard**；文字左右内边距 8；**激活 = hot_text #3D8FF5 字 + 2px 同色下划线**（距底 3，左右缩 8）；hover = #C8C8C8；常态 #9A9A9A；距工具区结束 <120 时截断不画 |

## 3. 主体：craft-ui 停靠框架系统

`crates/ui-egui/src/dock_ui.rs` 的 `dock_area()` 把 body 交给 craft-ui 的 `DockArea`；
EffectCraft 只供面板本体、组头装饰与命令路由，框架几何由 `vendor/craft-libs/crates/ui/src/docking/` 绘制。

- **框架间隙 gap = 4**：分隔处透出 app_bg 成暗缝；分隔条可拖改尺寸，hover/拖动/焦点时变 accent（theme.rs `gap: 4.0`；dock_ui.rs `dock_style()`；craft-libs `render/dock.rs`）。
- **框架无边框、无圆角**（浮动窗口例外：圆角 4 + 1px accent 描边，craft-libs `render/frame.rs`）。
- **页签条**：每框架顶部高 **30**（`tab_h`），条底 = panel_bg #232323，与框架体同色。
  - 页签 = 纯文字 **12px**，宽 = 文字宽 + **24**（左内边距 12）；**激活与未激活同底色，只差字色**：#E3E3E3 vs #9A9A9A（dock_ui.rs `dock_style()` 把 tab/active/floating 底色全设为 panel_bg；craft-libs `render/group.rs` `tab()`）。
  - 页签溢出收进 "…" 溢出钮（宽 22）；滚轮可切页签、中键关页签（dock_style 标志）。
  - 页签条顶部 **4px 是整组拖拽带**（craft-libs group.rs `BORDER_BAND`）。
- **组头（页签条右端）**：锁钮（占宽 18、图标 14；仅 Composition/Timeline/Effect Controls 组有，锁合成/图层）+ **≡ 面板菜单钮**（占宽 26、图标 16）（dock_ui.rs `group_header()`，常量 `MENU_W`/`LOCK_W`）。
- **框架可动性**：页签可拖到任意框架成组、拖到边缘拆分、Ctrl/Cmd 拖或拖出全部框架则浮动成独立 OS 窗口（默认浮窗 420×320 居中）；最后一个停靠框架不可被拖空（dock_ui.rs 文件头注释与 `apply_dock_all()`）。
- **焦点框架无常驻描边**：只有键盘焦点落在页签上时才有 1px accent 焦点描边（craft-libs group.rs `tab()` 的 focus_stroke）。
- Timeline 框架的页签条在面板页签之后还有**合成文档页签**（色标 + 名 + ×，宽 = 文字 + 24 + 16），是 EffectCraft 自绘的组头扩展（dock.rs `doc_tabs()`）。

### 3.1 面板清单（dock.rs `PanelKind`，共 34 种 + 2 参数化）

Project、Effect Controls、Composition、Layer、Timeline、Info、Audio、Preview、Effects & Presets、
Properties、Character、Paragraph、Align、Tracker、Wiggler、Smoother、Motion Sketch、Paint、Brushes、
Render Queue、Flowchart、History、Markers、Mask Interpolation、Script Console、Essential Graphics、
Video Scopes（LumetriScopes）、Footage、Media Browser、Metadata、Progress、Content-Aware Fill、
Create Nulls From Paths、VR Comp Editor；参数化：ScriptPanel(id)、Viewer(id)。

### 3.2 Default 工作区布局树（dock.rs `workspace_root()` 的 Default 分支）

```
根 hsplit FixedSecond(300)                    ← 右列固定 300
├─ 左主区 vsplit Ratio(0.56)                  ← 上 56% / 下 44%
│   ├─ 上 hsplit FixedFirst(300)              ← 左列固定 300
│   │   ├─ 页签组 [Project*, Effect Controls]
│   │   └─ 页签组 [Composition*, Layer]
│   └─ 页签组 [Timeline*, Render Queue]
└─ 右列 vsplit FixedFirst(76)                 ← 76 = 页签条 30 + 46 播放控制体（PREVIEW_FRAME）
    ├─ 页签组 [Preview*]
    └─ 页签组 [Properties*, Align, Audio, Effects & Presets]
```
（* = 该组当前显示的页签。）

1680×1020 下验算（body 1672×950，间隙 4）：
左主区宽 1672−300−4 = **1368**；上部高 (950−4)×0.56 ≈ **530**、下部 ≈ 416；
上部中列宽 1368−300−4 = **1064**；右列下框高 950−76−4 = **870**。

1440×900 下（设计板尺寸，body 1432×830）：左主区宽 **1124**；上部高 ≈ **463**、下部 ≈ 363；
中列宽 **820**；右列下框高 **750**。

### 3.3 其他预置工作区（共 15 个，dock.rs `WORKSPACES`）

Default、Standard、Small Screen、Animation、Effects、Motion Tracking、Paint、Text、Minimal、
All Panels、Review、Learn、Essential Graphics、Color、Undocked Panels。
结构同构（左右固定列 + 上下比分）：如 Standard 左 300 / 右 290 / 上 0.56，右列改为 Ratio(0.45) 的
[Info, Audio] / [Preview, Effects & Presets]；Small Screen 240 / 240。
**内置布局一律不用 Stack（手风琴）节点**，右列都是上下分帧——有测试断言（dock.rs `after_effects_workspaces_exist_with_their_core_panels`）。

## 4. 面板本体与框架的边界（本轮只拆框架，内容不展开）

框架只负责面板的几何、页签条与组头；面板本体（body）由应用自绘，框架不规定其内部布局。
以左列 Project 面板为例（panels/project.rs）：本体是「预览头 → 搜索框 → 列表（行高 19）→ 底栏」的纵向序列——
这是面板内容，不属于框架，本拆解不展开。启幕同样不搬任何面板内容，只把自有页面内容装进框架。

## 5. Home 屏（只记与外壳的关系）

start_screen 时 Home 覆盖整个 body 区（菜单行/工具行仍在），内部为左轨 236（panel_bg + 右 1px separator 线）+ 主区。
Home 是 EffectCraft 的功能内容，启幕不搬；与框架有关的只有一条：**外壳两行恒在，body 可被整块覆盖**（panels/home.rs）。

## 6. 状态提示（无状态栏）

`frame.rs draw_toast()`：消息浮在窗口左下（x = 左 +16，底 = 窗口底 −44，被导入卡片顶起时上移），
panel_bg 底、圆角 6、1px field_border 描边，12px 字，内边距 12×7；状态消息 5 秒、错误 toast 4 秒后消失，点击可关。

## 7. 主题 token（Dark，默认主题，theme.rs `Tokens::for_kind`）

| token | 值 | token | 值 |
|---|---|---|---|
| app_bg | #121212 | text | #C8C8C8 |
| header_bg | #1F1F1F | text_dim | #9A9A9A |
| panel_bg | #232323 | text_faint | #8C8C8C |
| tab_text | #9A9A9A | icon / icon_active | #B4B4B4 / #F0F0F0 |
| tab_text_active | #E3E3E3 | hover / pressed | #303030 / #454545 |
| focus = accent | #2D8CEB | field_bg / field_border | #171717 / #3A3A3A |
| accent_hover | #4AA0F5 | separator | #343434 |
| hot_text | #3D8FF5 | row / row_alt / row_selected | #232323 / #272727 / #3A3A3A |
| radius / radius_sm | 6 / 3 | gap / tab_h / row_h | 4 / 30 / 19 |

全局字样式：Body 12、Button 12、Small 11、Heading 15 semibold（theme.rs `text_styles`）；
字体 Inter + JetBrains Mono。另有 Darker、Light 两套主题与亮度微调（`Tokens::from_prefs` 的 `with_brightness`），
默认解析为 Dark。

## 8. 与截图旧认知的出入（以代码为准）

1. **工具行/菜单行底色**：代码 header_bg = **#1F1F1F**。此前截图采样得 #17181C 并写进规范——该值介于 Dark（#1F1F1F）与 Darker（#161616）之间，应是截图那台机器的主题/亮度设置不同。按默认 Dark 以 #1F1F1F 为准。
2. **面板底色**：代码 panel_bg = **#232323**。此前规范的卡面 #1E1F24 在代码三套主题里都不存在（Darker 是 #191919）。以 #232323 为准。
3. **面板页签激活态**：代码里激活与未激活**同底色、只差字色**，没有下划线、没有蓝。此前采样把「白字 + 下划线」当作页签通则不准确——**蓝字 + 蓝下划线只出现在工具行的工作区页签**，且蓝是 hot_text #3D8FF5，不是 accent #2D8CEB。
4. **不存在「侧边栏」构件**：左列就是一个与其它框架完全同构的停靠框架（Default 下装 Project 页签组、固定 300 宽），有页签条、组头 ≡ 菜单，可拖走、可浮动。此前规范 §10 围绕侧栏「内嵌面板 vs 贴边」的讨论在框架层面不成立——它就是 dock 框架之一；启幕若复刻框架，导航应装进左框架，而不是另设侧栏构件。
5. **内容区不是裸画布贴卡片**：body 有 4/2/4 内缩，框架间 4px 间隙透 app_bg；框架本体无边框无圆角，层级靠 panel_bg 与 app_bg 的明度差 + 间隙暗缝。
6. **没有状态栏**：状态是左下浮动 toast（§6）。此前若按「底部状态条」理解属误读。
7. **行高/行距**：列表行高 19（此前截图量测 ≈20，一致）；工具行高 40、菜单行高 24 此前未定值，现以代码为准。
8. 选中行 #3A3A3A（row_selected）与早先采样一致 ✓；工具激活钮的蓝填充圆角 4 与截图一致 ✓（色值 accent #2D8CEB）。
