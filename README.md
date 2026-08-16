# PetWeave

一个 **Wayland 原生的桌宠框架**：运行时 + SDK 约定，让"做一个桌宠"变成"写一个角色包"，预置 BongoCat 作为开箱即用的第一个宠物。

- 极轻量：实测空闲 RSS ≈ **5MB**、空闲 CPU ~0%（无 WebView/GTK/Qt 运行时）
- 原生 Wayland（`wlr-layer-shell`）：定位、全屏自动隐藏、多显示器、HiDPI
- 配置热重载、键盘热插拔、进程单例、优雅退出
- 预置宠物：`demo`（按键闪白）、`bongo`（BongoCat 爪击动画，wayland-bongocat 移植）

创新点说明见 [Innovation.md](Innovation.md) · 技术选型见 [docs/TECH_STACK.md](docs/TECH_STACK.md) · 实现计划见 [docs/ROADMAP.md](docs/ROADMAP.md)

## 构建

```bash
cargo build --release      # 产物: target/release/petweave
cargo test                 # 运行测试（32 项）
```

> **离线构建（本机）**：网络不可用且系统 cargo 缓存只读时，使用仓库内的 `.cargo-home`：
> ```bash
> CARGO_HOME=/home/hhs/Projects/petweave/.cargo-home cargo build --offline
> ```

### 依赖
- Rust 1.80+（stable）
- Wayland 合成器（支持 `wlr-layer-shell`）：niri / Hyprland / Sway / KWin(Plasma 6) 等
- 键盘监听需要 `/dev/input` 权限（见下方「权限」）

## 运行

### 快速开始

```bash
./target/release/petweave                          # 启动 demo 宠物（默认配置）
./target/release/petweave --pet bongo              # 启动 BongoCat（默认从仓库 assets/ 加载）
./target/release/petweave -c ~/.config/petweave/petweave.toml
```

启动后宠物显示在屏幕底部（默认 anchor=bottom，margin 16px）。**BongoCat 会在你按键时击打对应爪子**——按键盘左侧的键（A/W/Q/…）动左爪，右侧（L/;/Space/…）动右爪，同时按则双爪齐下。

### 命令行参数

| 参数 | 说明 | 示例 |
|---|---|---|
| `-c, --config <PATH>` | 指定 TOML 配置文件；未指定时尝试 `$XDG_CONFIG_HOME/petweave/petweave.toml` | `-c petweave.toml` |
| `--pet <demo\|bongo>` | 覆盖 `pet.kind`，选择内置宠物 | `--pet bongo` |
| `--width <N>` | 覆盖表面宽度（逻辑像素） | `--width 300` |
| `--height <N>` | 覆盖表面高度 | `--height 150` |
| `--fps <N>` | 覆盖动画帧率上限（1–240） | `--fps 60` |
| `--device <PATH>` | 显式指定输入设备，可重复 | `--device /dev/input/event6` |
| `--no-auto-input` | 关闭键盘自动探测 | |
| `--list-devices` | 列出探测到的键盘设备后退出 | |
| `--preview <PATH>` | 渲染宠物当前帧为 PNG 后退出（无需 Wayland，开发调试用） | `--preview out.png` |
| `-v, --verbose` | debug 日志（等价 `log_level = "debug"`，`RUST_LOG` 环境变量优先） | |

### 子命令

| 子命令 | 说明 |
|---|---|
| `petweave doctor` | 诊断环境：配置文件、Wayland 会话、键盘权限；`--apply` 一键安装 udev uaccess 规则（需要 root/sudo） |
| `petweave list-devices` | 同 `--list-devices`，列出键盘设备 |

### 权限

BongoCat 等宠物读取 `/dev/input/event*` 需要权限，两种方式任选：

```bash
# 方式 A（推荐）：udev uaccess 规则，随登录会话授权
petweave doctor --apply        # 或手动写入:
#   SUBSYSTEM=="input", KERNEL=="event*", TAG+="uaccess"
#   到 /etc/udev/rules.d/99-petweave-input.rules 后: sudo udevadm control --reload

# 方式 B：加入 input 组（需要重新登录）
sudo usermod -aG input $USER
```

用 `petweave doctor` 或 `petweave list-devices` 检查是否生效。

## 配置

配置文件为 TOML，**运行中修改会自动热重载**（300ms 防抖）。所有键都可省略，省略项用默认值。

### 完整示例

```toml
[general]
fps = 60                       # 动画帧率上限 1–240
log_level = "info"             # trace|debug|info|warn|error
sysinfo_interval_secs = 5      # CPU/内存采样间隔（秒），0=关闭 → 触发 Event::System

[input]
enabled = true                 # 全局键盘捕获开关
auto_detect = true             # 自动探测 /dev/input/event*
devices = []                   # 显式设备路径（优先于自动探测）
scan_interval_secs = 30        # 热插拔重扫间隔；未找到设备时 5s 快速重试

[render]
width = 256                    # 表面宽度（逻辑像素；bongo 会用自身宽高比覆盖）
height = 256
layer = "top"                  # background|bottom|top|overlay
anchor = "bottom"              # top|bottom|left|right 的任意组合（| 分隔）
margin_top = 16
margin_right = 0
margin_bottom = 16
margin_left = 0
output = ""                    # 绑定指定显示器（xdg-output 名称），空=自动
disable_fullscreen_hide = false  # 全屏时也保持可见

[pet]
name = "bongo"                 # 宠物实例名（日志/ID）
enabled = true
kind = "demo"                  # demo | bongo（角色包 M2 起支持）
color = "#ff6699"              # demo 宠物基础色（#rrggbb 或 #rrggbbaa）

[pet.bongo]                    # 仅 kind = "bongo" 时生效
assets_dir = "assets/bongocat" # 四帧 PNG 所在目录（找不到时回退到源码树）
cat_height = 110               # 猫高（像素），宽按素材宽高比自动
keypress_duration_ms = 100     # 按下后爪子保持时长（毫秒）
hand_mapping = true            # 按物理位置映射左右爪
mirror_x = false               # 水平翻转（并交换左右爪）
idle_sleep_timeout_secs = 0    # 闲置 N 秒后入睡（变暗），0=关闭
enable_scheduled_sleep = false # 定时睡眠窗口（墙钟）
sleep_begin = "22:00"          # 睡眠窗口开始（24h）
sleep_end = "06:00"            # 睡眠窗口结束（24h）
```

完整示例见 [`petweave.toml.example`](petweave.toml.example)。

### 行为说明

- **热重载**：改 `[render]` 的 layer/anchor/margins 与 `[pet]`/`[pet.bongo]` 参数即时生效（bongo 改 `cat_height` 会重新缩放）；改 `pet.kind` 或 `render.output` 需重启。
- **睡眠**：`idle_sleep_timeout_secs` 到期或处于定时窗口时显示"睡着"的猫（当前为调暗的占位帧）；定时睡眠期间按键被忽略，闲置睡眠按键即可唤醒。
- **全屏隐藏**：检测到（激活的）全屏窗口时自动隐藏宠物；`layer = "overlay"` 或 `disable_fullscreen_hide = true` 可跳过。注意：niri 未实现 `wlr-foreign-toplevel-management`，此功能在其上不生效（优雅降级，始终显示）。
- **单例**：同账号同时只能运行一个实例（flock 型 PID 文件 `$XDG_RUNTIME_DIR/petweave.pid`）。
- **多显示器/HiDPI**：`render.output` 指定显示器名称（`wlr-randr` / `niri msg outputs` 查看）；整数倍缩放按 buffer_scale 物理渲染，高分屏下清晰。
- **系统感知**：`sysinfo_interval_secs` 间隔向宠物推送 CPU/内存快照（`Event::System`，当前内置宠物暂未响应，供角色包/AI 接口使用）。

## 故障排查

| 现象 | 处理 |
|---|---|
| `list-devices` / `doctor` 显示没有键盘或权限拒绝 | 按「权限」一节配置，然后重新登录会话 |
| 宠物不响应按键 | 运行 `petweave list-devices` 确认设备被识别；在 `[input] devices` 里显式指定 |
| 提示 `another petweave instance is already running` | 已有一个实例在运行（或 `$XDG_RUNTIME_DIR` 异常） |
| 启动报 `wlr-layer-shell not available` | 当前合成器不支持 layer-shell（如 GNOME/Mutter），见兼容性说明 |
| 找不到猫素材 | 在仓库根目录运行，或把 `assets/bongocat` 复制到运行目录并配置 `assets_dir` |

## 项目结构

```
crates/
  petweave-core/   共享类型：config / events / Pet trait / render Frame
  petweave/        主机：cli / app(事件循环) / platform / graphics / runtime
assets/bongocat/   内置 BongoCat 四帧素材（MIT，署名见目录内 README）
docs/              技术栈分析 + 实现计划 + Live2D 路线
petweave.toml.example
```
