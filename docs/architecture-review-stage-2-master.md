# 架构评审 · 阶段 2：master

> 有效期：历史评审快照（记录评审时点事实）；架构变更时需复核，内容不再代表当前状态。
> 最后验证：2026-10-18（commit 1add6dd）

> 方法论：`codebase-design` 深模块词汇 + `improve-codebase-architecture` 流程。
> 约束：不修改任何代码，仅产出分析与建议。
> 范围：`master/`（打包器）全部源码模块。

## 1. 模块结构与依赖

```
master/src/
├── main.rs          # 入口：CLI + GUI 事件循环 + 打包线程调度
├── builder.rs       # 打包编排核心（pack / validate_url / CHILD_TEMPLATE）
├── tail_write.rs    # 尾部写入（外迁自 tail-core，写能力隔离点）
├── ui.rs            # 表单 HTML + IPC 协议解析 + 产物文件名
├── icon.rs          # 母版窗口图标（PNG → tao::Icon）
├── winicon.rs       # PNG→ICO + UpdateResource 注入 .rsrc（Windows-only）
├── sign.rs          # signtool 签名 hook（Windows-only，非阻塞）
└── platform/        # 平台外壳分发（windows.rs / linux.rs，#[cfg] 同名同签名）
```

**依赖方向**（单向）：

```
main ──→ {builder, icon, platform, tail_write, ui}
builder ──→ {tail_core, platform, tail_write}
ui ──→ {tail_core, platform}
tail_write ──→ tail_core
icon ──→ tail_core
winicon/sign ──→ platform re-export（cfg windows）
```

## 2. 逐模块深度评估

| 模块 | pub 项 | 行数 | 测试 | 深度 | 删除测试 |
|---|---|---|---|---|---|
| `main.rs` | 0（入口） | ~297 | 1 | 中 | 入口模块，N/A |
| `builder.rs` | 4 | ~230 | 5 | **深** | 删除后打包编排复杂度全转移到 main → 强烈值得 |
| `tail_write.rs` | 1 | ~103 | 3 | 中 | 关键接缝（写能力隔离），存在意义 > 实现量 |
| `ui.rs` | 4 | ~213 | 7 | 中 | 删除后表单/IPC 逻辑分散 → 值得 |
| `icon.rs` | 2 | ~31 | 0 | **浅** | 薄封装；删除后 PNG→Icon 逻辑转移到 main → 值得（但可合并） |
| `winicon.rs` | 2 | ~311 | 4 | **深** | ICO 格式 + Win32 资源注入复杂度高 → 强烈值得 |
| `sign.rs` | 1 | ~50 | 0 | **浅** | 薄 hook；删除后签名逻辑进 builder → 值得（但可合并） |

**深度判定**：
- `builder.rs`：4 pub 项承载 6 步打包编排（写模板→注入图标→写尾→原子落盘→签名→返回），含错误处理与临时文件清理，深。
- `winicon.rs`：2 pub 函数隐藏 ICO 格式构造、RT_GROUP_ICON/RT_ICON 拆分、Win32 UpdateResource 调用，深。
- `icon.rs` / `sign.rs`：薄封装，接口与实现比例接近 1:1，属浅模块。但它们是**平台接缝的一部分**（icon.rs 隔离 tao 依赖，sign.rs 隔离签名策略），浅是刻意的。

## 3. 深化候选

### 候选 1：提取 `default_output_path(url)` 消除 CLI/GUI 重复

> **状态：已执行**（commit `381d1b1`，2026-10-18。`main.rs` 顶层新增 `default_output_path`，CLI/GUI 两处改为调用。）

- **Files**：`main.rs`（`cli_pack` L80, `start_pack` L189）
- **Problem**：CLI 路径与 GUI 路径各自计算产物输出目录：
  ```rust
  // cli_pack:
  let out = out.unwrap_or_else(|| dir_of_current_exe().join(ui::product_filename(&url)));
  // start_pack:
  let out = dir_of_current_exe().join(ui::product_filename(&url));
  ```
  两处逻辑完全相同（CLI 多了 `--out` 覆盖），违反 locality 原则——未来改产物命名规则需同步两处。
- **Solution**：提取 `fn default_output_path(url: &str) -> PathBuf`，CLI 用 `out.unwrap_or_else(|| default_output_path(&url))`，GUI 直接调用。
- **Benefits**：
  - **Locality**：产物路径规则单点持有，改一处即生效。
  - 零行为风险，纯重构。
- **Before / After**：

```
Before:
cli_pack:    dir_of_current_exe().join(ui::product_filename(&url))  [1]
start_pack:  dir_of_current_exe().join(ui::product_filename(&url))  [2]  ← 重复

After:
default_output_path(url):  dir_of_current_exe().join(ui::product_filename(url))
cli_pack:    out.unwrap_or_else(|| default_output_path(&url))
start_pack:  default_output_path(&url)
```

- **Recommendation strength**：`Worth exploring`
- **ADR 冲突**：无。

### 候选 2：`builder::validate_url` 与 `tail_core` URL 工具的部分重叠

- **Files**：`builder.rs`（`validate_url` L26-46）
- **Problem**：`validate_url` 自行实现 scheme 剥离 + host 提取（`trim_start_matches` + `split`），与 `tail_core::host_of` 的 host 提取逻辑部分重叠。
- **分析**：语义不同——`validate_url` 是**用户输入合法性校验**（scheme 必须 http/https + host 须含点），`host_of` 是**运行时 host 提取**（宽松，无 scheme 返回空串）。两者关注点不同，强行复用会让校验逻辑依赖运行时函数的宽松行为，反而降低安全性。
- **结论**：保持独立。`validate_url` 的点号检查（点两侧均有字符）是打包前的输入防线，独立实现更清晰。
- **Recommendation strength**：`Speculative`（不建议）。

### 候选 3：`icon.rs` / `sign.rs` 浅模块是否应合并

- **Files**：`icon.rs`、`sign.rs`
- **Problem**：两模块均为薄封装（2 pub / 1 pub），接口与实现比例接近 1:1。
- **分析**：
  - `icon.rs` 浅是因为它隔离 `tao::window::Icon` 依赖——这是平台接缝的一部分，刻意浅。
  - `sign.rs` 浅是因为签名是可选 hook，策略可替换。
  - 合并到 `builder.rs` 会让 builder 承担图标/签名的平台细节，破坏 builder 的编排纯粹性。
- **结论**：保持独立。浅模块在此处是**接缝纪律**的体现，不是设计缺陷。
- **Recommendation strength**：`Speculative`（不建议）。

### 候选 4：`icon.rs` 无单测

- **Files**：`icon.rs`
- **Problem**：`window_icon` / `default_window_icon` 无单测。
- **分析**：依赖 `tao::window::Icon::from_rgba`，测试需构造真实 Icon 对象，且 `default_window_icon` 依赖编译期 `include_bytes!`。测试成本高于收益——窗口图标的正确性由 e2e（窗口标题栏图标可见）覆盖。
- **结论**：暂不补充。若未来 `window_icon` 逻辑复杂化（如多尺寸选择）再补。
- **Recommendation strength**：`Speculative`。

## 4. Top Recommendation

**候选 1（提取 `default_output_path`）** 是唯一值得执行的深化项。

理由：
1. 真实的 locality 违反——同一逻辑两处实现。
2. 零行为风险，纯结构重构。
3. 为未来产物命名规则变更（如 Linux deb 包名调整）提供单点修改位。

## 5. ADR 冲突检查

`docs/adr/` 尚无文件。本次评审无已记录决策需回避。

## 6. 下一步

- 若认可候选 1，可进入 grilling 循环。
- 阶段 3：`child/`（运行时：`webview` / `popup` / `icon` / `platform`）。
- 阶段 4：跨 crate 接缝与 CI/构建脚本。
