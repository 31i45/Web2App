# 架构评审 · 阶段 4：跨 crate 接缝与 CI/构建脚本

> 方法论：`codebase-design` 深模块词汇（module / interface / depth / seam / adapter / leverage / locality）+ `improve-codebase-architecture` 流程。
> 约束：不修改任何代码，仅产出分析与建议。
> 范围：跨 crate 接缝（`tail-core` ↔ `child` / `master`）+ CI/构建脚本（`build.ps1` / `build.sh` / `audit.ps1` / `e2e.ps1` / `e2e.sh` / `.github/workflows/ci.yml`）。

## 1. 跨 crate 接缝全景

### 1.1 依赖图

```
          ┌──────────────────────────────────┐
          │           tail-core              │
          │  (纯逻辑层：零 OS 依赖)           │
          │  tail · text · image · url       │
          └───────────────┬──────────────────┘
                          │
              ┌───────────┴───────────┐
              ▼                       ▼
        ┌──────────┐           ┌──────────┐
        │  child   │           │  master  │
        │ (运行时) │           │ (打包器) │
        └──────────┘           └──────────┘
              │                       │
              │  read_tail()          │  strip_tail / encode_tail
              │  host_of              │  read_tail_from
              │  decode_png           │  tail_write.rs（写能力隔离点）
              │  sanitize_host        │  decode_png
              │  default_title        │  sanitize_host
              │  js_string_literal    │  default_title
              └───────────────────────┘
```

**核心架构红线**：`tail-core` 保留 `encode_tail`（纯函数，两边均可调用），但**文件写入能力**（`tail_write.rs`）只存在于 `master`。`child` 的依赖图物理不含 `write_tail`——`audit.ps1` 第 4 项 `cargo tree` 断言机械守护此边界。

### 1.2 tail-core 18 项 pub interface 的调用方分布

| 域 | pub 项 | child caller | master caller | 接缝性质 |
|---|---|---|---|---|
| tail | `AppConfig` | main, webview | builder, tail_write | 配置数据结构（双向） |
| tail | `read_tail()` | main（1 处） | — | 子应用启动读自身尾部 |
| tail | `read_tail_from()` | — | builder, tail_write(测试) | 从任意路径读（master 专属） |
| tail | `encode_tail()` | — | tail_write | 编码纯函数（写能力前置） |
| tail | `strip_tail()` | — | builder, tail_write | 剥离尾部（打包前置去重） |
| tail | `current_exe()` | (read_tail 内部) | main | 可执行路径定位 |
| tail | `TAIL_MAGIC` / `TAIL_V3_FOOTER_LEN` | — | tail_write | 协议常量 |
| text | `js_string_literal` | webview（注入脚本） | main（表单反馈） | JS 安全转义（双向） |
| text | `base64_encode` / `base64_decode` | — | ui（图标 IPC） | 仅 master 用 |
| image | `decode_png` / `resize_box` | icon | icon, winicon | 图标像素处理（双向共享） |
| url | `host_of` | popup, webview, platform/{win,linux} | — | 运行时 host 提取（child 专属） |
| url | `sanitize_host` | webview（数据目录） | ui（exe 名）, platform/linux（deb 包名） | 文件名安全（双向，但用途不同） |
| url | `default_title` | platform/{win,linux} | builder, platform/linux | 窗口标题（双向） |
| url | `url_fingerprint` | webview（数据目录隔离） | — | child 专属隔离指纹 |

### 1.3 接缝设计亮点

**① 写能力的结构性隔离（深接缝）**

`encode_tail` 留在 `tail-core` 是有意的——它是纯函数（`&[u8] -> Vec<u8>`），不触及文件系统。`tail_write.rs`（`append_tail_to_file`）是**唯一**接触磁盘写入的模块，且只在 `master`。这意味着：
- `child` 即使被攻破，其二进制内不存在「向自身追加尾部」的代码路径——繁殖能力被编译期物理移除。
- `audit.ps1` 第 1/3/4 项从三个维度守护：字符串标记扫描、PE/ELF 映像计数、依赖树边界。

**② 数据目录 vs 产物文件名的有意分化**

同源于 `sanitize_host`，但：
- `child::webview_data_dir`：`sanitize_host(url) + "_" + url_fingerprint(url)`——加指纹防同 host 多路径冲突，数据隔离。
- `master::ui::product_filename`：`sanitize_host(url) + ".exe"`——不加指纹，产物文件名冲突由用户目录解决。

这是**同一接口、不同策略**的正确用法——`sanitize_host` 提供「安全文件名」基础能力，调用方按场景决定是否加指纹。

**③ 图标处理的双层共享**

`decode_png` / `resize_box` 在 `tail-core`（纯像素，零 tao 依赖）；`child/icon.rs` 与 `master/icon.rs` 各自做 `PNG 字节 → tao::window::Icon` 转换（tao 依赖不能进纯逻辑层）。两份 `icon.rs` 几乎同构，但因 `child` 是二进制 crate 无 lib 导出、`master` 不依赖 `child`，合并需新增共享 crate——为 ~30 行逻辑新增 crate 不划算。符合「**两个 adapter 才是真实接缝**」原则。

## 2. CI / 构建脚本评估

### 2.1 脚本矩阵与职责

| 脚本 | 平台 | 职责 | 门禁级别 |
|---|---|---|---|
| `build.ps1` / `build.sh` | Win / Linux | 5 步构建（child→test→clippy→master→装配） | 本地 + CI 调用 |
| `audit.ps1` | 跨平台（单脚本分支） | 4 项结构性审计 | CI 门禁 |
| `e2e.ps1` | Win | 7 项端到端（静态身份+产物链路+零残留+裸模板+母版+字节级） | 手动工具（非 CI） |
| `e2e.sh` | Linux | 4 项无头门禁（按键矩阵+data 隔离+deb+GUI 打包） | CI 门禁（G-1） |
| `ci.yml` | — | 5 job 流水线（verify-win / verify-linux / build-win / build-linux / release） | CI 总控 |

### 2.2 逐脚本深度评估

#### build.ps1 / build.sh

- **接口**：3 参数（SkipTests / SkipClippy / OutDir），5 步固定流程。
- **深度**：浅（流程编排，无业务逻辑）。
- **设计点**：
  - 两阶段构建（child 先于 master）是 `master/build.rs` 编译期内嵌 child 模板的架构约束，脚本强制显式先行，避免 `cargo test` 并行调度的竞态。
  - PowerShell 用 `-SkipTests` 具名开关，bash 用 `--skip-tests` 长选项——符合各平台习惯，不算不一致。
  - `OutDir` 在 ps1 是具名参数，在 sh 是位置参数——心智模型略不同，但文档已说明。

#### audit.ps1

- **接口**：2 参数（ChildPath / MasterPath），4 项检查。
- **深度**：中深。
- **设计点**：
  - **正向对照是关键**：第 2 项断言 master 必含标记、master 必含 ≥2 个 PE/ELF 映像——证明扫描器本身有效，否则「child 不含」可能是扫描空转的假阳性。这是审计脚本的红绿可信前提。
  - **PE/ELF 映像计数不用裸魔数**：机器码里会偶然命中 `MZ` 或 `\x7fELF`，必须用「魔数 + 指向结构的指针落界」（`e_lfanew` 指向 `PE\0\0`、`e_shoff` 指向完整节头表）才与「真实映像」同义。
  - **平台判定用 `[System.Environment]::OSVersion`** 而非 `$IsWindows`——Windows PowerShell 5.1 未定义 `$IsWindows`，引用静默得 `$null` 会走错分支。
  - **Latin1 编码做字节级子串搜索**：`[Text.Encoding]::GetEncoding(28591)` 使字节↔字符一一映射，Ordinal 搜索等价于原始字节流搜索，无编码规范化损失。

#### e2e.ps1 / e2e.sh

- **接口**：e2e.ps1 2 参数（MasterPath / ChildPath）；e2e.sh 环境变量（CHILD / MASTER）。
- **深度**：深。
- **设计点**：
  - **尾部协议独立实现（漂移点）**：`e2e.ps1::New-Product` 和 `e2e.sh` 的 python 内联都**手工**实现了 v3 尾部布局 + CRC32，与 `tail-core::encode_tail` 是**两份独立实现**。这是有意的——黑盒交叉校验：若 `tail-core` 协议实现有 bug，独立实现的 e2e 仍能捕获（前提是两份不同时错）。
  - **漂移点已标注**：两脚本头注均明确「布局常量以 `tail-core::TAIL_V3_FOOTER_LEN` 为准，此处是漂移点」。
  - **Windows e2e 含内联 C# `ResCheck`**：`LoadLibraryEx(AS_DATAFILE) + EnumResourceNames` 探测 `RT_GROUP_ICON`——`ExtractAssociatedIcon` 对无资源 exe 会回退通用图标，无法区分，必须枚举资源。
  - **Linux e2e 的 `_NET_WM_PING` 探针**：验证 GUI 打包期间主循环响应性（打包在事件循环内同步执行会阻塞 GDK，被桌面会话判死）。判据是「最大连续未回应 < 10」，且只在 staging 目录存在（打包确实进行中）时采样——避免完成边界的空闲回应污染。
  - **关闭路径用真实关闭消息**：Windows 用 `CloseMainWindow`（发 `WM_CLOSE`），Linux 用 `send_wm_delete`（发 `WM_DELETE_WINDOW`），而非强杀——验证关闭兜底机制（WNDPROC 哨兵 + 看门狗线程）真实生效。

#### ci.yml

- **接口**：5 job，3 触发条件（push main / push tags / PR）。
- **深度**：中深。
- **设计点**：
  - **concurrency 按 ref 分组，tag 不取消**：同一 ref 新推送取消旧运行，但 tag 发布运行保留（保证发布完整性）。
  - **最小权限**：默认 `contents: read`，release job 单独提升 `contents: write`。
  - **`download-artifact@v4` 用 `merge-multiple: true`**：否则多产物放入独立子目录，后续 `sha256sum` 找不到文件（已踩坑）。
  - **release 用 GitHub API 而非 `gh release create`**：`gh release create --generate-notes` 在 ubuntu runner 静默失败（无输出、不创建、exit 0），换 curl + python3 显式报错。
  - **基线钉 ubuntu-22.04**：ubuntu-latest（24.04）构建的二进制带 `GLIBC_2.39` weak 引用，在 jammy 系目标（Zorin 17 / 国产桌面 OS）上启动刷警告。22.04 产物在 22.04+ 全系可跑。
  - **两阶段构建显式先行**：CI 中 `cargo build -p child` 先于 `cargo build --workspace`，与本地 `build.ps1` 同口径。

## 3. 深化候选

### 候选 1：e2e 脚本尾部协议实现的漂移点管理

- **Files**：`e2e.ps1`（`New-Product` + `Get-Crc32`）、`e2e.sh`（python 内联尾部构造）
- **Problem**：e2e 脚本各自手工实现 v3 尾部布局 + CRC32，与 `tail-core::encode_tail` 是两份独立实现。若 `tail-core` 协议变更（如 v4），e2e 脚本需同步修改，否则产生假阴性（e2e 构造的产物与生产格式不匹配）。
- **分析**：
  - 独立实现本身是**有意的交叉校验**——生产实现有 bug 时，独立实现的 e2e 仍能捕获（前提是两份不同时错）。共享代码会失去此价值。
  - 当前漂移点已在头注标注，但**无机械校验**保证两份实现同步——若有人改 `tail-core` 协议但忘改 e2e，CI 会失败但错误信息不直观（e2e 构造的产物读不出尾部）。
- **Solution（不修改代码，仅建议）**：在 `tail-core` 的协议常量旁加一条注释，指向 e2e 脚本的对应位置，形成「改协议须同步两处」的显式心智链。或在 `audit.ps1` 中增加一项：用 `tail-core` 的 `encode_tail` 生成测试帧，验证 e2e 脚本生成的帧字节一致——但这要求 e2e 脚本可被调用，当前不可行。
- **当前状态**：漂移点已标注，CI 失败可发现不同步（e2e 产物读不出尾部），但根因定位需人工。
- **Recommendation strength**：`Speculative`（当前标注足够，机械校验成本 > 收益）。

### 候选 2：`audit.ps1` 标记列表的硬编码维护

- **Files**：`audit.ps1`（`$forbidden` 数组 L108-111）
- **Problem**：禁止标记列表（`UpdateResourceW` / `BeginUpdateResourceW` / `EndUpdateResourceW` / `signtool` / `WEB2APP_SIGN_CERT`）是硬编码字符串数组。若 `master` 新增写资源 API（如 `CryptSignMessage`），需手动加进列表，否则 child 若意外引入该 API 不会被捕获。
- **分析**：
  - 显式枚举禁止项是审计的本质——黑名单匹配的覆盖率取决于列表完整性。
  - 反向思路：扫描 `master` 的导入表，自动收集所有「写资源/签名」相关 API，然后断言 child 不含其中任何一个。但「写资源/签名相关」的判定本身需要规则，仍需人工维护判定逻辑。
  - 当前列表覆盖了项目实际使用的全部写资源 API（`UpdateResourceW` 系列 + `signtool` + 环境变量 `WEB2APP_SIGN_CERT`），且第 2 项正向对照证明扫描器对这些标记有效。
- **Conclusion**：保持显式枚举。新增写能力时同步更新列表是合理的人工成本。
- **Recommendation strength**：`Speculative`（不建议自动化，显式列表更清晰）。

### 候选 3：`build.ps1` 与 `build.sh` 的参数风格差异

- **Files**：`build.ps1`、`build.sh`
- **Problem**：`-SkipTests` vs `--skip-tests`、`-OutDir` 具名 vs 位置参数——跨平台心智模型略不同。
- **分析**：
  - PowerShell 具名开关 + bash 长选项是各平台惯例，强行统一（如 bash 也用具名 `--out-dir`）反而违背 bash 用户习惯。
  - `OutDir` 在 sh 是位置参数，文档已说明「`*` 匹配非选项参数即 OUT_DIR」。
- **Conclusion**：保持现状。平台惯例优先于跨平台一致性。
- **Recommendation strength**：不适用（排除）。

### 候选 4：CI `verify` job 中 test 与 build 的重复

- **Files**：`ci.yml`（verify job L42-50）
- **Problem**：`verify` job 先跑 `cargo test --workspace`，再跑 `build.ps1 -SkipTests`。test 阶段已编译 debug 产物，build.ps1 又编译 release——两次编译。
- **分析**：
  - 这是合理的：test 用 debug 产物（含断言），build.ps1 用 release 产物（审计/体积红线对象）。两者目标不同，不能合并。
  - `build.ps1 -SkipTests` 明确跳过 test，避免重复跑测试。
- **Conclusion**：保持现状。debug test + release build 是正确的 CI 分层。
- **Recommendation strength**：不适用（排除）。

### 候选 5：`e2e.sh` 的 `_NET_WM_PING` 探针是否应提取为独立工具

- **Files**：`e2e.sh`（`ping_once` 函数 L95-147）
- **Problem**：`ping_once` 是 ~50 行 python 内联，实现 `_NET_WM_PING` 客户端消息发送 + 回执等待。逻辑复杂（Xlib ctypes 绑定 + 事件循环解析），嵌在 e2e 脚本中可读性差。
- **分析**：
  - 该探针是「主循环响应性」验证的核心，是 e2e 独有的需求（单测无法覆盖 GUI 响应性）。
  - 提取为独立 python 脚本会增加仓库文件，但提升可读性和可测试性。
  - 但当前内联实现已在注释中详细说明踩坑记录（首版误把 message_type 与 `_NET_WM_PING` 原子比较、缺省 argtypes 导致 64 位指针截断段错误），这些上下文在独立文件中会丢失。
- **Conclusion**：保持内联。踩坑注释是该实现的重要组成部分，与代码同处更易维护。
- **Recommendation strength**：`Speculative`。

## 4. Top Recommendation

**跨 crate 接缝与 CI/构建脚本整体成熟，无值得立即执行的深化项。**

理由：
1. **写能力隔离是项目核心架构决策**，已通过 `tail_write.rs` 物理位置 + `audit.ps1` 三重审计（标记扫描 / 映像计数 / 依赖树）机械守护，接缝设计清晰。
2. **e2e 脚本的尾部协议独立实现是有意的交叉校验**，漂移点已标注，CI 失败可发现不同步——当前管理成本可接受。
3. **CI 流水线的设计决策（concurrency / 最小权限 / merge-multiple / API 替代 gh / ubuntu-22.04 基线）均有明确的踩坑背景**，每个非显然选择都有注释支撑，属经验驱动的稳定状态。
4. **build/audit/e2e 三脚本职责正交**：build 只管构建编排，audit 管静态结构，e2e 管行为链路——无职责重叠。

跨 crate 接缝是项目中**设计意图最清晰、机械守护最完备**的部分——「结构性不可繁殖」从架构约束落地为 CI 门禁，形成闭环。

## 5. ADR 冲突检查

`docs/adr/` 尚无文件。本次评审无已记录决策需回避。

## 6. 全阶段总结

| 阶段 | 范围 | 值得执行的候选 |
|---|---|---|
| 1 | tail-core | 候选 1：base64 测试归位到 `text.rs`（Worth exploring） |
| 2 | master | 候选 1：提取 `default_output_path(url)` 消除 CLI/GUI 重复（Worth exploring） |
| 3 | child | 无（架构整体成熟） |
| 4 | 跨 crate + CI | 无（接缝与流水线设计稳定） |

**两个 Worth exploring 候选**：
1. `tail-core` 的 base64 测试从 `tail.rs` 迁到 `text.rs`——纯测试归位，零行为风险。
2. `master` 提取 `default_output_path(url)`——消除 CLI/GUI 路径计算重复，零行为风险。

两者均为低风险、高 locality 收益的重构，可单独执行，互不依赖。
