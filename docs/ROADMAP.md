# PetWeave 功能实现计划表

> 依据 `docs/TECH_STACK.md` 的里程碑划分。`- [x]` 表示已完成，`- [ ]` 表示未完成。
> 更新日期：2026-10-18（M3 完成；M3.5 进行中，G0/G1 已落地，详见 `docs/GUI_ROADMAP.md`）

## 当前状态

| 里程碑 | 状态 | 说明 |
|---|---|---|
| M0 项目骨架 | ✅ 完成 | 编译通过、单测通过、niri 上实测可显示 |
| M1 BongoCat 移植 | ✅ 完成 | 爪击动画/热插拔/全屏隐藏/多显示器/睡眠/热重载/系统感知/doctor/单例 |
| M2 SDK / 角色包 | ✅ 完成 | 包格式/声明式精灵宠物/Lua 沙箱运行时/CLI/示例包/SVG 渲染 |
| M2.5 可配置按键/图案 | ⏳ 规划 | 包声明配置项 + 改 config 生效（见下） |
| M2.6 Lua 宿主按键与尺寸 | ✅ 完成 | `[[reactions]]` 解析、`[pet]` 连字符键名、`pet.size` 语义、`OWN_KEYS`；两个参考包端到端测试通过 |
| M2.7 尺寸与摆放契约（GUI 前置） | ✅ 完成 | 尺寸优先级 `[[pets]]` > 宠物 > `[render]`、等比缩放不拉伸、删除 anchor/bob、`pet.offset(dx,dy)` + `padding`；`Pet::resize_to` / `Runtime::fit_frames` 让配置尺寸真正缩放内容 |
| M3 交互升级 | ✅ 完成 | 指针/拖拽/物理/点击穿透/托盘/多宠物共存（设置面板移交 M3.5） |
| M3.5 图形设置界面 | 🔶 进行中 | G0 多宠物配置 ✅、G1 控制通道 ✅、G2 设置面板 MVP ✅（`petweave-settings`，独立进程）；G3 布局画布/预览、G4 诊断/分发待做。**详细路线见 [`docs/GUI_ROADMAP.md`](GUI_ROADMAP.md)** |
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
- [x] G1 控制通道：`$XDG_RUNTIME_DIR/petweave/control.sock`（JSON Lines、版本协商、目录 0700/socket 0600）；
      `Hello`/`ListPets`/`ListOutputs`/`Reload`（含内存补丁）/`SetPlacement`/`SetSize`/`SetVisible`/
      `SetMoveMode`/`SetPaused`/`Quit` + `PetsChanged`/`PetMoved`/`Status` 事件；`RenderPet` 延后
      （`caps.preview=false`）；`petweave control '<json>'` 调试客户端；9 项协议测试 + niri 双宠物实机验收
      （详见 [`docs/GUI_ROADMAP.md`](GUI_ROADMAP.md) §9 G1）
- [x] G2 设置面板 MVP：`crates/petweave-settings`（egui/eframe，MSRV 1.95，glow）+ 宠物列表/参数表单/关于页 +
      非阻塞控制通道 client + 原子保存（`toml_edit` 逐键更新保注释、`.bak`、0600、回读校验、旧格式迁移、外部修改检测）+
      `petweave settings` 入口；热重载：`kind`/`package` 自动重建、配置宽高等比缩放素材（含 BongoCat）；
      21 项库测试 + 3 项集成测试（含 `egui_kittest`）（详见 [`docs/GUI_ROADMAP.md`](GUI_ROADMAP.md) §9 G2）
- [ ] G3 布局画布与实时预览：按显示器分页拖拽定位（吸附）+ 缩略图/动画预览（复用 `Frame` 渲染）
      （G2 已提供 anchor + 四边 margin 的等效表单）
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

## M2.5 可配置按键与输入图案（proposal）

> 背景：包内角色（如 `packages/krill-pet`、`packages/puro-pet`）已具备
> `pattern0/1/2` 三类"按键特效"素材：按 1/2/3 键分别显示 `pattern0/1/2`，按住停住，
> 松手回 `basic`。但**哪些物理按键算"焦点键"(left/right 交替)、哪些键触发哪个 pattern、
> 长按多久开始抖动、抖动幅度多大**目前都写死在包脚本里（`main.lua` 顶部的
> `KEYS_LEFT`/`KEYS_RIGHT`/`PATTERN`/`SHOW_H`/`BOB_PX`/`HOLD_SEC` 常量），用户无法改。
> 目标：**包声明可配置项及默认值 → 用户只改 config**，宿主与包都不需要重新打包。

- [ ] M2.5-1 包清单声明配置项：`[[pets.options]]`（或 `[options]`）数组，每项
      `key`（命名空间内唯一）、`label`（中文说明，GUI 表单标题）、`type`
      （`string|int|float|bool|keycode|enum|keycode-list`）、`default`、`min`/`max`、
      `enum`（可选值 + 显示名）、`description`。`petweave-core::manifest` 解析 +
      类型校验（默认值必须满足自身约束），错误信息点名 `options.<key>`。
- [ ] M2.5-2 用户配置覆盖：`[[pets]].options`（`[pets.options]` 表）按 key 覆盖包默认值；
      `Config::validate()` 负责校验（单点数来源，GUI 复用），未声明的 key 报错而不是静默丢弃；
      `[[pets]]` 省略该表时行为与现在完全一致（向后兼容）。
- [ ] M2.5-3 运行时下发：`PetConfig` 新增 `options`（已落地的通用通道见下），
      `<pets>.options` → `PetConfig.options` → Lua `config.*`；Lua 侧只暴露包声明的 key，
      读出值已是"默认值 + 用户覆盖"后的最终值。
- [ ] M2.5-4 按键映射可配置：包用 `type = "keycode-list"` 声明 `keys.left` / `keys.right`
      （EV_KEY 码），未配置时回退到内置物理左右手映射；按"键码 → 槽位（left/right）"建表，
      同一物理键在两端都出现时以右/左声明顺序为准并给出告警。
- [ ] M2.5-5 长按与 pattern 档位：`hold_ms`（按下多久算长按）、`pattern.cycle_ms`、
      `pattern.interval_ms`（长按后 pattern0/1/2 轮流显示的速度）作为配置项，
      替代目前写死在 `main.lua` 里的 `HOLD_MS = 0.35` 等常量。
- [ ] M2.5-6 面向 Lua 的读取 API：`config.get(key)` / `config.keys()` / `config.number(key)`，
      沙箱内只读；配置文件热重载时重新下发（`Pet::reload` → `config` 刷新），不重启进程。
- [ ] M2.5-7 GUI 表单自动生成：设置面板按清单 `options` 渲染控件（类型 → 控件，
      `label/description` → 文案），用户"改 config 即完成配置"，不写 TOML。
- [ ] M2.5-8 文档与示例：`docs/PACKAGES.md` 新增"可配置项"章节；两个示例包把
      按键映射/长按时长提升为可配置项，`petweave.toml.example` 给出覆盖示例。
- [ ] M2.5-9 校验与迁移测试：未知 key、类型不符、越界、枚举非法值、旧配置缺省全覆盖。

> 落地顺序建议：M2.5-1/2/3 为通道（一次性打通，所有包受益）→ 4/5/6 让现有包"零改包"可配
> → 7 交给 M3.5 GUI → 8/9 收尾。在通道落地前，包内脚本的常量（`KEY_LEFT`/`KEY_RIGHT`/
> `HOLD_MS`/`PATTERN_STEP`）就是当前唯一可调点，修改需要重新打包。

## M2.6 Lua 宿主按键与尺寸（已解决，结论写入文档）

> 做参考包 `packages/krill-pet` / `packages/puro-pet` 时，包内行为"没有按预期运行"。
> 排查结论：**宿主 Lua 状态本身没有丢失**，问题出在三处契约不一致，现已修复。

- [x] M2.6-1 `[[reactions]]` 解析不到：`[[reactions]]` 数组被反序列化成 `keys` 字段，
  而代码读的是 `reactions.extra`，于是逐键反应表永远为空。已改为自定义 `Deserialize`：
  数组元素进入 `extra`，元素里的具名字段（`idle = …`）落到具名反应；`[reactions]` 表
  写法与 `[[reactions.extra]]` 旧拼法继续有效。
- [x] M2.6-2 `[pet]` 的连字符键名被静默忽略：`PetDecl` 没有 `rename_all`，文档写的
  `key-mode` / `bob-pixels` / `anchor-padding` 解析后全是默认值（这就是"press 模式没生效"）。
  已加 `serde(alias = …)`，下划线与连字符两种写法等价，并有测试 `pet_knobs_accept_both_spellings`。
- [x] M2.6-3 `pet.size` 的语义与包脚本的假设相反：旧实现把 `pet.size(w, h)` 当"把图片拉伸到
  `w`x`h`"，而包脚本以为它是"surface 高度"（图片贴顶画、下沿被裁）。现统一为：
  **`pet.size` 设置表面尺寸，图片不缩放、按 `anchor_mode` 摆放、超出部分裁掉**；
  新增 `pet.offset(px)` 做精确像素位移；`anchor_mode = "top" | "bottom"` 决定表面加高
  的方向。内置 `bob-pixels` 只对 `top` 锚点生效（贴底图片上抬会在脚下露空白）。
- [x] M2.6-4 脚本与声明式层抢键：`kind = "lua"` 的包同时被 sprite 的 `press` 模式映射按键，
  把脚本 `pet.play` 的结果覆盖掉。新增脚本声明 `OWN_KEYS`（脚本独占按键，精灵退化为画布）
  与 `OWN_PIXELS`（保留声明式按键，但允许脚本精确摆放）。
- [x] M2.6-5 空闲 CPU：声明 `OWN_KEYS`/`OWN_PIXELS` 的脚本在动画期间由宿主按 16ms 步进 tick，
  静止（没有按住、没有上跳）时 `next_deadline()` 返回 `None`，宿主照常阻塞在 poll。
- [x] M2.6-6 回归补齐：`press_packages_load` / `press_packages_behave` /
  `press_packages_sleep_when_idle_and_wake_on_keys` 三个端到端测试直接用仓库里的两个包驱动
  真实按键事件（交替、按住、表情、上跳 6px、落回 224x140），`sandbox_globals_behave`、
  `script_locals_survive_load` 覆盖沙箱行为。
- [ ] M2.6-7 气泡像素断言（`script_handlers_fire_and_play_animations`）：在测试进程里仍不稳定，
  暂保持只断言气泡状态；与本次按键/尺寸问题无关，留待单独排查。

## M3.1 默认移动模式与渲染层级（已完成）

- [x] 默认移动模式由 `physics` 改为 `drag`（`RenderConfig::default().physics = false`）；
      仍可用 `mode = "physics"` 或旧 `physics = true` 逐只/全局开启重力。
- [x] 托盘新增「渲染层级」子菜单：`Top`（常规，全屏时隐藏）/ `Overlay`（全屏时仍显示）。
      `HostCommand::SetLayer` 在主线程对每只宠物 `apply_props`；overlay 层绕过全屏自动隐藏。

## M2.7 尺寸与摆放契约（GUI 前置，已定案）

> 做设置界面时要能"调大小、调位置"，先得把底层契约定死，否则 UI、协议、配置三处都要返工。
> 决策（Q1A/Q2A/Q3B/Q4A/Q5A/Q6A+B/Q7A）与 `docs/GUI_ROADMAP.md` §13 Q9–Q11 一致。

- [x] 尺寸优先级：显式 `[[pets]].width/height` > 宠物 `preferred_size()` > `[render]`；
      只写一边时另一边按原图比例推算（`petweave_core::pet::scaled_for`）。
- [x] 等比缩放：内容按盒子比例缩放（`scale_to_fit`），永不拉伸；`Pet::resize_to` +
      `Runtime::fit_frames` / `tick_all` 在尺寸变化后同时更新渲染帧与图片。
- [x] 删除 `bob_pixels` / `anchor_padding` / `anchor_x` / `anchor_mode` 与内置弹跳；
      旧包的这些键被忽略、仍可加载。
- [x] 摆放模型：图片贴表面左下角、`pet.offset(dx, dy)`（y 向下）精确位移，
      `pet.padding` 四周预留余量；`pet.bump()` 移除。
- [x] anchor 保持为 layer-shell 实现细节：配置仍用 `anchor/margin_*`，GUI 层翻译为
      x/y + 对齐方式，不新增绝对坐标键（Q4A）。
- [x] `petweave doctor -c <config>`：诊断可针对任意配置文件（GUI 诊断页复用）。
- [x] 包体回归：`packages/` 下 bongo-sprite / blinky / lua-demo / krill-pet / puro-pet
      全部按新模型预览通过。

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
- **G2 体积/内存基线**：release 下 `petweave` **7.73MiB**、`petweave-settings` **12.39MiB**；
  GUI 常驻 RSS ≈190MB（Mesa 软件 GL 回退，真实 GPU 待复测），宿主单 BongoCat（scale 2）≈14.9MB。
  宿主依赖树仍无 egui/eframe/winit（N01）。
- **G0 体积基线**：release 二进制 strip 后 7.46MB → **7.76MB（+4%）**，增量来自 `toml_edit`
  （`config migrate` 保注释）与多 surface/资产缓存代码；宿主仍无任何 GUI 依赖（N01 满足）。
