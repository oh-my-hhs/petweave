# PetWeave — Wayland 桌宠框架

PetWeave 是一个 **Wayland 原生的桌宠框架**：运行时 + SDK 约定，让"做一个桌宠"变成"写一个角色包"，并预置 BongoCat 作为开箱即用的第一个宠物。

## 与现有项目的区别

| | wayland-bongocat | deskpet | Tauri BongoCat | Shijima-Qt（已归档） | **PetWeave** |
|---|---|---|---|---|---|
| 定位 | 单个宠物应用 | 单应用+脚本 | 单应用 | 单个跑器 | **框架/平台** |
| 渲染 | SVG 精灵 | 精灵 | WebView + Live2D | Qt | 精灵（CPU/SHM），Live2D 走 GPU（规划） |
| 内存 | ~8MB | ~18MB | 100MB+ | 重 | **~5MB** |
| 插件/角色包 | 无 | 内置脚本 | 无 | 无 | **角色包格式（规划）** |
| Wayland 支持 | ✅ | ✅ | ❌ 仅 X11 | 充满 hack | ✅ 原生 |

## 创新点

### 1. 框架定位：从"应用"到"平台"
现有项目大多是单一宠物实现。PetWeave 提供统一的**宠物抽象**（事件/渲染/动画调度），宠物作者只写表现层，Wayland、输入、权限、多显示器全部由运行时承担——一个宠物一套代码，处处可跑。

### 2. 极致轻量
纯 Wayland 客户端，无 WebView/GTK/Qt 运行时。空闲时事件循环阻塞在 poll（~0% CPU），实测 **RSS ≈ 5MB**（Tauri BongoCat 的 WebView 是 100MB+）。采用双缓冲 SHM + 预缩放帧缓存 + 按需唤醒的动画调度。

### 3. 开发者体验
- **热重载**：改配置即生效（属性/宠物/尺寸三级应用）
- **调试工具**：`--preview` 导出当前帧为 PNG（无需显示器）、`list-devices` 识别键盘、`doctor` 检测环境
- **权限一键**：`petweave doctor --apply` 安装 udev uaccess 规则

### 4. 解决 Wayland 桌宠的痛点
- **输入权限**：udev `uaccess` 规则 + 检测/安装工具，免手动折腾 `input` 组
- **兼容性**：layer-shell 定位、全屏自动隐藏（多合成器策略）、多显示器按名绑定、HiDPI 缓冲缩放；不支持时**诚实降级**而非崩溃
- **健壮性**：键盘热插拔、进程单例（flock PID）、优雅退出

### 5. 面向未来的互动
- 已实现：按键爪击动画、闲置/定时睡眠、系统感知（CPU/内存事件）
- 规划中：拖拽与物理、多宠物共存、Lua 脚本角色包、AI 接口预留、Live2D 模型支持（复用 BongoCat 模型社区）

## 现状

- **已实现（M0–M1）**：Wayland 渲染管线、BongoCat 爪击移植、热插拔、全屏隐藏、多显示器/HiDPI、睡眠、热重载、系统感知、doctor、单例
- **规划中（M2+）**：`.petweave` 角色包格式、声明式 + Lua 脚本宠物、拖拽/物理/多宠物、Live2D/GPU 后端

## 相关文档

- [docs/TECH_STACK.md](docs/TECH_STACK.md) — 技术选型与架构
- [docs/ROADMAP.md](docs/ROADMAP.md) — 功能实现计划（含完成勾选）
- [docs/LIVE2D.md](docs/LIVE2D.md) — 复杂 BongoCat（Live2D 版）移植路线
