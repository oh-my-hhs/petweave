# 角色包制作教程（M2）

> PetWeave 的差异化核心：**做一个桌宠 = 写一个角色包**，不需要碰 Wayland/输入/渲染。
> 支持两种宠物类型：**声明式精灵宠物**（零代码）与 **Lua 脚本宠物**（mlua 沙箱）。

## 包格式 `.petweave`

一个角色包就是包含 `pet.toml`（清单）+ 素材的目录，可打包成 zip（`.petweave`）：

```
my-pet/
├── pet.toml          # 清单：元数据 + 动画 + 事件接线
└── sprites/
    └── sheet.png     # 精灵表（网格布局，如 Codex 8×9/8×11 风格）
```

## 1. 写 `pet.toml`

```toml
[meta]
name = "my-pet"              # 必须；文件系统安全名 [a-zA-Z0-9._-]
version = "1.0.0"
author = "你"
license = "MIT"
description = "我的第一个桌宠"

[pet]
kind = "sprite"              # 当前仅支持 sprite
# surface_width = 64        # 可选：表面尺寸；默认取动画格大小

[animations.idle]            # 动画：名字任意，被 [reactions] 引用
sheet = "sprites/sheet.png"  # 包内相对路径
cell_width = 32              # 一格的像素尺寸（精灵表按网格切）
cell_height = 32
frames = [0, 1, 2, 3]        # 播放顺序（行优先的格子索引）；默认 [0]
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

- **网格**：`sheet` 的宽高必须能被 `cell_width × cell_height` 整除；`frames` 是行优先格子索引（Codex 8×9/8×11 图集直接声明格大小即可）。
- **大图单帧**：若一帧就是一张大图（如 BongoCat 的 864×360 PNG），声明 `cell_width/height` 为原图尺寸，再用 `[pet] surface_width/height` 缩放到目标大小。
- **校验**：名字非法、格子不能整除、`reactions` 引用不存在的动画等都会在加载时报错。

## 2. 安装与运行

```bash
petweave install my-pet/            # 目录安装
petweave install my-pet.petweave    # 或打包后的文件
petweave list                       # 查看已安装

# 运行（配置里引用包名）
# [pet]
# kind = "sprite"
# package = "my-pet"
petweave -c petweave.toml
```

`pet.package` 也可以直接填一个**目录路径**（开发模式，免安装）；运行时加载失败会打印明确原因。

## 3. 打包分发

```bash
petweave package my-pet/ -o my-pet.petweave   # 生成 zip 包
petweave import oneko.xpm -o oneko.png        # XPM(Oneko) 精灵表 → PNG
```

## 4. Lua 脚本宠物（`kind = "lua"`）

声明式宠物之外的进阶玩法：`main.lua` 脚本驱动动画与交互，运行在 **mlua 沙箱**里
（仅暴露白名单 API，无 `io`/`os`/`package`/`debug`；指令数预算防止死循环，脚本
报错只记日志、不会拖垮宿主）。

```toml
[pet]
kind = "lua"
script = "main.lua"      # 默认 main.lua
surface_width = 64
surface_height = 64
```

```lua
-- main.lua
function init()                      -- 加载时调用一次
    pet.speak("hi! press keys")      -- 显示 4 秒气泡
end

function on_key(code, pressed)       -- 全局键盘（EV_KEY 码）
    if pressed then pet.play("flash") end
end

function on_tick(dt) end             -- 动画时钟（秒）
function on_system(cpu, mem) end     -- CPU/内存百分比
function on_fullscreen(active) end
```

**脚本 API**：

| API | 说明 |
|---|---|
| `pet.play(id)` | 播放一个动画（`pet.toml` 中声明的 id）；返回是否切换 |
| `pet.animations()` | 所有动画 id 列表 |
| `pet.current()` | 当前动画 id |
| `pet.speak(text)` | 显示一段时间的文字气泡（气泡 + 系统字体文本渲染） |
| `sys.cpu()` / `sys.mem()` | 最近一次系统采样（百分比） |
| `sys.focus()` | 当前窗口（预留，暂返回空） |

安全边界：`init`/各事件回调各自有 200 万条指令预算；`io.open` 等被禁用
（沙箱测试覆盖）。

## 仓库内示例

- `packages/bongo-sprite/` — 用声明式包复刻 BongoCat 爪击（大图缩放 + 左右爪反应），格式的"吃狗粮"验证
- `packages/blinky/` — 最小 2×2 网格示例（Codex 风格网格 + idle 循环 + 按键反应）
- `packages/lua-demo/` — Lua 脚本示例（按键说话 + 动画切换）

## 状态与下一步

- ✅ `.petweave` 格式 + 清单校验 + 安装/卸载/列表/打包 CLI + XPM 导入
- ✅ 声明式精灵宠物运行时（网格切帧、循环/一次性动画、左右爪反应、表面缩放）
- ✅ Lua 脚本运行时（mlua 沙箱：白名单 API、指令预算、事件 on_key/on_tick/on_system/on_fullscreen、动作 play/speak、查询 sys.*）
- ✅ SVG 渲染（resvg）：BongoCat 睡眠帧已用官方 SVG 素材栅格化，替代占位帧
- ⏳ 签名与社区商店：包格式预留 `signature` 位置，随社区工具落地
