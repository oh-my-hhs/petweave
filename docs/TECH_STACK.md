# PetWeave 技术栈分析与设计建议

> 版本：v0.1（分析稿）· 依据：`Innovation.md` + `Reference/wayland-bongocat` + `Reference/BongoCat`
> 目标：为「Wayland 桌宠框架/平台」做技术选型与架构决策，并支撑 BongoCat、Oneko 等经典桌宠的移植。

---

## 0. 结论摘要（TL;DR）

| 决策点 | 建议 | 一句话理由 |
|---|---|---|
| 语言 | **Rust**（edition 2021+，stable） | 内存安全 + 全链路成熟生态（Wayland/Lua/GPU），规避 Shijima-Qt 式的重型 GUI hack |
| 窗口/定位 | **wlr-layer-shell**，每个宠物一个 surface | 唯一被主流合成器普遍支持的可锚定、可穿透 overlay 方案 |
| 渲染 | 抽象 `RenderBackend`；MVP 用 **wl_shm + CPU 合成**（预缩放帧缓存），v2 加 **wgpu + linux-dmabuf** | 精灵/矢量宠物不需要 GPU；Live2D 和特效才需要 GPU |
| 输入 | **evdev**（`/dev/input/event*`）+ 合成器 IPC 增强 | Wayland 没有全局键盘监听协议，evdev 是唯一通用路径 |
| 权限 | **udev `uaccess` 规则** + 输入组兜底 + 权限体检工具 | 免 sudo 配置、可自动化、随登录会话生效 |
| 宠物脚本 | **Lua（mlua）** + 声明式配置（无代码宠物）；原生插件走 Rust cdylib | 艺术家可零代码做宠物，进阶行为用 Lua |
| 角色包 | `.petweave`（ZIP）：`pet.toml` + assets + `main.lua` + 签名 | 借鉴 shimeji/Codex 宠物包理念，做成标准格式 |
| 进程模型 | 主机进程（Wayland/输入/渲染）+ 宠物实例；第三方宠物可选**隔离子进程**（JSON-RPC over Unix socket） | 隔离与崩溃安全，同时保持低资源 |
| 兼容性 | Hyprland/Sway/niri ★★★，KWin ★★☆，GNOME ★☆☆（受限模式） | 明确的兼容矩阵 + 降级策略，不承诺做不到的事 |
| 资源目标 | 单宠物 RSS ≤ 25MB（优先 ≤15MB）、空闲 CPU ~0%、启动 <100ms | 对标 deskpet（~18MB）与 wayland-bongocat（~8MB） |

---

## 1. 需求拆解（Innovation.md → 工程需求）

Innovation.md 的差异化定位是：**从"更好的桌宠"进化为"让任何人都能轻松创造桌宠的平台"**。拆成工程需求：

1. **框架/平台，而非单应用**：需要 SDK、插件/角色包系统、文档与创建工具。
2. **低资源**：单宠物 ≈ deskpet 水平（~18MB）；空闲近乎零 CPU。
3. **权限安全化**：`/dev/input/event*` 读取权限的自动/半自动配置与体检。
4. **兼容性层**：Hyprland 最佳，尽力覆盖 Sway、Niri、KDE，GNOME 给出诚实说明。
5. **交互升级**：拖拽、重力/碰撞物理、多宠物共存。
6. **系统感知**：CPU/内存、活跃窗口反应；预留 AI/外部信息（天气、邮件）接口。
7. **开箱即用**：预置 BongoCat、Oneko 等经典宠物包。
8. **社区生态**：角色包分享、制作工具链、文档。

---

## 2. 两个参照物的启示

### 2.1 wayland-bongocat（C23，~5.3k LOC，8MB RSS，300KB 二进制）

**可复用的优秀实践**（框架应继承）：
- **渲染**：SVG 预栅格化到目标尺寸 → BGRA 帧缓存 → 纯 memcpy blit；向量图任意尺寸像素级清晰；premultiplied alpha 避免边缘暗边。
- **性能**：空闲时动画线程 `poll(eventfd, 1s)` 事件驱动（~0% CPU）；输入子进程 `atomic_fetch_or` 写入按键位 + eventfd 唤醒；未变化帧不重绘（~95% 省绘制）。
- **热重载三段式**：属性级（位置/层级）→ 缓冲区级（尺寸）→ 全重建（输出变更），避免崩溃。
- **全屏隐藏**：wlr-foreign-toplevel-management + KDE 兜底。
- **输入**：evdev 快速重试（5s 直到发现设备 → 30s 热插拔扫描）；`fork()/execvp()` 而非 `popen()` 防注入；PID 文件 O_NOFOLLOW 0600。
- **安全**：路径校验（必须 `/dev/input/` 前缀）、整数校验、PIE/full RELRO/noexecstack。

**局限**（框架要解决的）：
- 宠物写死（内置 SVG 猫），无插件/脚本系统、无角色包。
- 手写/提交生成绑定，无包管理，分发靠 AUR/PKGBUILD。
- 无拖拽、无物理、无系统感知、单宠物/单用途。

### 2.2 BongoCat（Tauri 2 + Vue + Pixi.js + Live2D Cubism）

**可继承的资产**：Live2D 模型生态（Awesome-BongoCat 社区、模型转换工具），模型即角色包的核心资产。
**架构教训**：WebView（WebKitGTK ~100-300MB）+ 透明窗口 + X11-only Linux 支持 —— 恰恰是"重 GUI + hack"路径，且不支持 Wayland。

### 2.3 Shijima-Qt 的教训（已归档）

**"Qt + Wayland Layer Shell 的结合充满 hack"** → 不要用重型 GUI 框架去适配 layer-shell。本框架核心**无 GUI 依赖**（渲染 = 自绘 SHM/wgpu 像素），设置界面独立于运行时（见 §4.10）。

---

## 3. 语言选型

### 3.1 候选对比

| 语言 | 生态成熟度 | 内存/性能 | 框架开发效率 | 社区贡献门槛 | 结论 |
|---|---|---|---|---|---|
| **Rust** | Wayland: wayland-rs/sctk/wgpu；Lua: mlua；图像: image/resvg | 优（可到 10-25MB） | 高（cargo、serde、tokio/calloop） | 中（借用检查有学习曲线） | **推荐** |
| C23 | 好但全部手写（wayland-bongocat 证明可行） | 最佳（8MB） | 低（绑定、内存、插件系统全手搓） | 高 | 备选，仅限"极致瘦身"路线 |
| C++/Qt | layer-shell 绑定脆弱 | 差（Qt 常驻 ~50MB+） | 中 | 中 | **否决**（Shijima-Qt 教训） |
| Zig | Wayland 绑定不成熟 | 优 | 低 | 高 | 否决（MVP 生态不足） |
| Go | Wayland 绑定薄弱、GC | 中 | 中 | 中 | 否决 |
| Python/Node | 只能做"外部宠物进程"角色 | 差 | — | — | 否决为核心，但**外部进程协议**允许它们参与 |

### 3.2 为什么是 Rust（展开）

1. **内存安全**：输入线程、动画线程、Wayland 回调、热重载并发 —— wayland-bongocat 的 CHANGELOG 里一长串 use-after-free/race 修复正是 C 的代价；Rust 在编译期消灭这类问题。
2. **生态恰好全覆盖**：`smithay-client-toolkit`（layer-shell/xdg-output）、`wayland-protocols`（wlr-foreign-toplevel-management、ext-*）、`evdev`、`mlua`、`resvg`、`wgpu`、`zbus`（KWin/托盘）、`sysinfo`、`ksni`/`tray-icon`、`notify`、`clap`。全部是库级集成，无需绑死任何 GUI。
3. **分发**：单一静态二进制（musl 目标可选），便于 PKGBUILD/Nix/flatpak/AppImage 多路分发。
4. **可嵌入脚本**：mlua（Lua 5.4/LuaJIT）体积小、易沙箱，适合做"宠物行为语言"。
5. **风险与对策**：
   - 编译时间长 → 合理模块划分 + cargo 增量缓存；发行版打包用发布 profile + LTO。
   - 内存控制需要用心 → 参考 wayland-bongocat 的"预缩放帧缓存/双缓冲 SHM"模式；警惕 `image` 解码大图、`tokio` 无谓线程池（首版用 calloop 事件循环即可）。

### 3.3 宠物开发语言（SDK 侧）

- **声明式（零代码）**：精灵表 + 行为配置（走、待机、点击反应…）→ 艺术家友好，覆盖 Oneko/Codex 宠物包。
- **Lua 脚本**：进阶行为（按键映射、状态机、系统感知、联网）。
- **Rust cdylib 原生插件**：性能敏感或深度集成（如 Live2D 播放器、AI 模型）。
- **远期**：WASM 插件 ABI（统一、沙箱好，但 wasmtime 体积大，放 v2+）。

---

## 4. 技术栈逐项分析

### 4.1 Wayland 协议栈

| 协议 | 用途 | 备注 |
|---|---|---|
| `wlr-layer-shell` | 宠物 surface 定位（anchor/margin/层/独占区）、exclusive zone | 版本协商 `MIN(advertised, desired)`；KWin 自 Plasma 6.0 起支持 |
| `xdg-output` | 多显示器按名称枚举 | 与 wl_output 逻辑尺寸/缩放配合（HiDPI 已踩过坑） |
| `wlr-foreign-toplevel-management` / `ext-foreign-toplevel-list` | 全屏检测、（尽力）活跃窗口 | wlroots 系实现前者，KWin/niri 实现后者；两者都试，缺失则降级 |
| `ext-workspace`（远期） | 工作区感知 | 用于"按工作区隐藏/显示" |
| `wl_seat`（pointer/keyboard） | **自家 surface 内**的指针/按键（拖拽、点击互动） | 全局键盘监听走 evdev，见 §4.3 |
| `zwp_virtual_keyboard`（远期） | 宠物"打字"给系统 | 未来互动功能，非 MVP |

实现层面：基于 `wayland-rs`（`wayland-client`）+ `calloop` 事件循环；公共部分用 `smithay-client-toolkit`，扩展协议用 `wayland-protocols` 的生成模块，协议 XML 随仓库提交（借鉴 wayland-bongocat 的 `make protocols` 思路，避免构建期依赖 wayland-scanner）。

### 4.2 渲染

**抽象 `RenderBackend` trait**（`begin_frame / draw_sprite / draw_svg / draw_mesh / present`），两条实现：

- **Backend A：wl_shm + CPU 合成（MVP）**
  - 每宠物一个小 surface（如 512×512 以内，双缓冲 = 每缓冲 ~1MB，多宠物总内存可控）。
  - 资产预解码：精灵表 PNG → 预缩放 BGRA 帧缓存；SVG → resvg 预栅格化（等价 nanosvg 方案）。
  - 损伤区域（damage）提交；未变化帧不提交（wayland-bongocat 已证明收益巨大）。
  - 成本：wayland-bongocat 单帧 ~15KB memcpy / <1ms —— 结论：精灵类宠物完全够用，且合成器无关、无 GPU 依赖。
- **Backend B：wgpu + linux-dmabuf（v2，按需）**
  - 需要 GPU 的场景：Live2D 骨骼变形网格（Cubism Core 有 Rust 绑定 `live2d-rs`/`cubism-core`）、粒子/模糊特效、未来 3D。
  - wgpu 的 Wayland surface 支持已成熟；与 layer-shell 组合需自建 `wl_surface`→wgpu surface 桥接（社区已有先例）。
  - 策略：**默认 CPU，检测到 Live2D/特效资产时自动切 GPU**，单宠物独立选择后端。

**为什么不用 WebView/GTK/Qt 渲染**：资源占用（18MB 目标）直接出局；Shijima-Qt 前车之鉴。

### 4.3 输入捕获与权限（核心痛点之一）

**事实**：Wayland 没有全局键盘监听协议。可选路径：

1. **evdev（推荐，唯一合成器无关路径）**：直接读 `/dev/input/event*`，延迟低（wayland-bongocat 数据流已验证）。合成器经 libinput 也读同一设备，互不冲突（非独占读）。
2. **合成器 IPC 增强**：Hyprland socket / niri IPC / KWin DBus 提供活跃窗口、工作区等 evdev 给不了的语义信息（不是按键源）。
3. **远期评估 xdg-desktop-portal**：RemoteDesktop 门户可收输入但面向远程会话、有常驻指示器，UX 不适合桌宠；KDE GlobalShortcuts 门户逐键弹确认。结论：**不做默认路径**，留作"无权限运行"的实验选项。

**权限方案（三选一，工具自动推荐）**：
- **A. udev `uaccess` 规则（首选）**：随会话激活授权、无需注销重登、无需 root 组。
  ```
  SUBSYSTEM=="input", KERNEL=="event*", TAG+="uaccess"
  ```
- **B. `input` 组**：`usermod -aG input $USER`（wayland-bongocat 的做法），需重登。
- **C. 合成器 IPC 降级**：部分功能（全屏隐藏、活跃窗口）在无设备权限时仍可用。

配套工具（框架自带）：
- `petweave doctor`：检测设备权限、报告缺失、给出可复制命令 / 自动安装 udev 规则（`--apply`）。
- `petweave find-devices`：交互式按键识别设备（借鉴 bongocat-find-devices --interactive）。
- 热插拔：5s 快速重试 → 30s 扫描（复用 wayland-bongocat 策略）。
- 隐私：默认**不记录键码**；debug 开关需显式开启并在文档中大字警告（keylogger 风险）。

### 4.4 插件与脚本系统（核心痛点之二）

**双层模型**：

```
角色包 (.petweave)
 ├─ 声明式（无代码）：pet.toml + 精灵表/动作表   ← 覆盖 80% 创作者
 └─ 脚本式：main.lua（Lua 5.4，mlua 沙箱）
原生插件（可选）：.so 实现 PetPlugin C ABI      ← 深度集成
外部宠物（v2）：任意语言，Unix socket JSON-RPC  ← 语言自由 + 隔离
```

**Lua API 草案**（v1 面）：
- 事件回调：`on_key(key, down)`、`on_pointer(ev)`、`on_tick(dt)`、`on_fullscreen(bool)`、`on_active_window(info)`、`on_system(cpu, mem)`、`on_message(msg)`
- 动作：`play(anim)`、`move_to(x,y)`、`set_state(idle|walk|sleep)`、`speak(text)`（气泡）、`set_draggable(bool)`、`set_physics(bool)`
- 查询：`sys.cpu()/mem()`、`focus.title()/app_id()`、`net.http_get(url)`（白名单）、`storage.get/set`
- 沙箱：禁 `io`/`os.execute`（白名单函数表）、指令数/内存上限、超时看门狗、版本化 API（`require "petweave.api.v1"`）。

**原生插件 ABI**：稳定 C ABI（`petweave_plugin_v1` 结构体：name/version/init/event/paint），Rust 侧 `#[no_mangle]` 导出；为未来 WASM 插件保留同一事件模型。

### 4.5 角色包格式 `.petweave`（草案）

```
my-pet.petweave (ZIP)
├─ pet.toml          # name/version/author/license/compositor_req/thumbnail
├─ main.lua          # 可选
├─ assets/
│   ├─ sprites/      # 精灵表 PNG(+JSON 切图元数据)  [Oneko/Codex 8x9/8x11 兼容]
│   ├─ svg/          # SVG 动作组                    [BongoCat 风格]
│   ├─ live2d/       # Cubism model3 目录            [BongoCat 模型社区兼容]
│   └─ audio/        # 音效（可选）
└─ signature         # 可选，社区商店用
```

导入兼容：官方提供 **Codex 宠物图集（8×9/8×11）导入器**、Oneko 精灵表、BongoCat 模型转换流程 —— 让"移植"变成"导入"。

### 4.6 配置与热重载

- 格式：TOML（`~/.config/petweave/petweave.toml` + 每宠物覆盖）。
- 三段式热重载（继承 wayland-bongocat）：属性级（位置/层）→ 缓冲区级（尺寸）→ 全重建（输出变更）；重载只发生在 Wayland 主循环 tick 内，避免跨线程碰 Wayland 结构。
- 用 `notify`（inotify）监视 + 防抖。

### 4.7 拖拽、物理、多宠物

- **拖拽**：layer-shell 非交互式定位，连续移动需改 anchor/margin 触发 re-layout —— 这是已知的合成器性能痛点。
  - 缓解：拖拽时按帧率节流 margin 更新；个别合成器（Hyprland）表现尚可，KWin 需实测；文档写明各合成器体验。
  - 备选：拖拽会话临时切换为"全屏透明画布"只承载被拖宠物（内存 4K 双缓冲 ~66MB 不可常驻，仅拖拽期短暂启用）——v2 再考虑。
- **物理**：轻量自研（重力 + 地面/窗口边缘碰撞 + 弹性衰减），宠物局部坐标系，不引物理引擎。
- **多宠物**：主机进程内多实例（共享 Wayland 连接、输入、资产缓存）；第三方/不信任宠物 → 隔离子进程（JSON-RPC over `$XDG_RUNTIME_DIR` Unix socket），进程崩溃不影响主机。主机崩溃/重连自动恢复宠物会话（借鉴 bongocat 的 parent liveness check）。

### 4.8 系统感知与 AI 预留

- 通用：`sysinfo`（CPU/内存/网络）+ `wlr-foreign-toplevel-management`/`ext-foreign-toplevel-list`（全屏、活跃窗口）。
- 合成器插件 trait：`CompositorBackend`，实现 Hyprland（socket）、niri（IPC，当前用户环境）、KWin（DBus/脚本）。向宠物暴露统一 `focus.*`/`workspace.*` API。
- AI 预留：`on_message(msg)` + `speak()` 接口即可接入 CATAI/Sakura 式本地模型；宠物包可声明"需要 AI 后端"，运行时按能力协商（不内置模型）。

### 4.9 安全模型

- 输入：只读键位、默认不落盘；路径白名单（`/dev/input/`）校验。
- Lua：白名单沙箱 + 资源上限；原生插件需用户显式信任（签名/安装来源）。
- 外部宠物进程：seccomp/landlock 可选收紧；IPC 鉴权（`$XDG_RUNTIME_DIR` 0600 socket + cookie）。
- 构建加固：PIE、full RELRO、noexecstack（Rust 默认大部分满足，发布 profile 显式配置）。

### 4.10 周边

- **托盘/设置 UI**：托盘用 `ksni`（纯 Rust StatusNotifier）或 `tray-icon`；设置面板用 **egui**（进程内，需 GPU 或软渲染）或独立小 Web 面板（v2）——**不引入 GTK/Qt 运行时**。
- **自启**：XDG autostart 条目 + systemd user service（`--user`，`Restart=on-failure`）。
- **日志**：`tracing` + 文件环形缓冲；`petweave doctor` 收集诊断。
- **打包**：Arch PKGBUILD、Nix flake（含 NixOS 模块，自动装 udev 规则）、AppImage/flatpak 评估（flatpak 需 portal 路径适配，放 v2）。

---

## 5. 架构总览

```
┌─────────────────────────── petweave (host) ───────────────────────────┐
│  main:  CLI + 配置加载 + 单例(PID文件)                                  │
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────────────────────┐ │
│  │ Wayland 主循环│◄─┤ 输入捕获(evdev)│  │  PetRuntime (每宠物实例)        │ │
│  │ (calloop)   │  │ +合成器IPC插件 │  │  ├─ 声明式解释器 / Lua VM      │ │
│  │ layer-shell │  │ (niri/hypr/  │  │  ├─ 状态机 + 动画调度          │ │
│  │ xdg-output  │  │  kwin)       │  │  └─ 物理(重力/碰撞)           │ │
│  │ foreign-toplevel│              │  └──────────────────────────────┘ │
│  └──────┬──────┘  └──────────────┘             ▲                      │
│         │ RenderBackend (SHM CPU / wgpu GPU)   │ 事件(按键/焦点/系统)   │
│         ▼                                      │                      │
│  wl_surface × N (每宠物一 surface, 输入区域=宠物形状)                  │
└───────────────────────────────────────────────────────────────────────┘
        │ Unix socket (JSON-RPC) —— 可选隔离宠物子进程 / 外部语言宠物
```

数据流（借鉴 wayland-bongocat 并泛化）：
```
evdev → 输入线程 → 事件总线(eventfd唤醒) → 各 PetRuntime.on_key()
/proc + compositor IPC → 系统感知 tick → on_system()/on_active_window()
PetRuntime 状态机 → 帧索引 → RenderBackend.draw() → wl_surface.commit()
```

---

## 6. 兼容性矩阵与降级策略

| 合成器 | layer-shell | 全屏/活跃窗口 | IPC 增强 | 拖拽体验 | 评级 |
|---|---|---|---|---|---|
| Hyprland | ✓ | wlr-foreign-toplevel + socket | 丰富 | 尚可 | ★★★ |
| Sway | ✓ | wlr-foreign-toplevel + swaymsg | 一般 | 尚可 | ★★★ |
| **niri（当前环境）** | ✓ | ext-foreign-toplevel + `niri msg` | 丰富 | 待实测 | ★★★ |
| KWin/Plasma 6 | ✓ (6.0+) | ext-foreign-toplevel + DBus | 丰富 | 待实测 | ★★☆ |
| GNOME/Mutter | ✗ | 无 | 受限 | — | ★☆☆ |
| River/Wayfire/Labwc/Weston | ✓ | wlr-* | 无 | 尚可 | ★★☆ |

降级策略：GNOME 走 xdg-shell toplevel"受限模式"（可显示、不可穿透/不可置顶完美）；未实现协议时静默降级并在 `petweave doctor` 报告；文档诚实标注。

---

## 7. 里程碑路线图

| 里程碑 | 内容 | 验收 |
|---|---|---|
| **M0 骨架** | Rust 工程、calloop 主循环、layer-shell 最小 surface、SHM 双缓冲、配置/CLI、`doctor`/`find-devices` | 静态色块显示于 niri 桌面、可穿透 |
| **M1 BongoCat 移植** | SVG 帧渲染、evdev 按键→左右爪动画、热重载、全屏隐藏、多显示器 | 在 niri/Hyprland 上复现 wayland-bongocat 体验，RSS ≤ 25MB |
| **M2 SDK/角色包** | `.petweave` 格式、声明式宠物（Oneko 精灵表导入）、Lua VM + 事件 API、包管理器 CLI | 第三方无代码/脚本宠物可安装运行 |
| **M3 交互升级** | 拖拽 + 轻物理、多宠物（进程内）、托盘/设置面板（egui） | 可拖拽有重力、双宠物共存 |
| **M4 深度** | wgpu 后端 + Live2D（BongoCat 模型社区）、外部宠物进程协议、系统感知/AI 预留接口落地 | 导入 Live2D 模型、宠物对 CPU/焦点反应 |

---

## 8. 风险与对策

| 风险 | 对策 |
|---|---|
| layer-shell 拖拽在部分合成器上卡顿 | 节流 + 文档分级 + v2 全屏画布拖拽会话；不承诺 100% 顺滑 |
| GNOME 生态缺失 | 诚实标注"受限模式"，把精力集中在 wlroots/niri/KDE |
| evdev 权限成为用户门槛 | uaccess 规则一键安装 + `doctor` + 发行版打包时预置规则 |
| Lua 沙箱逃逸 | 白名单函数表 + 资源上限 + 定期审计；原生插件需显式信任 |
| Live2D 依赖闭源 Cubism Core | 资产级依赖（运行时才需要），框架核心不绑定；文档说明许可证 |
| 内存目标被 GPU/WebView 拖垮 | 渲染后端按需启用；CPU 后端为默认路径 |

---

## 附：推荐依赖清单（MVP）

```
wayland-client / wayland-protocols / smithay-client-toolkit   Wayland
calloop                              事件循环（sctk 集成）
evdev                                输入设备读取
mlua                                 Lua 5.4 沙箱
resvg (usvg)                         SVG 栅格化
image + png                          精灵表解码
toml / serde / serde_json            配置与元数据
zip                                  角色包打包
notify                               inotify 热重载
sysinfo                              CPU/内存感知
zbus + ksni (或 tray-icon)           DBus(KWin) + 托盘
clap                                 CLI
tracing / env_logger                 日志
wgpu（v2，feature-gated）             GPU 后端
```
