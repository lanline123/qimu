# 启幕（暂定名）· PPT 管理器 — LAN 版

面向大企业的 PPT 内容管理与原地更新系统：一期为局域网部署（Rust 客户端
GUI + CLI + 服务端），设计规格见 `docs/`。

## 架构（一期两件套，规格 §4.3）

- `crates/qimu-core` — 共享核心库：领域模型（内容块 / 形态 / 语义版本 /
  索引 Git 模型）与 PPTX 包处理。全部逻辑只写一遍，GUI / CLI / 服务端
  都是它的前端 —— 这是 CLI 与 GUI 全量等价（AI 可全量接管）的前提。
- `crates/qimu-server` — 局域网主机服务（Axum）：一份可迁移数据目录
  （SQLite 元数据 + 内容哈希文件存储）。
- `crates/qimu-cli` — CLI：全非交互、JSON 进出、稳定退出码。
- `crates/qimu-client` — 桌面 GUI（egui 壳在 POC 下一步接入）。

## 地基复用（storytold Crafting Apps，MIT OR Apache-2.0）

- DeckCraft：PPTX 引擎与放映、兼容性参照（`vendor/deckcraft`）
- craft-libs：egui 壳、通用控件、原生对话框、自更新（含局域网镜像）
- craft-i18n：界面多语言词条与检查
- craft-ai-integrations：AI 服务接入层（翻译 / 分块提议 / 讲稿起草）
- PdfCraft：附件 PDF 客户端内预览候选

`vendor/` 下为浅克隆参照；正式依赖在 POC 验证后以路径依赖 / 子模块锁定版本接入。

## 当前进度（Phase-0 POC）

- [x] 工作区骨架与领域模型（含 X.Y.Z 语义版本与递增规则）
- [x] PPTX 按页拆分 → 片段重组 → 逐页指纹校验（规格 §3.1 导入安全网，
      Route B 包级手术），CLI `qimu deck split-verify <file> --json`
- [ ] DeckCraft 往返保真与内嵌清单存活验证（场景 13 门槛）
- [ ] COM 桥原地替换回路（Windows PowerPoint 实机）
- [ ] egui 壳接入与四页界面

## 构建与运行

```sh
cargo build --workspace
cargo test --workspace
cargo run -p qimu-cli -- deck info <file.pptx> --json
cargo run -p qimu-cli -- deck split-verify <file.pptx> --json
cargo run -p qimu-server   # http://127.0.0.1:8730/health
```
