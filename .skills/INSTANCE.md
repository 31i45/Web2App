# INSTANCE.md · 项目实例文件（Web2App）

> 本文件是 .skills 通用方法论在 Web2App 项目的全部实例化：项目专有事实（命令、红线、工具名、平台名、专有清单）只存在于此，skills 本体不内嵌。
> 有效期：下列任一事实变更时需更新
> 最后验证：2026-10-18（创建时逐项核对自 skills 迁出内容与当日实测）

## 1. 真机验证实例（Skill 03 A1）

- 命令：Windows `pwsh .\e2e.ps1`；Linux `bash e2e.sh`（Xvfb）
- 场景：

| 场景 | Windows | Linux |
|---|---|---|
| 打包器启动 | | |
| 打包 URL（无图标） | | |
| 打包 URL（自定义图标） | | |
| 产物启动渲染 | | |
| 关闭零残留 | | |
| 重复打包 | | |
| 裸模板不可运行 | | N/A |

- 抽检：2-3 个真实网站
- 最近全绿：Windows 30/30（2026-10-18 真机）；Linux 37 断言由 CI（ubuntu-22.04）执行，本机无对应环境时按 Skill 03 A1 标注「半」

## 2. 数字红线（Skill 03 A2）

| 数字 | 红线/目标 | 实测（v3.0.3） |
|---|---|---|
| child.exe 体积 | < 1 MiB（CI 硬门禁） | 719,360 B (~702 KiB) |
| web2app.exe 体积 | < 2 MiB（CI 硬门禁） | 1,560,064 B (~1.49 MiB) |
| Linux 打包器裸 ELF | < 2 MiB（build-linux CI 断言，2026-10-18 补设） | 1,944,880 B (~1.85 MiB) |
| 单元测试数 | ≥ 84（tail-core 42 / child 19 / master 23） | 84 |
| e2e 断言数 | Windows ≥ 30 / Linux ≥ 37 | 30 / 37 |

- 版本号一致性：git tag == Cargo.toml workspace version == .deb control Version（master/src/platform/linux.rs `control_text`）
- 版本失配已处置（2026-10-18）：workspace version bump 至 3.0.3；下个 tag 发布时三方一致即闭环（v3.0.2 失配为历史事实）

## 3. 核心不变量断言（Skill 03 A4）

- 产物主体 == child 模板（字节级，master/build.rs 编译期内嵌）
- 尾部帧 == 预期（v3 = 29B 含 CRC32 / v2 = 24B 兼容）
- 重复打包不膨胀（base + payload + 29）
- 覆盖图标后仅 .rsrc 区段变化

## 4. 核心功能清单（Skill 02 B3）

打包流程、尾部读取、窗口关闭、WebView 构建、Profile 目录

## 5. 外部输入点（Skill 02 B1）

URL、图标 PNG、尾部帧、CLI 参数、环境变量（`WEB2APP_SIGN_CERT`）

## 6. 供应链工具（Skill 02 B5）

- 首选：GitHub Dependabot——版本更新已由 .github/dependabot.yml 启用；漏洞告警（vulnerability alerts）仍需仓库 Settings → Security → Dependabot 手工开启
- 备选：`cargo audit`（未安装）、`cargo tree`（audit.ps1 已断言 child 依赖边界）
- 直接依赖 7 个：wry、tao、png、webview2-com、windows-sys、gtk、winres；Cargo.lock 共 283 包

## 7. 平台接缝实例（Skill 01 B3）

- 语言：Rust。机制：`#[cfg]` 同名同签名模块（master/src/platform、child/src/platform），不用 trait
- 决策记录：docs/CROSS_PLATFORM_DESIGN.md §11 ADR-1/ADR-2

## 8. 违禁项清单（Skill 03 A3）

- 词级（CI grep 目标）：

<!-- CI-BANNED-WORDS-BEGIN -->
结构性不可繁殖
自繁殖
non-reproduction
self-reproducing
<!-- CI-BANNED-WORDS-END -->

- 概念级（发布前人工核对）：不得在对外文档描述「不可通过补丁/删参数等方式获得再打包能力」这一机制本身
- 已处置（2026-10-18）：README.md:150 与 README.zh-CN.md:150 弱化——保留「写尾能力隔离于 master crate」的架构理由，移除「防补丁再打包」机制描述
- CI 现状：ci.yml 已含 banned-words job，读取上方标记块（.skills/ 目录需提交入库，否则该 job 报「清单为空」并失败——这是刻意的强制门）

## 9. 环境维度实例（Skill 03 A5）

- OS：Windows 10/11；Zorin 17（构建基线 ubuntu-22.04，glibc 2.35）
- 运行时：WebView2（Win11 内置）；WebKitGTK 4.1 / GTK 3
- 安全软件：Defender ASR（项目目录已加排除规则）
- 已知环境事实：apt 安装本地 .deb 的「_apt 权限拒绝」提示为正常行为；Linux e2e 勿用 dbus-run-session 包裹
