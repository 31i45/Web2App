//! tail-core — Web2App 共享层（纯逻辑层）。
//!
//! 三方共用：尾部协议（child 读取运行配置；**写尾已外迁 `master/src/tail_write.rs`**，
//! 本 crate 只保留解析与 [`encode_tail`] 编码纯函数——写能力不在 child 的依赖图中，
//! 构成编译期结构性保证）、
//! PNG 解码与缩放（child 把用户图与 w2a 默认图转为像素，供 tao Icon 用）、
//! URL 工具（master 产物文件名与 child 数据目录共用 sanitize_host 单点持有）。
//!
//! 设计红线：**零 OS 依赖**（零 tao/wry、零 Win32/GTK/Cocoa），可任意宿主单测。
//! 全部平台外壳代码（错误呈现 / 关闭看门狗 / 数据目录 / 路径规范化）都在
//! `child|master` 的 `platform/` 同名模块中，本 crate 仅为「纯数据层」。
//! 窗口图标转换在 child/icon.rs（依赖 tao::window::Icon），此处只到像素字节为止。

mod image;
mod tail;
mod text;
mod url;

pub use image::*;
pub use tail::*;
pub use text::*;
pub use url::*;
