<p align="center"><img src="apps/desktop/assets/icons/app-128.png" width="96" alt="PoE Toolkit"></p>

# PoE Toolkit

为 Path of Exile 1 / 2 提供行情查询、物价标注和补丁管理的桌面应用。

- **双游戏工作区**：独立保存客户端、服区、赛季与标注方案。
- **自动发现客户端**：识别国服、国际服及 Steam、WeGame、Epic、独立客户端。
- **行情与标注**：查看报价、预览文字变更，按方案应用价格标注。
- **备份与恢复**：写入前备份，支持撤销操作和中断恢复。
- **桌面体验**：Windows / macOS 系统材质、深浅色、托盘与应用内更新。

## 安装

从 [Releases](https://github.com/DINGDANGMAOUP/poe2_toolkit/releases) 下载最新内测版。

| 平台 | 安装包 |
| --- | --- |
| Windows x64 | `windows-x86_64-…-Setup.exe` |
| macOS Apple Silicon | `macos-aarch64-….pkg` |
| macOS Intel | `macos-x86_64-….pkg` |

ZIP 为便携包，需完整解压。后续可在设置中检查更新，下载完成后点击标题栏“重启更新”。

## 使用

1. 选择 PoE1 或 PoE2，连接客户端并选择市场与赛季。
2. 刷新行情，选择需要的标注功能，查看补丁预览。
3. 退出游戏后应用补丁；需要撤销时进入操作记录。

客户端资源不匹配或报价条件不足时，应用会提示原因并停止写入。物价标注只修改游戏文字资源，不替换字体文件。

## 从源码构建

需要 Rust 工具链及对应平台的 C/C++ 构建工具；Windows 使用 MSVC，macOS 使用 Xcode Command Line Tools。

```sh
cargo build --workspace --release --locked
cargo run --release --locked -p poe2-desktop --bin poe2-toolkit
```

应用、核心逻辑、资源格式和服务分别位于 `apps/` 与 `crates/`；`.github/` 管理自动构建、打包和签名更新源。依赖许可证随安装包分发。
