# 角色包制作教程（M2）

> PetWeave 的差异化核心：**做一个桌宠 = 写一个角色包**，不需要碰 Wayland/输入/渲染。
> 当前支持**声明式精灵宠物**（零代码）；Lua 脚本运行时因离线环境无法拉取 `mlua` 依赖，暂缓（见文末）。

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

## 仓库内示例

- `packages/bongo-sprite/` — 用声明式包复刻 BongoCat 爪击（大图缩放 + 左右爪反应），格式的"吃狗粮"验证
- `packages/blinky/` — 最小 2×2 网格示例（Codex 风格网格 + idle 循环 + 按键反应）

## 状态与下一步

- ✅ `.petweave` 格式 + 清单校验 + 安装/卸载/列表/打包 CLI + XPM 导入
- ✅ 声明式精灵宠物运行时（网格切帧、循环/一次性动画、左右爪反应、表面缩放）
- ⏳ Lua 脚本运行时：**离线环境缓存中没有 `mlua`**，联网后可加入（事件/动作 API 已在 `docs/TECH_STACK.md` §4.4 设计）
- ⏳ 签名与社区商店：包格式预留 `signature` 位置，随社区工具落地
