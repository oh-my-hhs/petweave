# 角色包制作教程

> PetWeave 的差异化核心：**做一个桌宠 = 写一个角色包**，不需要碰 Wayland/输入/渲染。
> 支持两种宠物类型：**声明式精灵宠物**（零代码）与 **Lua 脚本宠物**（mlua 沙箱）。
> 本教程从零开始，带你做完第一个角色包并发布给社区。

## 0. 角色包是什么

一个角色包就是**一个目录**（可打包成 `.petweave` zip），包含：

```
my-pet/
├── pet.toml          # 清单：元数据 + 动画 + 事件接线（必填）
├── main.lua          # Lua 行为脚本（仅 kind = "lua"）
└── sprites/
    ├── sheet.png     # 精灵表（网格布局，如 Codex 8×9/8×11 风格）
    └── ...           # 任意素材，包内路径引用
```

安装后由运行时加载，与内置宠物体验完全一致：热重载、键盘事件、系统感知全都免费获得。

---

## 1. 从零创建：第一个宠物 "my-pet"

### 1.1 准备素材

**推荐：网格精灵表（Codex 风格）**。一张 PNG，按格子切帧：

- 常见规格：8×9 / 8×11 的等尺寸格子（很多现成素材直接可用）
- 每个格子一张"姿势"：待机、走路、反应、睡觉……
- 透明背景（PNG alpha），避免黑边

没有现成精灵表？两条路：

```bash
# Oneko 风格 XPM → PNG（导入器内置）
petweave import oneko.xpm -o sprites/sheet.png

# 或者每张姿势一张大图（如 BongoCat 864×360 PNG），包格式同样支持
```

### 1.2 写 `pet.toml`

```toml
[meta]
name = "my-pet"              # 必须；文件系统安全名 [a-zA-Z0-9._-]
version = "1.0.0"            # 版本号（建议语义化）
author = "你"
license = "CC0-1.0"          # 素材与包的许可证
description = "我的第一个桌宠"

[pet]
kind = "sprite"              # sprite（零代码）或 lua（脚本）
surface_width = 64           # 可选：表面尺寸；默认取动画格大小
surface_height = 64

[animations.idle]            # 动画：名字任意，被 [reactions] 引用
sheet = "sprites/sheet.png"  # 包内相对路径
cell_width = 32              # 一格像素宽（精灵表按网格切）
cell_height = 32
frames = [0, 1, 2, 3]        # 播放顺序（行优先格子索引）；默认 [0]
fps = 4                      # 帧率；默认 1
loop = true                  # 循环；默认 false

[animations.flash]
sheet = "sprites/sheet.png"
cell_width = 32
cell_height = 32
frames = [3]
fps = 8

[reactions]                  # 事件接线（kebab-case 键名）
idle = "idle"                # 无事时的循环动画
key-left = "flash"           # 按到左半键盘
# key-right = "..."          # 右半键盘
# key-both = "..."           # 双手同按
```

> 没有 `[animations]`/`[reactions]` 的包也可以（Lua 纯脚本宠物），但至少要有素材被引用。

### 1.3 本地调试（无需安装、无需 Wayland）

```bash
# 直接指向包目录（开发模式），把当前帧导出为 PNG 检查效果：
petweave -c <(printf '[pet]\nkind = "sprite"\npackage = "my-pet"\n') --preview out.png
# 或在配置里写 package = "/绝对/路径/my-pet"
```

`--preview` 是角色包开发最重要的工具：**不启动 Wayland 就能看到宠物长什么样**。

### 1.4 安装运行

```bash
petweave install my-pet/          # 复制进本地仓库
petweave list                     # 确认已安装
cat > ~/.config/petweave/petweave.toml <<'EOF'
[pet]
kind = "sprite"
package = "my-pet"
EOF
petweave                          # 运行！按键盘左半区触发 flash
```

### 1.5 打包与分发

```bash
petweave package my-pet/ -o my-pet.petweave   # 生成 zip 包
petweave uninstall my-pet
petweave install my-pet.petweave              # 从包文件重装（验证包完整性）
```

---

## 2. 清单字段全参考

### `[meta]`

| 字段 | 必填 | 说明 | 默认 |
|---|---|---|---|
| `name` | ✅ | 包名，文件系统安全 `[a-zA-Z0-9._-]`；安装目录 = 包名 | — |
| `version` | — | 版本号 | `1.0.0` |
| `author` | — | 作者 | 无 |
| `license` | — | 许可证（SPDX 标识），决定能否进社区商店 | 无 |
| `description` | — | 一句话介绍（`petweave list` 显示） | 无 |

### `[pet]`

| 字段 | 必填 | 说明 | 默认 |
|---|---|---|---|
| `kind` | ✅ | `sprite` 或 `lua` | — |
| `surface_width/height` | — | 表面像素尺寸；不填 = 第一个动画的格子大小；大图素材用它缩放 | 格子大小 |
| `script` | kind=lua | Lua 入口文件名 | `main.lua` |

### `[animations.<id>]`

| 字段 | 必填 | 说明 | 默认 |
|---|---|---|---|
| `sheet` | ✅ | 素材 PNG 的包内路径 | — |
| `cell_width/height` | ✅ | 格子像素尺寸；素材宽高必须能整除 | — |
| `frames` | — | 播放顺序（行优先格子索引） | `[0]` |
| `fps` | — | 播放帧率 | `1` |
| `loop` | — | 是否循环 | `false` |

### `[reactions]`

| 字段 | 说明 |
|---|---|
| `idle` | 无反应时的循环动画 |
| `key-left` / `key-right` | 左/右半键盘按键时播放（一次性） |
| `key-both` | 双手同按时播放 |

校验规则：名字非法、格子不能整除、`frames` 越界、`reactions` 引用不存在的动画 → 加载报错并给出明确信息。

---

## 3. Lua 进阶玩法（`kind = "lua"`）

脚本运行在 **mlua 沙箱**：白名单环境（无 `io`/`os`/`package`/`debug`/`require`），每次事件回调有 200 万条指令预算（死循环自动中止），脚本报错只记日志、不拖垮宿主。

```toml
[pet]
kind = "lua"
script = "main.lua"
surface_width = 64
surface_height = 64
```

```lua
-- main.lua
local since = 0

function init()
    pet.speak("hi! press keys")
end

function on_key(code, pressed)
    if pressed then
        pet.play("flash")
        pet.speak("key " .. code)
    end
end

-- on_tick 是实现"定时行为"的惯用入口（dt = 距上次 tick 的秒数）
function on_tick(dt)
    since = since + dt
    if since > 10 then
        since = 0
        pet.speak("I'm still here!")
    end
end

function on_system(cpu, mem)
    if cpu > 90 then pet.speak("cpu is hot!") end
end

function on_fullscreen(active)
    if active then pet.speak("fullscreen!") end
end
```

### 脚本 API 一览

| API | 说明 |
|---|---|
| `pet.play(id)` | 播放 `pet.toml` 中声明的动画；返回是否切换 |
| `pet.animations()` | 所有动画 id 列表 |
| `pet.current()` | 当前动画 id |
| `pet.speak(text)` | 显示 4 秒文字气泡（气泡 + 系统字体渲染） |
| `sys.cpu()` / `sys.mem()` | 最近一次系统采样（百分比） |
| `sys.focus()` | 当前窗口信息（预留，暂返回空串） |

### 沙箱边界（写脚本时注意）

- 没有文件系统/进程/网络 API —— 需要外部数据请通过未来版本的 `net.*` 预留接口
- 没有 `require`/`package` —— 单文件脚本，复用逻辑请写在同一个文件里
- 死循环/超大计算会被指令预算中止，然后**该次回调被跳过**（宠物继续运行）

---

## 4. 调试技巧

| 场景 | 方法 |
|---|---|
| 看宠物长什么样 | `petweave --preview out.png`（无需 Wayland，导出当前帧） |
| 素材网格不对 | 先本地 `petweave --preview`，再检查 `cell_width/height` 是否整除 |
| 动画不切换 | 确认 `[reactions]` 引用的 id 与 `[animations.<id>]` 一致（错误信息会点名） |
| Lua 脚本报错 | 日志会出现 `lua <handler>: <错误>`，先在本机 `lua main.lua` 语法检查 |
| 安装后加载失败 | `petweave list` 看包是否在；`package` 字段填包名（非路径）时检查仓库目录 |

---

## 5. 发布与生态

### 发布前自检清单

- [ ] `petweave package my-pet/ -o my-pet.petweave` 打包成功
- [ ] 卸载后从 `.petweave` 重装一次，`--preview` 正常
- [ ] `pet.toml` 填了 `author`、`license`、`description`
- [ ] 素材无版权问题（自己画的 / 有许可的）
- [ ] 准备一张封面图（`petweave --preview` 输出即可）

### 发布渠道

- **GitHub Releases / Gitee**：把 `.petweave` 文件作为发布资产上传
- **Awesome 列表**：向社区仓库提交你的包（见 [CONTRIBUTING.md](CONTRIBUTING.md)）
- **未来**：社区商店与签名机制（包格式已预留 `signature` 位置）

### 命名与规范建议

- 包名小写、短横线分隔：`my-cat`、`miku-dance`、`shimeji-rem`
- 版本号语义化：`1.0.0` → `1.1.0`（加动画）→ `2.0.0`（破坏性变更）
- 重名处理：先安装先得；同名升级请保持包名一致（`install` 会自动替换）

---

## 6. 仓库内示例（现成的学习素材）

- `packages/bongo-sprite/` — 用声明式包复刻 BongoCat 爪击（大图缩放 + 左右爪反应），格式的"吃狗粮"验证
- `packages/blinky/` — 最小 2×2 网格示例（Codex 风格网格 + idle 循环 + 按键反应）
- `packages/lua-demo/` — Lua 脚本示例（按键说话 + 动画切换）

## 状态与下一步

- ✅ `.petweave` 格式 + 清单校验 + 安装/卸载/列表/打包 CLI + XPM 导入
- ✅ 声明式精灵宠物运行时（网格切帧、循环/一次性动画、左右爪反应、表面缩放）
- ✅ Lua 脚本运行时（mlua 沙箱：白名单 API、指令预算、事件 on_key/on_tick/on_system/on_fullscreen、动作 play/speak、查询 sys.*）
- ✅ SVG 渲染（resvg）：BongoCat 睡眠帧已用官方 SVG 素材栅格化，替代占位帧
- ⏳ 签名与社区商店：包格式预留 `signature` 位置，随社区工具落地
