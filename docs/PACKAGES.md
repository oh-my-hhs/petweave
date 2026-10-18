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
surface_width = 64           # 可选：表面盒子尺寸；默认取动画格大小（等比缩放）
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
| `surface_width/height` | — | **包内**默认盒子尺寸：素材按比例缩放进这个盒子（不拉伸）。用户在 `petweave.toml` 里改尺寸要写 `width/height`（不是 `surface_width/height`，后者在用户配置里会被忽略并告警） | 格子大小 |
| `script` | kind=lua | Lua 入口文件名 | `main.lua` |
| `key_mode` | — | `paw`：老行为，按左/右半键盘一次性播放 `key-left`/`key-right`（BongoCat 式）；`press`：按下算一次"点按"，按住期间停留，见下文 | `paw` |
| `alternate` | — | `press` 模式：每次点按在 `key-left` / `key-right` 两张反应图之间**交替** | `false` |
| `force_alternation` | — | `press` 模式：不论按的是左半还是右半键盘都交替（两张图都是"击打"动作时用） | `false` |
| `padding` | — | 移动余量：四周各留 `N` 像素给脚本 `pet.offset` 用，不参与裁剪；0 = 不留 | `0` |

以上键名带下划线和带连字符（`key-mode`…）都可以，两种写法等价。

`press` 模式的语义：按下是"点按"，松手回到 `idle`；**按住不松手就一直停在当前姿势**（不会
自动超时）。按住期间新按下的键会顶替姿势（另一只手/另一张图），松手时回到 `idle`。自动重复
的按下会被忽略；键盘的按住/松手也能驱动 Lua 的 `on_key(code, pressed)`。

**摆放模型（只有一条规则）**：图片按盒子的比例等比缩放，**贴表面左下角**绘制，再叠加脚本用
`pet.offset(dx, dy)` 给的位移（x 向右、y 向下，`(0,0)` 是原位）。`padding` 是四周对称的余量，
所以静止时图片距离底边恒为 `padding`，上跳/下落都能在余量里移动而不被裁掉。

> 早期版本的 `anchor_mode` / `anchor_padding` / `anchor_x` / `bob_pixels` 已移除（宿主不再
> 内置弹跳）。旧包里的这些键会被忽略、不影响加载；需要弹跳的包改用 `padding` + `pet.offset`。

### `[animations.<id>]`

| 字段 | 必填 | 说明 | 默认 |
|---|---|---|---|
| `sheet` | ✅ | 素材 PNG 的包内路径 | — |
| `cell_width/height` | ✅ | 格子像素尺寸；素材宽高必须能整除 | — |
| `frames` | — | 播放顺序（行优先格子索引） | `[0]` |
| `fps` | — | 播放帧率 | `1` |
| `loop` | — | 是否循环 | `false` |

### `[reactions]` / `[[reactions]]`

TOML 里 `[reactions]` 表与 `[[reactions]]` 数组是**同一个键**，一个文件只能用其中一种写法：

* 表（老写法）：具名反应，见下表；
* 数组（给"逐键反应"用）：每个元素可写具名字段，或写 `keys` + `animation`。

| 字段 | 说明 |
|---|---|
| `idle` | 无反应时的循环动画 |
| `key-left` / `key-right` | 左/右半键盘按键时的反应（`paw` 模式一次性播放；`press` 模式停留到松手） |
| `key-both` | 双手同按时播放（仅 `paw` 模式） |
| `click` | 左键点击宠物时播放（一次性） |
| `keys` + `animation` | 逐键反应：按下列表中的键（Linux EV_KEY 码）时显示该动画；`press` 模式下按住即停留、松手回 `idle` |

```toml
# 数组写法：一条具名，加三条逐键（主键盘 1/2/3）
[[reactions]]
idle = "basic"

[[reactions]]
keys = [2]
animation = "pattern0"
```

表写法里的逐键条目写成 `[[reactions.extra]]`（同一数组的旧拼法），不能与 `[reactions]` 混用。
逐键条目 `keys` 至少一个、`animation` 必须存在，否则加载报错；同一个键出现多次时后者生效。

校验规则：名字非法、格子不能整除、`frames` 越界、`reactions` 引用不存在的动画、`key_mode`
非法 → 加载报错并给出明确信息。

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

function on_pointer(x, y, pressed, button)
    if pressed and button == 272 then
        pet.speak("pet me!")
    end
end
```

### 脚本 API 一览

| API | 说明 |
|---|---|
| `pet.play(id)` | 播放 `pet.toml` 中声明的动画；返回是否切换 |
| `pet.animations()` | 所有动画 id 列表 |
| `pet.current()` | 当前动画 id |
| `pet.speak(text)` | 显示 4 秒文字气泡（气泡 + 系统字体渲染） |
| `pet.held()` | 当前是否有键按住（`press` 模式下由键盘驱动） |
| `pet.size(w, h)` | 把表面改成 `w`x`h`，图片按比例缩放进去（不拉伸） |
| `pet.w()` / `pet.h()` | 当前表面宽/高 |
| `pet.offset(dx, dy)` | 图片精确位移：x 向右、y **向下**（屏幕坐标），`(0,0)` 归位；不会自己回落，超出 `padding` 的部分被表面裁掉 |
| `sys.cpu()` / `sys.mem()` | 最近一次系统采样（百分比） |
| `sys.focus()` | 当前窗口信息（预留，暂返回空串） |
| `on_pointer(x, y, pressed, button)` | 指针事件（悬停/移动/按下/松开，`button=272` 为左键） |

### 脚本接管按键与尺寸（`OWN_KEYS` / `OWN_PIXELS`）

脚本顶部给一个全局变量，告诉宿主"画面由脚本说了算"（写在文件顶层，加载时读一次）：

| 声明 | 效果 |
|---|---|
| `OWN_KEYS = true` | 精灵变成纯画布：不再按 `key_mode` / `[[reactions]]` 抢键，动画只由 `pet.play` 决定；所有键都送到 `on_key` |
| `OWN_PIXELS = true` | 保留 `pet.toml` 的按键处理，但允许脚本用 `pet.size` / `pet.offset` 精确摆放 |

自绘尺寸的语义：`pet.size(w, h)` 给的是**表面盒子**大小，图片等比缩放进去、贴左下角绘制。
用户配置里的 `[[pets]].width/height` 优先于它（Q1A：显式配置是契约），只写一边时另一边按原图
比例推算。`--preview` 会先让这类脚本跑一次 `on_tick` 再截图，所以导出的是脚本设定的尺寸。

需要"上跳/抖动"的包：在 `pet.toml` 设 `padding = 6`，然后在 `on_tick` 里按时间调用
`pet.offset(0, -dy)`（负 y = 向上）。宿主不再提供内置弹跳。

声明了 `OWN_KEYS` 的宠物在脚本静止（没有按住、没有动画）时会让宿主睡眠，保持空闲 ~0% CPU。
按键回调里做的任何改动（`pet.play` / `pet.size` / `pet.offset`）都会触发重绘。

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
- `packages/krill-pet/`、`packages/puro-pet/` — **原图直用**的成品包：六张 800×500 原图分别作为
  `basic/left/right/pattern0/pattern1/pattern2`（每张整图就是单格精灵表），按 140px 高等比显示为
  224x140、内容贴左下角，尺寸固定不变。`main.lua` 声明 `OWN_KEYS` 自己管键：**任意键**（键盘上除
  1/2/3 的键，左右两侧、功能键、方向键都算）每按一次在 left/right 间交替，按住停留，松开回
  basic；1/2/3 键显示 pattern0/1/2 并按住停住。这是最简的"按键轮切 + 停手恢复"脚本，可作为
  类似包的地基。

## 状态与下一步

- ✅ `.petweave` 格式 + 清单校验 + 安装/卸载/列表/打包 CLI + XPM 导入
- ✅ 声明式精灵宠物运行时（网格切帧、循环/一次性动画、左右爪反应、表面缩放、
  `press` 语义 + 交替 + 抖动、逐键反应）
- ✅ Lua 脚本运行时（mlua 沙箱：白名单 API、指令预算、事件 on_key/on_tick/on_system/on_fullscreen/on_scroll、动作 play/speak/size/offset/held/bump、查询 sys.*；`OWN_KEYS`/`OWN_PIXELS` 脚本接管按键与摆放）
- ✅ SVG 渲染（resvg）：BongoCat 睡眠帧已用官方 SVG 素材栅格化，替代占位帧
- ✅ 参考包端到端：`krill-pet` / `puro-pet` 的交替、按住停留、表情键与尺寸固定在宿主里由测试
  直接驱动真实按键验证（见 `docs/ROADMAP.md` M2.6）
- ⏳ 可配置键位/长按时长：目前写在各包 `main.lua` 常量里，规划见 `docs/ROADMAP.md` M2.5
- ⏳ 签名与社区商店：包格式预留 `signature` 位置，随社区工具落地
