# Web2App

自繁殖单文件跨平台 Web→桌面应用打包器。输入网页 URL，自动抓取 favicon 作为图标，生成单文件桌面应用；产物本身也是母版，可再次打包新应用（自繁殖闭环）。

受开源项目https://github.com/tw93/Pake和小暖科技的SiteNative项目启发。

## 快速开始

```powershell
# 一键构建（测试 + Release + 母版装配 + logo 自举）
.\build.ps1

# 运行母版
.\dist\Web2App.exe
```
<img width="1654" height="1172" alt="image" src="https://github.com/user-attachments/assets/054b657d-88ec-458b-b90b-2335af94ed10" />

母版界面只有两元素：**网页 URL 输入框、打包按钮**（回车也可提交）。点击「打包」，自动获取网页 favicon 注入产物图标，产物生成在母版所在目录（文件名 = URL host，如 `example.com.exe`）。

## 使用产物

- 直接双击运行 → 打开目标网页的桌面应用窗口
- `product.exe --master` → 重新进入打包器界面，继续繁殖新应用

## 构建产物指标

| 指标 | 目标 | 实测 |
|---|---|---|
| 单文件体积 | < 10 MB | 1.75 MB |
| 冷启动 | < 2 s | 秒级 |
| 关闭残留 | 无 | 进程/WebView2 子进程全部退出 |
| 运行副作用 | 无 | 无后台控制台；exe 目录零落盘 |

## 架构

```
src/
├── main.rs      # 唯一入口：--set-icon 工具模式 / --master / 尾部配置分发
├── webview.rs   # 渲染层：wry(WebView2/WKWebView/WebKitGTK) + tao 窗口（深色标题栏）
├── ui.rs        # 母版表单（内嵌 HTML，两元素）
├── builder.rs   # 打包引擎：复制自身 → 剥离旧图标 → 注入 favicon → 追加尾部
├── tail.rs      # 自繁殖协议：<payload JSON> <MAGIC:16B> <len:u64 LE>
├── icon.rs      # 图标域：favicon 获取/解析/组装 + 跨平台注入统一入口
└── pe.rs        # PE 资源注入（仅 Windows 编译，对上层零平台暴露）
```

跨平台策略：同一功能优先同一实现（环境探测链），无法避免的平台差异收敛到 `icon.rs` 的 `embed_icon`/`strip_icon` 单函数适配层——分 case 聚合于一屏，上层调用零感知。

### 自繁殖原理

母版 = 无尾部配置的裸 exe；产物 = 母版 + 尾部配置。尾部协议从文件末尾倒数 24 字节定位，天然支持覆盖旧配置。产物含全部母版能力（复制自身打包新应用），实现「生成即母版」。

### 运行时行为

- **无控制台**：release 构建为 Windows GUI 子系统（`windows_subsystem`），双击运行不出现后台终端。
- **零目录污染**：WebView 用户数据统一放系统应用数据区（Windows `%LOCALAPPDATA%`，macOS `~/Library/Application Support`，Linux `~/.local/share`），exe 旁边不产生任何文件夹。
- **深色标题栏**：窗口主题固定 `Theme::Dark`，与 UI 深色主题一致。

### 图标机制

- **产物图标 = 目标网页 favicon**：打包时请求 `https://<host>/favicon.ico`（10s 超时），支持 ICO 容器（PNG/BMP-DIB 条目）与裸 PNG，转换后注入 PE 资源节。获取失败不阻塞打包，产物保留母版图标。
- **母版图标 = w2a 艺术字**：`assets/logo.png`（深蓝渐变 + 白色 w2a），构建脚本经 `--set-icon` 工具模式自举注入。
- **替换语义**：产物组装时物理剥离母版图标节再注入 favicon（字节级回收，自繁殖多代不膨胀）。

## 待解决问题

1. **跨平台限制（机制性）**：源码可编译出 Windows / macOS / Linux（x64 / ARM64）各平台母版，但自繁殖仅在单一平台内闭环——产物平台恒等于母版平台，无法由一个母版生成其他 OS 或架构的产物；且 Unix 产物缺省无执行权限位、Apple Silicon 强制签名与尾部追加协议冲突，均待处理。

## 开发

```powershell
cargo test --bin web2app    # 42 个单元测试
cargo build --release       # Release（opt-level=z, lto, strip）
```

依赖：`wry 0.57`（devtools only）、`tao 0.37`、`dunce 1`、`png 0.18`、`ureq 2`（favicon 获取）。JSON 手写，零 serde。

