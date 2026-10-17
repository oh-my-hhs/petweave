# PetWeave 功能实现计划表

> 依据 `docs/TECH_STACK.md` 的里程碑划分。`- [x]` 表示已完成，`- [ ]` 表示未完成。
> 更新日期：2026-10-17（M3 进行中；新增 M3.5 图形设置界面，详见 `docs/GUI_ROADMAP.md`）

## 当前状态

| 里程碑 | 状态 | 说明 |
|---|---|---|
| M0 项目骨架 | ✅ 完成 | 编译通过、单测通过、niri 上实测可显示 |
| M1 BongoCat 移植 | ✅ 完成 | 爪击动画/热插拔/全屏隐藏/多显示器/睡眠/热重载/系统感知/doctor/单例 |
| M2 SDK / 角色包 | ✅ 完成 | 包格式/声明式精灵宠物/Lua 沙箱运行时/CLI/示例包/SVG 渲染 |
| M3 交互升级 | ✅ 完成 | 指针/拖拽/物理/点击穿透/托盘/多宠物共存（设置面板移交 M3.5） |
| M3.5 图形设置界面 | 🔶 进行中 | G0 多宠物运行时与配置 v2 ✅；GUI 独立进程（egui/eframe）待做。**详细路线见 [`docs/GUI_ROADMAP.md`](GUI_ROADMAP.md)** |
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
- [x] 离线构建支持（依赖锁定 + vendored Lua，`cargo build --offline` 可复现）
- [x] `petweave doctor` 权限体检工具（udev uaccess 一键安装）—— 在 M1 完成
- [x] PID 文件单例 + `$XDG_RUNTIME_DIR` 安全落盘 —— 在 M1 完成（flock 型）
- [x] 发布加固：LTO + codegen-units=1 + `panic=abort` + strip，二进制 8.8MB → 5.4MB

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

- [x] 指针事件接入（wl_seat）：悬停/点击反应 → `Event::Pointer`，sprite `click` 反应 + Lua `on_pointer`
- [x] 拖拽：按住左键移动宠物（自动切换自由定位，margin 按渲染帧提交）
- [x] 轻物理：重力 + 屏幕边缘碰撞 + 弹性衰减 + 静止收敛（自研，`physics` 可关）
- [x] 多宠物共存：进程内多实例（共享 Wayland 连接与资产缓存）—— 由 M3.5 G0 完成
      （配置 v2 `[[pets]]`、每宠物独立 surface/拖拽/物理/显隐、运行中增删、轻量资产缓存）
- [x] 点击穿透：`wl_surface.set_input_region` 按 alpha 包围盒收窄（`click_through` 可关）
- [x] 托盘图标（ksni，StatusNotifierItem：左键显隐、菜单退出，`tray_enabled` 可关）
- [ ] 设置面板（egui，独立进程）—— 移交 M3.5

## M3.5 图形设置界面（GUI）

> 目标：多宠物管理 + 参数可视编辑 + 布局画布 + 实时预览，GUI 作为**独立进程**，
> 宿主保持零 GUI 依赖。完整需求、协议、配置 v2 与逐项任务见
> **[`docs/GUI_ROADMAP.md`](GUI_ROADMAP.md)**（增量维护，实现时在那里勾选）。

- [x] G0 多宠物运行时与配置 v2：`[[pets]]` 数组 + `[render]` 默认值继承 + 旧 `[pet]` 兼容迁移 + 每宠物独立 surface + 运行中增删
      （详见 [`docs/GUI_ROADMAP.md`](GUI_ROADMAP.md) §9 G0；资源基线见文末）
- [ ] G1 控制通道：`$XDG_RUNTIME_DIR/petweave/control.sock`（JSON Lines、版本协商、权限 0600）
- [ ] G2 设置面板 MVP：`crates/petweave-settings`（egui/eframe）+ 宠物列表 + 参数表单 + 原子保存（`toml_edit` 保注释）
- [ ] G3 布局画布与实时预览：按显示器分页拖拽定位（吸附）+ 缩略图/动画预览（复用 `Frame` 渲染）
- [ ] G4 诊断与分发：内嵌 `doctor`、托盘「设置…」、PKGBUILD/Nix 产物、README 章节
- [ ] 入口：`petweave settings`（exec 同目录 `petweave-settings`）+ 托盘菜单 + 独立二进制
- 验收：不手写 TOML，纯 GUI 完成"三只宠物同屏、各自定位、改 BongoCat 高度并在桌面立即生效"

## M4 深度集成

- [ ] wgpu + linux-dmabuf GPU 渲染后端（`RenderBackend` 第二实现）
- [ ] Live2D 支持（Cubism Core Rust 绑定）+ BongoCat 模型社区兼容 ——
      详细路线见 `docs/LIVE2D.md`（渲染地基 → 模型加载 → 动作系统 → 完整对齐）
- [ ] 手柄/指针事件扩展（`Event::Gamepad` / `Event::Pointer`，对齐 Tauri BongoCat 输入）
- [ ] 设置面板扩展（模型管理/快捷键/行为预设）—— 基础面板见 M3.5，扩展项见 `docs/GUI_ROADMAP.md` G5
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
- **M3.5 选型已定**：GUI 用 egui/eframe，独立 crate + 独立二进制（`petweave-settings`），
  宿主不引入任何 GUI 依赖；布局同时提供"表单"与"画布拖拽"两种方式；先做多宠物运行时
  再做界面。决策与理由见 `docs/GUI_ROADMAP.md` §2（决策记录 D1–D11）。
- **M3 多宠物基线**：三宠物同屏（2×BongoCat + 1 sprite）在 niri 实测通过——三块独立 SHM 缓冲
  （HiDPI scale 1.5 → 物理 ×2）、独立 anchor/margin/click_through/移动模式；热重载 300ms 去抖内
  完成增删（surface 创建/销毁）、换 kind（重建）、改尺寸（重配置）。共享资产缓存使同尺寸的两只
  BongoCat 只解码一次（`hits`/`decoded` 见 `runtime … asset cache` 日志），Q1 结论为**按 key 引用计数共享**。
- **G0 体积基线**：release 二进制 strip 后 7.46MB → **7.76MB（+4%）**，增量来自 `toml_edit`
  （`config migrate` 保注释）与多 surface/资产缓存代码；宿主仍无任何 GUI 依赖（N01 满足）。
