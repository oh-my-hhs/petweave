# PetWeave 功能实现计划表

> 依据 `docs/TECH_STACK.md` 的里程碑划分。`- [x]` 表示已完成，`- [ ]` 表示未完成。
> 更新日期：2026-08-16（M0 骨架完成，进入 M1）

## 当前状态

| 里程碑 | 状态 | 说明 |
|---|---|---|
| M0 项目骨架 | ✅ 完成 | 编译通过、13 项单测通过、niri 上实测可显示 |
| M1 BongoCat 移植 | 🔄 进行中 | 下一步：SVG 渲染 + 爪击动画状态机 |
| M2 SDK / 角色包 | ⏳ 未开始 | |
| M3 交互升级 | ⏳ 未开始 | |
| M4 深度集成 | ⏳ 未开始 | |

---

## M0 项目骨架

- [x] Cargo workspace 布局（`crates/petweave-core` 共享类型 + `crates/petweave` 主机）
- [x] Wayland 连接 + registry + `wlr-layer-shell` surface（niri 实测：连接成功、surface 建立、正常退出）
- [x] SHM 双缓冲 CPU 渲染管线（RGBA `Frame` → BGRA blit → attach/damage/commit）
- [x] 配置系统：TOML + 全默认值 + 校验 + CLI 覆盖（`petweave.toml.example`）
- [x] CLI：`--config / --list-devices / --width / --height / --fps / --device / --no-auto-input / --verbose`
- [x] evdev 键盘发现与读取线程 → calloop 事件总线（`Event::Input`）
- [x] 首个宠物：demo pet（按键时闪白），打通 输入→事件→状态→渲染→呈现 全链路
- [x] SIGINT/SIGTERM 优雅退出（calloop signals）
- [x] 单元测试 13 项（配置解析/校验、Frame 操作、BGRA 转换、锚点/层级解析、颜色解析）
- [x] 示例配置 `petweave.toml.example`
- [x] 离线构建支持（`.cargo-home` 符号链接只读 registry 缓存，见 README）
- [ ] `petweave doctor` 权限体检工具（udev uaccess 一键安装）
- [ ] PID 文件单例 + `$XDG_RUNTIME_DIR` 安全落盘
- [ ] 发布加固：LTO + strip + `panic=abort`，测量二进制体积

## M1 BongoCat 移植（目标：在 niri/Hyprland 复现 wayland-bongocat 体验）

- [ ] SVG 渲染（resvg）+ 启动时预缩放帧缓存（继承 wayland-bongocat 方案）
- [ ] BongoCat 爪击动画状态机：左右手键位映射、双爪并发、按键保持时长
- [ ] 动画帧率控制与空闲休眠（帧回调驱动 + eventfd 式唤醒，空闲 ~0% CPU）
- [ ] 键盘热插拔：5s 快速重试 → 30s 周期扫描
- [ ] 全屏自动隐藏：`wlr-foreign-toplevel-management` / `ext-foreign-toplevel-list` + KWin 兜底
- [ ] 多显示器：`xdg-output` 按名称定位、HiDPI 逻辑/物理尺寸换算
- [ ] 闲置/定时睡眠模式
- [ ] 配置热重载三段式：属性级 → 缓冲区级 → 全重建
- [ ] 系统感知：`sysinfo` CPU/内存 → `Event::System`，宠物可反应
- [ ] `petweave doctor` 权限工具（从 M0 移入，随 M1 一并完成）
- [ ] PID 文件单例（从 M0 移入）
- [ ] 资源指标基线：单宠物 RSS ≤ 25MB、空闲 CPU ≈ 0、启动 < 100ms

## M2 SDK / 角色包（核心差异化：从"应用"到"平台"）

- [ ] `.petweave` 角色包格式：zip + `pet.toml`（元数据/许可证/缩略图）+ assets + 签名
- [ ] 声明式宠物运行时：精灵表 PNG + 动作表（走/待机/点击反应），零代码
- [ ] 经典资产导入器：Oneko 精灵表、Codex 8×9/8×11 图集
- [ ] Lua 脚本运行时（mlua 沙箱：白名单 API、资源上限）
- [ ] Lua API v1：事件（`on_key/on_pointer/on_tick/on_fullscreen/on_system`）+ 动作（`play/move_to/speak`）+ 查询（`sys.*/focus.*`）
- [ ] `petweave install/uninstall/list` CLI + 本地角色包仓库
- [ ] 角色包制作教程文档
- [ ] 预置包：BongoCat（SVG 资产）、Oneko（精灵表）

## M3 交互升级

- [ ] 指针事件接入（wl_seat）：悬停/点击反应
- [ ] 拖拽：指针按下→margin 节流移动，记录各合成器体验
- [ ] 轻物理：重力 + 桌面边缘碰撞 + 弹性衰减（自研，不引引擎）
- [ ] 多宠物共存：进程内多实例（共享 Wayland 连接与资产缓存）
- [ ] 点击穿透：`wl_surface.set_input_region` 按宠物形状收窄
- [ ] 托盘图标（ksni）+ 设置面板（egui，独立进程可选）

## M4 深度集成

- [ ] wgpu + linux-dmabuf GPU 渲染后端（`RenderBackend` 第二实现）
- [ ] Live2D 支持（Cubism Core Rust 绑定）+ BongoCat 模型社区兼容
- [ ] 外部宠物进程协议：Unix socket JSON-RPC，任意语言可写宠物
- [ ] 隔离沙箱：外部宠物进程 seccomp/landlock（可选）
- [ ] AI 预留落地：`on_message/speak` 接口 + CATAI/Sakura 适配示例
- [ ] 打包分发：Arch PKGBUILD + Nix flake（含 udev 规则自动安装）

## 横切事项

- [ ] 合成器兼容矩阵实测：niri / Hyprland / Sway / KWin(Plasma 6) / GNOME(受限模式)
- [ ] 输入权限文档 + NixOS 模块
- [ ] 性能基线持续测量（每里程碑回归）
- [ ] 隐私审计：确认键码默认不落盘、debug 开关显式化

---

## 备注

- M0 中"权限工具/PID 文件"标注为未完成：骨架优先，两项随 M1 一并落地更合适。
- 部分 M0 项（多显示器、全屏隐藏等）设计上依赖协议绑定，统一在 M1 引入
  `wayland-protocols(-wlr)` 的 foreign-toplevel 与 xdg-output 模块。
- **M0 资源基线（实测）**：demo pet + 256×256 surface 运行时 RSS ≈ **4.6 MB**
  （niri，release 构建），空闲时事件循环阻塞在 poll（~0% CPU）——已优于
  wayland-bongocat 的 8MB 与 deskpet 的 18MB 目标。
