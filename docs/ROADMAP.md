# PetWeave 功能实现计划表

> 依据 `docs/TECH_STACK.md` 的里程碑划分。`- [x]` 表示已完成，`- [ ]` 表示未完成。
> 更新日期：2026-08-16（M0 骨架完成，进入 M1）

## 当前状态

| 里程碑 | 状态 | 说明 |
|---|---|---|
| M0 项目骨架 | ✅ 完成 | 编译通过、单测通过、niri 上实测可显示 |
| M1 BongoCat 移植 | ✅ 完成 | 爪击动画/热插拔/全屏隐藏/多显示器/睡眠/热重载/系统感知/doctor/单例 |
| M2 SDK / 角色包 | ✅ 完成 | 包格式/声明式精灵宠物/Lua 沙箱运行时/CLI/示例包/SVG 渲染 |
| M3 交互升级 | ⏳ 未开始 | |
| M4 深度集成 | ⏳ 未开始 | Live2D 路线已设计（`docs/LIVE2D.md`） |

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

- [x] 精灵渲染 + 启动时预缩放帧缓存（`image` 解码 PNG → `Frame`；SVG/resvg 路线
      待网络可用后替换，见 `docs/LIVE2D.md`）
- [x] BongoCat 爪击动画状态机：左右手键位映射（与 wayland-bongocat 同表）、
      双爪并发、按键保持时长（`keypress_duration_ms`）、`mirror_x`
- [x] 动画驱动与空闲休眠：`Pet::tick` + `next_deadline` 自适应睡眠（空闲阻塞
      在 poll，~0% CPU）
- [x] 资源指标基线：单宠物 RSS ≈ **5.7MB**（niri，release，264×110 surface）
- [x] 键盘热插拔：5s 快速重试 → 30s 周期扫描（`input.rs` 管理器线程），
      设备过滤（字母键集合校验，排除音频/媒体按键设备）
- [x] 全屏自动隐藏：`wlr-foreign-toplevel-management`（activated+fullscreen
      状态、按输出判定 + KDE 式全局兜底）；layer=overlay 或
      `disable_fullscreen_hide` 时跳过。注：niri 未实现该协议，优雅降级
- [x] 多显示器：`xdg-output` 按名称绑定 surface（`render.output`），HiDPI
      buffer_scale 整数缩放（逻辑尺寸 → 物理缓冲）
- [x] 闲置/定时睡眠模式：`idle_sleep_timeout_secs` + `enable_scheduled_sleep`
      （HH:MM 窗口）；睡眠帧由 idle 帧调暗合成（等 SVG 资产后替换）
- [x] 配置热重载三段式：属性级（layer/anchor/margins）→ 缓冲区级（尺寸）→
      宠物级（bongo 参数/cat_height）；kind/output 变更提示重启
- [x] 系统感知：`sysinfo` 定时采样 → `Event::System`（CPU/内存快照）
- [x] `petweave doctor` 权限工具（输入权限检测 + udev uaccess 规则安装 `--apply`）
- [x] PID 文件单例：flock 型 `$XDG_RUNTIME_DIR/petweave.pid`（防 stale 竞态）
- [x] `--preview` 帧导出、`list-devices` 子命令、日志级别跟随配置

## M2 SDK / 角色包（核心差异化：从"应用"到"平台"）

- [x] `.petweave` 角色包格式：目录 + zip 打包，`pet.toml` 清单（元数据/动画/事件接线）+ assets
- [x] 清单校验（名字安全、网格整除、reaction 引用存在）—— `petweave-core::manifest`
- [x] 声明式精灵宠物运行时：网格精灵表（Codex 8×N 风格）、循环/一次性动画、
      `idle`/`key-left`/`key-right`/`key-both` 事件接线、大图缩放到表面尺寸
- [x] 包管理 CLI：`install`（目录或 zip）、`uninstall`、`list`、`package`（打包）、
      `import`（Oneko XPM → PNG）；仓库位于 `$XDG_DATA_HOME/petweave/pets/`
- [x] 预置示例包：`packages/bongo-sprite`（声明式 BongoCat 吃狗粮）、`packages/blinky`（网格示例）、
      `packages/lua-demo`（Lua 脚本示例）
- [x] 教程文档 `docs/PACKAGES.md`
- [x] Lua 脚本运行时：mlua（vendored Lua 5.4）沙箱 —— 白名单环境（无 io/os/package/debug）、
      指令预算钩子防死循环、错误吞掉不拖垮宿主
- [x] Lua API v1：事件 `on_key/on_tick/on_system/on_fullscreen/init`；动作 `pet.play/pet.speak`
      （气泡 + 系统字体文本渲染）/`pet.animations`/`pet.current`；查询 `sys.cpu/sys.mem/sys.focus`
- [x] SVG 渲染（resvg）：BongoCat 睡眠帧改用官方 SVG 素材栅格化（`graphics::svg_to_frame`），
      替代调暗占位帧
- [ ] 签名与社区商店—— 包格式预留 `signature` 位置

## M3 交互升级

- [ ] 指针事件接入（wl_seat）：悬停/点击反应
- [ ] 拖拽：指针按下→margin 节流移动，记录各合成器体验
- [ ] 轻物理：重力 + 桌面边缘碰撞 + 弹性衰减（自研，不引引擎）
- [ ] 多宠物共存：进程内多实例（共享 Wayland 连接与资产缓存）
- [ ] 点击穿透：`wl_surface.set_input_region` 按宠物形状收窄
- [ ] 托盘图标（ksni）+ 设置面板（egui，独立进程可选）

## M4 深度集成

- [ ] wgpu + linux-dmabuf GPU 渲染后端（`RenderBackend` 第二实现）
- [ ] Live2D 支持（Cubism Core Rust 绑定）+ BongoCat 模型社区兼容 ——
      详细路线见 `docs/LIVE2D.md`（渲染地基 → 模型加载 → 动作系统 → 完整对齐）
- [ ] 手柄/指针事件扩展（`Event::Gamepad` / `Event::Pointer`，对齐 Tauri BongoCat 输入）
- [ ] egui 设置面板（模型管理/行为配置/快捷键）
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
