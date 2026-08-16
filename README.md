# PetWeave

一个面向 Wayland 的桌宠**框架**（host runtime + 未来的角色包 SDK），目标是让"移植 BongoCat / Oneko"变成"安装角色包"。

设计文档：[docs/TECH_STACK.md](docs/TECH_STACK.md) · 实现计划：[docs/ROADMAP.md](docs/ROADMAP.md)

## 当前状态（M0 骨架 ✅）

- Rust workspace：`petweave-core`（配置/事件/Pet trait/Frame）+ `petweave`（主机）
- Wayland：`wlr-layer-shell` surface + SHM 双缓冲 CPU 渲染
- 输入：evdev 全局键盘监听（读线程 → 事件总线）
- 首个宠物：demo pet（按键闪白），打通全链路
- niri 上实测可运行，13 项单元测试通过

## 构建

```bash
cargo build --release          # 或 cargo build / cargo test
./target/release/petweave --list-devices   # 查看可用的键盘设备
./target/release/petweave                  # 运行（默认配置）
./target/release/petweave -c petweave.toml.example --width 200 --height 200
```

### 离线构建说明（本机）

本机网络不可用且系统 cargo 缓存为只读。已配置 `.cargo-home/`（gitignore）：符号链接到
只读的 `~/.cargo/registry/{cache,index}`，`src` 为可写目录。使用：

```bash
CARGO_HOME=/home/hhs/Projects/petweave/.cargo-home cargo build --offline
```

### 运行依赖

- Wayland 合成器（支持 `wlr-layer-shell`）：niri / Hyprland / Sway / KWin(Plasma 6) 等
- 键盘监听需要 `/dev/input` 权限：udev `uaccess` 规则或 `input` 组
  （`petweave doctor` 工具在 M1 提供一键配置）

## 布局

```
crates/
  petweave-core/   共享类型：config / events / pet trait / render Frame
  petweave/        主机：cli / app(事件循环) / platform{wayland,input} / graphics / runtime
docs/              技术栈分析 + 功能实现计划（带完成勾选）
petweave.toml.example
```
