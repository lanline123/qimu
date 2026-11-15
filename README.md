# 启幕（暂定名）· PPT 管理器 — LAN 版

面向大企业的 PPT 内容管理与原地更新系统：一期为局域网部署（Rust 客户端
GUI + CLI + 服务端），设计规格见 `docs/`。

## 架构（一期两件套，规格 §4.3）

- `crates/qimu-core` — 共享核心库：领域模型（内容块 / 形态 / 语义版本 /
  索引 Git 模型）与 PPTX 包处理。全部逻辑只写一遍，GUI / CLI / 服务端
  都是它的前端 —— 这是 CLI 与 GUI 全量等价（AI 可全量接管）的前提。
- `crates/qimu-server` — 局域网主机服务（Axum）：一份可迁移数据目录
  （SQLite 元数据 + 内容哈希文件存储）。
- `crates/qimu-cli` — CLI：全非交互、JSON 进出、稳定退出码，与 GUI 全量等效。
- `crates/qimu-client` — 桌面 GUI（egui）：演示厅 / 内容页 / 索引页 / 设置四页。
- `crates/qimu-bridge` — PowerPoint COM 桥（§4.3）：Windows 上驱动正在运行的
  PowerPoint 做原地替换；其他平台为桩实现，文件级流程由 CLI `deck apply` 承担。

## 地基复用（storytold Crafting Apps，MIT OR Apache-2.0）

- DeckCraft：PPTX 引擎与放映、兼容性参照（`vendor/deckcraft`）
- craft-libs：egui 壳、通用控件、原生对话框、自更新（含局域网镜像）
- craft-i18n：界面多语言词条与检查
- craft-ai-integrations：AI 服务接入层（翻译 / 分块提议 / 讲稿起草）
- PdfCraft：附件 PDF 客户端内预览候选

`vendor/` 下为浅克隆参照；正式依赖在 POC 验证后以路径依赖 / 子模块锁定版本接入。

## 当前进度

- [x] 工作区骨架与领域模型（含 X.Y.Z 语义版本与递增规则）
- [x] PPTX 按页拆分 → 片段重组 → 逐页指纹校验（规格 §3.1 导入安全网，
      Route B 包级手术），CLI `qimu deck split-verify <file> --json`
- [x] 存储层（SQLite + 内容哈希文件仓库）、账号 / 组织 / 组权限（服务端强制）
- [x] 服务端完整 API：登录、导入、发布、索引（分支 / 标签 / 提交）、导出
      （内嵌清单）、更新检查、原地更新包、附件、演示厅审核、搜索
- [x] CLI 全命令面（含 `deck check` / `deck apply` 同事端原地接受更新）
- [x] egui 四页 GUI（界面截图见 `docs/screenshots/`）
- [x] 端到端验收（Redwood 真稿 17 页）：导入拆分 → 建索引 → 导出 →
      发布新版 → 同事端检查到更新 → 原地替换（页数可变）→ 复检最新
- [x] COM 桥：trait + Windows COM 实现（windows 目标编译通过，待实机验证）
- [x] 真实缩略图：服务端 LibreOffice 渲染每页 PNG（首次渲染后磁盘缓存），
      GUI 内容卡 / 索引图墙 / 版本行全部显示真图
- [x] 个人绑定导出（§3.14）：`{{姓名/电话/公司网站…}}` 模板占位符在导出时
      按导出人资料（个人版）或公司公共资料（公用版，个人字段擦除）解析；
      缺字段拦截导出，绝不拿他人数据顶替（`tools/test_personalize.py`）
- [x] 索引分支合并（§3.10）：条目级三方合并（分叉点为基），单边改动自动
      合并、双边同改出冲突清单，裁决后生成合并提交；服务端 + CLI + GUI
      裁决区（`tools/test_merge.py`）
- [x] 块内语言绑定（§3.12）：逐页绑定（版本页 ↔ 版本页，1 对多）数据 +
      API + CLI + 内容块详情内缩略图点选绑定
- [x] 离线缓存（§3.17）：演示厅「我的库」——广场条目 / 我的组装一键缓存
      整稿快照到本地（~/.qimu/cache），离线可打开 / 移除
- [x] 场景 13 保真（可自动化部分）：自制案例 2 稿（原生图表×3+内嵌数据、表格、动画 timing、切换、备注、嵌入视频）与真 SmartArt 稿（Apache POI 夹具）经导入→导出后部件齐全、LibreOffice 渲染逐页像素一致（tools/test_fidelity.py）；清单存活：LibreOffice 打开→保存会剥离自定义属性与镜像部件，已加固为三路承载——核心属性关键词中的 deck 标识（编辑器会把冒号规范化为空格，读取端兼容）在 LO 往返后存活，CLI 可凭标识从服务器导出台账恢复完整清单并正常检查更新
- [x] 演示厅分享与群组空间（§3.17/§7.7）：自定义组（建组/成员管理）、三类指定分享（个人→收件箱、部门→含子部门与后来加入者、自定义组→群组空间）、保存到我的库（引用）、条目级导出授权（分享即授权、离组即撤销），store/API/CLI/GUI 四层齐全，E2E 见 tools/test_shares.py
- [x] 讲稿（§3.16）：版本逐页讲稿（导入时由备注生成）可在线编辑且不升版本；导出勾选「添加备注」把讲稿写回备注页（tools/test_scripts.py）
- [x] AI 翻译骨架（§3.13）：术语表（服务端设置）+ 可插拔 provider（内置 stub 供开发/测试；OpenAI 兼容接口已实现，设置里配 endpoint/model/key 后启用）→ 生成目标语言形态的草稿版本（待校对）、逐页 ai 绑定记录配对、讲稿一并翻译；图表内部缓存标签不在 run 级翻译范围内
- [x] 导入提块（§5.8）：POST /v1/import/propose 给出分块建议（当前为确定性启发式 proposer，接口即 AI 提块的接入点），CLI `import-propose` / `import --propose`、GUI「智能分块导入」，导入者确认后落库
- [x] 修复：组装改写 .rels 时元素扫描前缀碰撞（<Relationship 误匹配 <Relationships 根元素）曾使全部导出稿的 .rels 部件畸形（LibreOffice 宽容未报错，严格读取器拒绝）；已修复并加回归断言，导出稿现可通过 python-pptx 严格加载、全部部件 XML 良构
- [x] 索引条目摘录选页 GUI：图墙条目「选页」打开页缩略图勾选（全选/清空），保存即提交（全选自动清除摘录标记），卡面显示「摘录 N 页」
- [x] 离线缓存策略与加密：管理员策略（允许开关 + 容量上限，服务端设置、客户端执行、CLI cache-policy）；缓存快照 AES-256-GCM 加密落盘（本机密钥 ~/.qimu/cache.key），打开时解密到临时副本；策略区仅超级管理员可见
- [x] 设置页按设计稿重做为左侧分区导航 + 单分区显示（个人资料 / 组织管理 / 内容组与授权 / 公司公共资料 / 内容语言 / AI 翻译 / 离线缓存策略，分区按角色渲染、不可见不渲染），表单标签与输入框成列对齐，主保存按钮为蓝色胶囊；AI 翻译分区补齐（提供方/接口/模型/密钥 + 术语表编辑，此前仅 CLI 可改）
- [x] 修复打包形态下缩略图永久挂死：数据目录为相对路径（打包默认 qimu-data）时，LibreOffice 档案 URI 被拼成 file://qimu-data/...（主机名错位）导致 soffice 无限挂起；改为绝对路径 + Windows 路径规范化后生成 URI（本机以打包目录全新安装实测：缩略图 6 秒返回真实渲染图）
- [x] 界面字体换为阿里巴巴普惠体 3.0（官方 CDN 下载、SHA-256 校验一致；构建时内嵌，缺字体文件时回退系统 CJK 字体）；Windows 试用包已可交叉编译打包（mingw 目标 x86_64-pc-windows-gnu，三个 exe 仅依赖系统 DLL）
- [x] 客户端视觉改版（Craft 家族深色设计语言，按 EffectCraft 实机界面定版；先在 FigCraft 里画设计稿确认再实现，设计稿与规范在 design/ 目录）：画布 #121212、chrome #1C1C1C、卡片 #232323 + #383838 细边 + 10px 圆角、蓝色强调 #2D8CEB（页签下划线 / 主按钮 / 版本胶囊）、选中态用灰不用蓝填充、无描边深灰输入框 #2A2A2A、紧凑间距（13px 正文字体）；卡片为封面优先版式，索引条目卡为左侧首页大图 + 多页小图阵列；顶行 = 名称 + 形态·语言·版本同排、策略胶囊/锁定切换/选页/↑↓/红色 × 在右上角一排；条目下方有快速切换条（历史版本倒序小图，单击切换，选历史自动锁定、选最新恢复跟随），条尾「更多」弹窗列全部历史版本；卡片之间有「+ 插入条目」入口（弹窗选块插入指定位置）
- [x] 演示厅「开始演示」：广场条目一键物化整稿并以系统默认演示应用打开（DeckCraft 全屏接入点已留）
- [ ] DeckCraft 往返保真验证（需实机打开 DeckCraft）
- [ ] COM 桥原地替换回路（Windows PowerPoint 实机）

## 构建与运行

```sh
cargo build --workspace
cargo test --workspace
cargo run -p qimu-cli -- deck info <file.pptx> --json
cargo run -p qimu-cli -- deck split-verify <file.pptx> --json
cargo run -p qimu-server   # http://127.0.0.1:8730/health
```
