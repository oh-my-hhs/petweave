# 复杂 BongoCat（Live2D 版）如何继续

> 针对 `Reference/BongoCat`（ayangweb/BongoCat，Tauri 2 + Vue + Pixi.js + Live2D Cubism）
> 的移植路线设计。M1 已完成"轻量版"（PNG 爪击，`runtime/builtin/bongo.rs`），
> 本文回答：**完整的 Live2D BongoCat 如何在 PetWeave 上落地**。

## 1. 差距分析：复杂版比轻量版多什么

| 能力 | 轻量版（M1 已完成） | 复杂版（Tauri BongoCat） | PetWeave 现状 |
|---|---|---|---|
| 渲染 | PNG 精灵帧 | Live2D Cubism（GPU 骨骼变形） | 需 wgpu 后端 |
| 模型 | 固定 4 帧 | 任意 Live2D 模型（moc3+贴图+动作） | 需模型加载器 |
| 动画 | 爪击状态机 | 动作(motion)/表情(expression)/物理(physics3) | 需动作系统 |
| 输入 | 键盘 | 键盘 + 鼠标 + **手柄(gamepad)** | 需手柄事件 |
| 界面 | 无 | 模型管理/行为配置/快捷键设置页 | 需设置 UI |
| 社区资产 | 无 | Awesome-BongoCat 模型社区、模型转换工具 | 需包格式兼容 |

## 2. 技术路径（分四块）

### 2.1 渲染后端：wgpu + linux-dmabuf（RenderBackend v2）

- 现状：`RenderBackend` trait 只有 `present(&Frame)`（CPU 像素路径）。
- 演进：新增 GPU 路径 —— 宠物不再只产出 `Frame`，而是产出一张 GPU 纹理
  （`wgpu::Texture`），由后端经 `zwp_linux_dmabuf`/`wl_drm` 呈现到 layer surface。
- 后端自动选择：**精灵宠物走 CPU/SHM，Live2D 宠物走 GPU**，互不影响。
- 现实约束：`wgpu` 28 已在本地离线缓存中（连同 `wgpu-core`/`wgpu-hal`），
  dmabuf 呈现路径可先行验证；`naga` 等传递依赖需确认缓存齐全。

### 2.2 Live2D 运行时

- **Cubism Core**（闭源 `.so`，官方免费许可）是唯一运行时依赖：
  - 路线 A（首选）：Rust 绑定 crate（`live2d-rs` / `cubism-core`），需网络拉取；
  - 路线 B（离线兜底）：手工 FFI `dlopen("libLive2DCubismCore.so")`，
    模型文件格式公开、可直接解析 `model3.json` + `moc3` + 贴图 + `motion3.json`。
- 模型即资产：Cubism 模型目录结构是**标准布局**（`Reference/BongoCat/src-tauri/assets/models/`
  下的 standard/keyboard/gamepad 三套可直接复用），放入 `.petweave` 包的
  `assets/live2d/` 即可，**无需转换**。
- 动作系统：`motion3.json` 描述动作，`exp3.json` 表情，`physics3.json` 物理；
  行为配置（哪个按键触发哪个动作/表情）做成 `pet.toml` 里的声明式 `[live2d.behaviors]`，
  对齐 BongoCat 的"行为配置"交互。

### 2.3 输入扩展：手柄与指针

- `Event::Input` 目前只承载键盘 `code`。扩展：
  - 手柄：读 `/dev/input/js*`（或 evdev 的 EV_KEY/EV_ABS），新增
    `Event::Gamepad(GamepadEvent { button, axis, pressed })`；
    键位→爪子的映射直接复用 `paw_for_keycode` 的思路。
  - 指针：wl_seat pointer 事件（悬停/点击/拖拽），新增 `Event::Pointer`。
- 这样"键盘/鼠标/手柄同步动作"的能力与 Tauri 版对齐，且事件模型保持不变
  （宠物只订阅事件）。

### 2.4 设置 UI 与包管理

- 设置面板：**egui**（进程内，可选独立进程），实现 BongoCat 的
  模型管理（导入/切换/删除）、行为配置、快捷键、开机自启。
- 包管理 CLI：`petweave install awesome-model.petweave` —— 复杂 BongoCat 以
  **角色包**形态分发，而非单独的应用。

## 3. 落地阶段

| 阶段 | 内容 | 依赖 |
|---|---|---|
| L1 渲染地基 | wgpu 后端 + dmabuf 呈现 + 纹理型 RenderBackend | 网络拉 `live2d-rs` 或离线 FFI |
| L2 模型加载 | model3 解析、moc3 加载、贴图、渲染一帧静态模型 | L1 |
| L3 动作系统 | motion/expression/physics 驱动 + 行为配置表 | L2 |
| L4 完整对齐 | 手柄事件、指针互动、egui 设置面板、`.petweave` 打包 | L3 + M2 包格式 |
| L5 社区 | Awesome-BongoCat 模型批量导入、模型商店 | L4 |

## 4. 风险与对策

| 风险 | 对策 |
|---|---|
| Cubism Core 闭源、许可限制（免费但非开源，不可再分发 SDK） | 框架核心不依赖它：作为**资产级运行时依赖**（宠物包内声明）；文档明确许可 |
| wgpu Wayland dmabuf 在部分合成器上的成熟度 | 后端按需启用、降级回 SHM；兼容矩阵记录 |
| GPU 路径内存/功耗高于 CPU 路径 | 仅 Live2D 宠物启用 GPU；闲置时暂停渲染循环 |
| 模型资产体积（贴图大） | 包格式支持懒加载/按需解码 |

## 5. 与现有架构的衔接

- `Pet` trait 增加可选的 GPU 渲染钩子（默认走 `render(Frame)`，Live2D 宠物实现
  `render_gpu(&mut self, ctx: &mut GpuContext)`）。
- `Runtime` 按宠物类型选择后端；多宠物混合（PNG 猫 + Live2D 猫）互不干扰。
- 所有新能力都落在 M4 里程碑，M2 的角色包格式先行（`assets/live2d/` 目录约定）。
