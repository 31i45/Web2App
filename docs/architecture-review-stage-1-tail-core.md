# 架构评审 · 阶段 1：tail-core

> 方法论：`codebase-design` 深模块词汇（module / interface / depth / seam / adapter / leverage / locality）+ `improve-codebase-architecture` 流程（Explore → Present → Grilling）。
> 约束：不修改任何代码，仅产出分析与建议。
> 范围：`tail-core/` 拆分后的四子模块（`tail` / `text` / `image` / `url`）+ `lib.rs` 门面。

## 1. 模块结构概览

```
tail-core/src/
├── lib.rs    # 纯 re-export 壳：pub use {tail,text,image,url}::*
├── tail.rs   # 尾部协议域（协议解析/编码 + JSON + CRC32 + IO）
├── text.rs   # 文本编码域（JS 字面量 + base64）
├── image.rs  # 图像处理域（PNG 解码 + box 缩放）
└── url.rs    # URL 工具域（host 提取/净化/指纹）
```

**依赖方向**（单向，无环）：

```
tail ──→ text（base64_encode/decode）
image ──→ png crate
url ──→ （无内部依赖）
lib ──→ {tail, text, image, url}（re-export）
```

## 2. 逐模块深度评估

| 模块 | pub interface 项数 | 实现复杂度 | 测试数 | 深度判定 | 删除测试结论 |
|---|---|---|---|---|---|
| `lib.rs` | 18（re-export） | 无 | 0 | 门面壳，N/A | 删除后调用方需用子模块长路径，复杂度转移到调用方 → 值得保留 |
| `tail.rs` | 9（含 AppConfig 方法） | 高：协议定位/CRC/JSON/IO | 27 | **深** | 删除后尾部协议复杂度全部转移到 child/master → 强烈值得 |
| `text.rs` | 3 | 中：各 30–50 行 | 1 | 中 | 删除后编码逻辑分散到 tail 和 child/master → 值得 |
| `image.rs` | 2 | 高：PNG 解码+炸弹防御+box 缩放 | 7 | **深** | 删除后 PNG 处理复杂度转移到 child/icon、master/winicon → 强烈值得 |
| `url.rs` | 4 | 中：字符串处理 | 7 | 中 | 删除后文件名安全逻辑分散到 master/child → 值得 |

**深度判定依据**（`codebase-design` 词汇）：
- `tail.rs`：9 pub 项承载协议帧定位（`locate_in`/`classify`）、CRC32、手写 JSON 转义/解析、磁盘 IO 全部复杂度，接口窄而实现厚，符合深模块。
- `image.rs`：2 pub 函数（`decode_png`/`resize_box`）隐藏 PNG 格式细节与炸弹防御，深。
- `text.rs` / `url.rs`：接口与实现比例适中，属中等深度的工具模块。

## 3. 深化候选

### 候选 1：base64 测试归属迁移到 text.rs

> **状态：已执行**（commit `e253ca4`，2026-10-18。迁移后 text.rs 共 3 个测试，tail.rs 25 个，总数 42 不变。）

- **Files**：`tail.rs`（`base64_known_vectors`, `base64_roundtrip_random_lengths`）→ `text.rs`
- **Problem**：`base64_encode`/`base64_decode` 定义在 `text.rs`，但它们的核心测试（2 个）放在 `tail.rs`。这违反「**接口即测试面**」原则——`text.rs` 的 interface 测试应在 `text.rs` 内。当前状态是拆分时按「尾部协议域」分组的副作用：base64 主要服务于 `AppConfig`，测试随协议域走了。
- **Solution**：将 `base64_known_vectors` 和 `base64_roundtrip_random_lengths` 移入 `text.rs` 的 `#[cfg(test)] mod tests`。`tail.rs` 的 `to_json`/`from_json` 已有 roundtrip 测试覆盖 base64 集成路径，单元测试归位不影响覆盖。
- **Benefits**：
  - **Locality**：`text.rs` 的测试面回到 `text.rs`，未来修改 base64 时无需跨模块找测试。
  - **Leverage**：如果 base64 未来被其他模块使用，测试已在正确位置，无需搬迁。
  - 接口与测试的对应关系清晰，降低 AI 导航成本。
- **Before / After**：

```
Before:                         After:
tail.rs                         text.rs
  tests:                          tests:
    base64_known_vectors  ──┐        base64_known_vectors
    base64_roundtrip_*     ──┤        base64_roundtrip_*
    ... 25 tail tests        │        js_string_literal_escapes_literals
                              └──→    （共 3 个）
text.rs                         tail.rs
  tests:                          tests:
    js_string_literal               ... 25 tail tests（无 base64 单元测试）
```

- **Recommendation strength**：`Worth exploring`
- **ADR 冲突**：无（`docs/adr/` 尚无记录）。

### 候选 2：tail.rs 内部三域（协议 / JSON / CRC32）是否独立成模块

- **Files**：`tail.rs`
- **Problem**：`tail.rs` 内部耦合了三个子域：尾部协议解析（`locate_in`/`classify`/`Frame`）、JSON 转义解析（`json_escape`/`extract_string_field`/`unescape`）、CRC32（`Crc32`/`payload_crc`）。理论上 JSON 和 CRC 是通用工具，可独立。
- **Solution**：无（保持现状）。
- **Benefits of keeping**：
  - **删除测试**：若现在把 JSON/CRC 抽成独立模块，复杂度不消失，但增加模块数且**无第二个 caller**——属于「为抽象而抽象」，违反「**一个 adapter 是假设接缝，两个才是真实接缝**」原则。
  - 当前 JSON/CRC 只服务于尾部协议，物理上靠近使用点是正确的。
- **触发条件**：仅当 JSON 或 CRC 被 `tail.rs` 之外的模块需要时才考虑抽离。
- **Recommendation strength**：`Speculative`（当前不建议）。

### 候选 3：url.rs 的 default_title 是否应降为 private

- **Files**：`url.rs`
- **Problem**：`default_title` 同时被 `sanitize_host` 内部调用和对外暴露。若只有内部 caller，应降为 private 以收窄接口。
- **验证结论**：经 grep 确认，`default_title` 被 `master/builder.rs`、`child/platform/{linux,windows}.rs`、`master/platform/linux.rs` 直接调用（窗口标题、deb 包名等场景），**必须保持 pub**。
- **Recommendation strength**：不适用（排除）。

## 4. Top Recommendation

**候选 1（base64 测试归位）** 是唯一值得立即执行的深化项。

理由：
1. 它是当前唯一真实的浅接缝征兆——`text.rs` 的接口测试不在自己模块内。
2. 改动纯测试代码，零行为风险，不影响 42 个测试的总数与覆盖。
3. 提升 `text.rs` 的 locality，未来 base64 若扩展 caller 无需二次搬迁。

候选 2（tail.rs 内部分拆）当前不满足「两个 adapter」条件，保持内聚是对的。

## 5. ADR 冲突检查

`docs/adr/` 目录已约定（见 `PLATFORM_SEAM_NOTES.md` §9）但尚无 ADR 文件。本次评审无已记录决策需回避。

## 6. 下一步

- 若认可候选 1，可进入 grilling 循环细化（迁移哪些测试、是否保留 tail.rs 中的 base64 集成断言）。
- 阶段 2：`master/`（打包器，含 `builder`/`tail_write`/`winicon`/`sign`/`platform`）。
- 阶段 3：`child/`（运行时，含 `webview`/`popup`/`icon`/`platform`）。
- 阶段 4：跨 crate 接缝与 CI/构建脚本。
