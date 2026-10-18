# 架构评审 · 阶段 3：child

> 方法论：`codebase-design` 深模块词汇 + `improve-codebase-architecture` 流程。
> 约束：不修改任何代码，仅产出分析与建议。
> 范围：`child/`（子应用运行时模板）全部源码模块。

## 1. 模块结构与依赖

```
child/src/
├── main.rs          # 入口：读尾部配置 → open_webview（极简，31 行）
├── webview.rs       # 统一渲染层：装配 + JS 注入桥 + 事件循环（591 行）
├── popup.rs         # 新窗口路由判定（纯逻辑，零 OS 依赖）
├── icon.rs          # 窗口图标转换（PNG → tao::Icon）
└── platform/        # 平台外壳（windows 804 / linux 474 行，#[cfg] 同名同签名）
```

**依赖方向**：

```
main ──→ {webview, platform}
webview ──→ {popup, tail_core, platform}
icon ──→ tail_core
platform ──→ {windows|linux}
```

## 2. 逐模块深度评估

| 模块 | pub 项 | 行数 | 测试 | 深度 | 删除测试 |
|---|---|---|---|---|---|
| `main.rs` | 0 | 31 | 0 | 极简入口 | N/A |
| `webview.rs` | 5 | 591 | ~10 | 中 | 删除后装配逻辑全转移到 main → 强烈值得 |
| `popup.rs` | 1（+辅助） | 201 | 5 | **深** | 深模块典范：1 函数承载 4 路路由判定 → 强烈值得 |
| `icon.rs` | 2 | 98 | 4 | 中 | 薄封装但隔离 tao 依赖 → 值得 |
| `platform/` | ~12 接缝项 | 1278 | 0(平台逻辑) | **深** | 平台差异收敛点 → 强烈值得 |

**深度判定**：
- `popup.rs`：**深模块典范**。`classify_new_window` 一个函数承载空白窗/同站/跨站/伪协议四路判定，接口窄（3 参数），实现厚（规则 + 同站边界判定），零 OS 依赖，可单测。
- `webview.rs`：中。5 pub 项（`ChildEvent`/`open_webview`/`build_window`/`build_webview`/`webview_data_dir`），但内部混合了窗口构建、WebView 配置、JS 注入、事件循环。作为「装配层」，内聚是合理的。
- `platform/`：深。12 个接缝项承载 Win32/GTK 两套实现，是跨平台差异收敛点。

## 3. 架构摩擦点

### 正面：`popup.rs` 是深模块典范

- 纯判定逻辑，零 OS 依赖，与平台实现（`platform::create_popup` 等）完全解耦。
- 接口：`classify_new_window(url, opener_host, has_popup_features) -> NewWindowAction`，三参数一返回值。
- 实现：4 路规则（空白窗 → Popup；http(s) → 按同站/popup 特征分流；伪协议 → Ignore；其余 → External）+ `same_site` 点边界判定。
- 测试：5 个单测覆盖全部规则与边界。
- **无需改进**。

### 正面：`icon.rs` 与 `master/icon.rs` 的刻意重复是正确的

- 两文件几乎同构（`window_icon` + `default_window_icon`）。
- 原因：child 是二进制 crate 无 lib 导出；master 不依赖 child。若合并需引入共享 crate，为 30 行逻辑新增 crate 不划算。
- 符合「**两个 adapter 才是真实接缝**」原则——此处重复是刻意的，不是缺陷。

### 候选 1：`webview.rs` 的脚本注入逻辑是否应独立

- **Files**：`webview.rs`（`home_hotkey_script` ~L353-413，及其他注入函数）
- **Problem**：`webview.rs` 591 行混合了装配（窗口/WebView 构建）和 JS 注入逻辑（回首页脚本、全屏上报、window.close 拦截等）。注入逻辑是纯字符串构造，与装配无关。
- **分析**：
  - 当前 `webview.rs` 作为「统一渲染层」，装配 + 注入同处是刻意设计（文档明确说「本文件只做装配与纯 JS 注入桥」）。
  - 注入脚本与 WebView 配置紧密耦合（如 `with_initialization_script` 必须在 `build_webview` 内调用）。
  - 拆出独立 `injection` 模块会增加模块数，但注入函数的 caller 只有 `build_webview`，属「一个 adapter = 假设接缝」。
- **结论**：保持。装配层内聚是合理的，拆分收益 < 成本。
- **Recommendation strength**：`Speculative`。

### 候选 2：`webview.rs` 的 `open_webview` 是否过长

- **Files**：`webview.rs`（`open_webview` L56-131，`build_webview` L167-248）
- **Problem**：`open_webview` 包含事件循环处理（新窗口、全屏、标题、Toast、GoHome），逻辑较长。
- **分析**：事件循环的 match 分支是天然的事件分发结构，每个分支调用 platform 或 webview 自身方法。拆分事件处理器反而增加间接层。
- **结论**：保持。事件循环的 match 结构是清晰的。
- **Recommendation strength**：`Speculative`。

### 候选 3：`platform/` 无单测

- **Files**：`platform/windows.rs`、`platform/linux.rs`
- **Problem**：平台外壳（1278 行）无单测。
- **分析**：平台代码依赖 Win32/GTK 运行时，单元测试需要真实环境。当前由 e2e（Windows 30 项 / Linux 37 项）覆盖。平台外壳的接缝签名由编译器强制（缺一即编译失败），行为由 e2e 验证。
- **结论**：保持。平台代码的测试策略是 e2e 而非单测，这是合理的（平台代码的错误是行为错误，不是逻辑错误）。
- **Recommendation strength**：`Speculative`。

## 4. Top Recommendation

**child 架构整体成熟，无值得立即执行的深化项。**

理由：
1. `popup.rs` 是深模块典范，纯判定 + 零 OS 依赖 + 可单测。
2. `webview.rs` 作为装配层，内聚是合理的设计选择（文档明确）。
3. `icon.rs` 与 master 的重复是刻意的（两个 adapter = 真实接缝）。
4. `platform/` 的测试由 e2e 覆盖，符合平台代码的测试策略。

child 是项目中架构最干净的部分——经过跨平台验证（Windows + Linux）逼出的接缝设计已经稳定。

## 5. ADR 冲突检查

`docs/adr/` 尚无文件。本次评审无已记录决策需回避。

## 6. 下一步

- 阶段 4：跨 crate 接缝（tail-core ↔ child/master）与 CI/构建脚本（audit.ps1 / build.ps1 / e2e.ps1）。
