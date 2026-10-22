# M3.5 · G3 验收手册（布局画布 + 实时预览）

> **用途**：G3 的验收手册 + 验收记录。
> 2026-10-22 第一轮只写代码与自动化测试；同日晚在 niri 上补齐了**实机验收**（§6），
> 过程中发现并修掉了 2 个只有真拖拽才会暴露的问题（§7）。
> 本文保留「怎么做、看什么、什么算通过」的步骤，方便换机器/换合成器复跑。
>
> 相关文档：[ROADMAP](ROADMAP.md) · [GUI_ROADMAP §9 G3](GUI_ROADMAP.md) · [ARCHITECTURE](ARCHITECTURE.md) · [README](../README.md)

---

## 0. 本轮改了什么（范围）

| 位置 | 内容 |
|---|---|
| `crates/petweave-settings/src/canvas.rs`（新） | 纯几何：`CanvasMap` 输出↔画布可逆映射、`snap_axis`/`snap_move` 吸附、`pet_rect` / `placement_at` / `apply_position`（anchor+margin ↔ rect 往返）、`apply_bottom_center` / `clear_placement` 预设、`hit_test`、`output_pages`/`page_for_pet` 分页、`CanvasState`（当前页/吸附开关/拖拽/去重缓存） |
| `crates/petweave-settings/src/preview.rs`（新） | `PreviewCache`：用**宿主同款** `petweave::runtime::Runtime` 渲染，`preview_key` 只在渲染输入变化时重建，10fps 节流，纹理缓存；`downscale_rgba` 预乘 alpha 降采样 |
| `crates/petweave-settings/src/ui/layout.rs`（新） | 布局页：显示器分页画布、宠物缩略图 + 名字、选中高亮、拖拽 + 吸附参考线、anchor 复选 + 四边 margin 表单、两枚预设、其它显示器宠物计数 |
| `crates/petweave-settings/src/ui/pets.rs` | 列表页新增**缩略图列**；预览加载失败给计数提示 |
| `crates/petweave-settings/src/ui/params.rs` | 参数页右上角**动画预览**（点击强制刷新） |
| `crates/petweave-settings/src/ui/about.rs` | 「关于」页新增预览运行时信息 + 预览加载失败原因（G4 诊断页之前的临时落点） |
| `crates/petweave-settings/src/app.rs` | `Page::Layout`、`preview`/`canvas` 状态、`output_pages`/`canvas_pets`/`set_pet_position`/`push_placement`/`resend_placement`/预设入口、布局页 2s 刷新 `ListPets`+`ListOutputs` |
| `crates/petweave-settings/src/client.rs` | `Client::set_placement`（`SetPlacement` 发送；未握手时丢弃） |
| 测试（新） | `canvas` 6 项、`preview` 4 项、`client` 2 项单测；`tests/ui_layout.rs` 6 项（kittest + API）、`tests/layout_host.rs` 1 项（脚本宿主） |
| 实机验收修复（2026-10-22） | `ui/layout.rs`：拖拽抓取偏移改用 `press_origin()`（原来会让宠物按拖拽阈值的位移整体跳一下）；画布背景 sense 改为 `hover`（不再与宠物框抢 press）。详见 §7 |
| **宿主 `crates/petweave`** | **零改动**（控制通道协议、渲染路径都没有变更，风险面全在 GUI 进程） |

上一次稳定提交：`d1fd37f docs(M3.5 G2): sync README and ROADMAP with the shipped fixes`。

---

## 1. 自动化回归（不需要桌面，先跑这个）

```bash
CARGO_HOME=$PWD/.cargo-home cargo build --offline
CARGO_HOME=$PWD/.cargo-home cargo test  --offline        # 期望：186 项全绿
CARGO_HOME=$PWD/.cargo-home cargo clippy --offline --all-targets
```

通过标准：

1. `cargo test` 汇总 **186 项**（103 host + 38 core + 45 settings）。settings 的 45 项 =
   34 库测试 + `ui_save` 2 + `ui_layout` 6 + `layout_host` 1 + `reload_path` 2。
2. `clippy` 只允许**既有**告警（`manifest.rs` / `render.rs` / `graphics/mod.rs` / `package.rs` /
   `demo.rs` / `app.rs` 测试里的 `field_reassign_with_default`）。`petweave-settings` 必须零告警 ——
   若出现新告警，先修再加功能。
3. 宿主零 GUI 依赖（N01）：

   ```bash
   cargo tree -p petweave --offline | grep -Ei 'egui|eframe|gtk|winit'   # 必须无输出
   ```

只想跑 G3 相关的部分：

```bash
CARGO_HOME=$PWD/.cargo-home cargo test --offline -p petweave-settings --lib canvas:: \
  && CARGO_HOME=$PWD/.cargo-home cargo test --offline -p petweave-settings --lib preview:: \
  && CARGO_HOME=$PWD/.cargo-home cargo test --offline -p petweave-settings --test ui_layout \
  && CARGO_HOME=$PWD/.cargo-home cargo test --offline -p petweave-settings --test layout_host
```

**G3 独有的自动化断言**（这些已经证明了逻辑正确，桌面验收只需确认"看起来对"）：

| 断言 | 测试 |
|---|---|
| 拖到 `(640, 300)` → 保存 → 重新解析后 `pet_rect` 的 x/y 误差 < 0.5px | `ui_layout::drag_then_save_round_trips_the_position` |
| 距右边缘 3px → 吸附贴边（`guide_x = 输出宽度`），并保留 `bottom|right` 角 | `ui_layout::drag_normalises_an_edge_anchor_and_snaps_to_the_edge` |
| 画布按 `output` 分页、显示宿主上报的真实 rect；`SetPlacement` 报文里 `pet/anchor/margins` 正确 | `layout_host::canvas_uses_host_outputs_and_rects_then_pushes_placement` |
| 「放到底部居中」在 `[render]` 已是底部 16px 时**一个键都不写**；「恢复默认位置」清空四边 margin | `ui_layout::layout_page_renders_and_presets_edit_the_draft`、`…::layout_page_lists_pets_per_output_and_reset_restores_inheritance` |
| 改名不重建预览运行时，改尺寸才重建 | `ui_layout::preview_builds_a_runtime_for_every_enabled_pet` |
| 缩略图降采样不产生黑边（预乘 alpha） | `preview::tests::downscale_does_not_darken_alpha_edges` |
| 未握手时 `SetPlacement` 被丢弃（不会写坏 socket） | `client::tests::set_placement_is_dropped_without_a_handshake` |

---

## 2. 桌面环境准备

```bash
# 1) 构建（宿主与面板都要）
CARGO_HOME=$PWD/.cargo-home cargo build --release --offline -p petweave -p petweave-settings

# 2) 用一份"三只宠物、两只在不同显示器/角落"的配置起宿主
XDG_RUNTIME_DIR=$PWD/.tmp-run WAYLAND_DISPLAY=$WAYLAND_DISPLAY \
  ./target/release/petweave -c /tmp/g3-test.toml --verbose &

# 3) 打开设置面板（同一个 -c！否则保存不会生效）
./target/release/petweave settings -c /tmp/g3-test.toml
```

`/tmp/g3-test.toml`（三只宠物：BongoCat 底部居中、demo 左上、blinky 右下；注释留一行用来验证"保注释"）：

```toml
# G3 验收配置：改完请确认这行注释还在
[general]
fps = 60

[render]
width = 200
height = 120

[[pets]]
id = "bongo-main"
name = "BongoCat"
kind = "bongo"
anchor = "bottom"
margin_bottom = 16

[[pets]]
id = "demo-1"
name = "Demo 左上"
kind = "demo"
anchor = "top|left"
margin_top = 40
margin_left = 40

[[pets]]
id = "blinky-1"
name = "Blinky 右下"
kind = "sprite"
package = "blinky"
anchor = "bottom|right"
margin_bottom = 40
margin_right = 40
```

niri 上设置窗口会被平铺，建议加浮动规则（Hyprland 同理）：

```kdl
// ~/.config/niri/config.kdl
window-rule {
    match app-id="petweave-settings"
    open-floating true
    default-column-width { fixed 1100px; }
}
```

**前置检查**：面板顶栏必须显示绿色「已连接（3 只宠物）」。
若显示「宠物未运行」，先看 `$XDG_RUNTIME_DIR/petweave/control.sock` 是否存在，
以及 `./target/release/petweave control -c /tmp/g3-test.toml '{"op":"ListPets"}'` 是否有响应。

---

## 3. 桌面验收清单（逐项做，逐项打勾）

> 2026-10-22 已按本节在 niri 上走完，结论见 §6；本节保留为复跑步骤（换合成器/换机器时照做）。

> 记法：**操作** → **预期**。任何一项不符，按 §5 排查。

### 3.1 画布与分页（F21）

- [ ] **操作**：点侧栏「布局」。
  **预期**：出现一块深色画布，中间是等比缩放的显示器矩形（左上角写着「显示器名（宽×高 @缩放）」），
  画布上有 3 个带名字的宠物框，BongoCat 的图能认出来（不是灰块）。
- [ ] **操作**：如果宠物 `output` 分别设成不同显示器，切换画布顶部的显示器下拉。
  **预期**：每页只显示属于该显示器的宠物；其它显示器上的宠物数量显示在下方（"N 只宠物在其它显示器"）。
  宠物框的相对位置与桌面上一致（同一显示器内）。
- [ ] **操作**：在桌面上手动拖动一只宠物到别处（宿主拖拽），观察画布。
  **预期**：画布上该宠物框在 ~1 秒内跟到新位置（`PetMoved`/2s `ListPets` 回流）。

### 3.2 拖拽与实时生效（F22）

- [ ] **操作**：在画布上按住一只宠物拖动。
  **预期**：桌面上的该宠物**跟手移动**（感知延迟应 < 50ms）；画布上的框同步移动；
  鼠标指针是"抓取"形状。
- [ ] **操作**：松手。
  **预期**：侧栏下方状态提示「已更新配置（点「保存并应用」写入文件）」，顶栏出现「有未保存的修改」。
- [ ] **操作**：点「保存并应用」。
  **预期**：文件里该宠物出现 `anchor = "..."` 与对应的 `margin_*`（**只写能生效的那条边**，
  例如 `bottom|left` 只写 `margin_bottom` + `margin_left`）；文件首行注释 `# G3 验收配置` 仍在；
  桌面位置不跳变。

### 3.3 吸附（F23）

- [ ] **操作**：把宠物拖到靠近左边缘（< 6px）。
  **预期**：自动吸附贴边，画布上出现橙色竖向参考线。
- [ ] **操作**：拖到水平方向靠近中间。
  **预期**：吸附到中心线（橙色参考线），松手后配置里 `margin_left/right` 对应值使宠物居中。
- [ ] **操作**：拖到远离边/中心的任意位置。
  **预期**：吸附到 8px 网格（位置是 8 的倍数）。
- [ ] **操作**：取消勾选「吸附」再拖。
  **预期**：完全跟手、不再对齐，也没有橙色参考线。

### 3.4 表单双向同步与预设（F20/F24）

- [ ] **操作**：在下方表单里点 anchor 的「上/下/左/右」复选。
  **预期**：桌面宠物立刻贴到对应边；画布上的框同步；切回勾选前的状态能还原。
- [ ] **操作**：改 `margin_left` 数值（DragValue 拖动）。
  **预期**：桌面宠物即时水平移动；画布框同步。
- [ ] **操作**：点「放到底部居中」。
  **预期**：宠物移到底部居中；配置里只留下必要的键（若 `[render]` 本来就是底部 16px，则**什么都不写**）。
- [ ] **操作**：点「恢复默认位置」。
  **预期**：该宠物的 `anchor` 与四边 `margin_*` 全部从文件里消失，位置回到 `[render]` 的默认（底部居中）。

### 3.5 预览（F30）

- [ ] **操作**：回到「宠物列表」。
  **预期**：每只启用的宠物左侧有一张缩略图，与桌面显示**同一帧**（BongoCat 睡眠时缩略图也是睡眠帧）；
  被停用的宠物显示灰块而不是图。
- [ ] **操作**：进「参数」页，看右上角预览。
  **预期**：动画在动（约 10fps），尺寸/内容与桌面一致；点一下预览会立刻刷新一帧。
- [ ] **操作**：`top -p $(pgrep -f petweave-settings) ` 观察 CPU。
  **预期**：预览运行时 CPU 增量应为个位数百分比；切到「关于」页后回落（预览只在显示时跑）。

### 3.6 显示器热插拔（F21 后半）

- [ ] **操作**：拔掉第二块显示器（或 `wlopm --off`）。
  **预期**：画布页数减少，当前页自动校正（不会停在空页）；原本在该显示器上的宠物回落到第一页。
- [ ] **操作**：插回。
  **预期**：页面重新出现（2 秒内）。

### 3.7 离线与进程边界（F33/F34）

- [ ] **操作**：`kill` 掉宿主，再打开设置面板。
  **预期**：顶栏灰色「宠物未运行」；布局页正常显示，画布顶部有黄色提示
  「宿主未连接：画布使用示意尺寸（1920×1080）…」；拖拽只改配置、保存仍成功（下次启动生效）。
- [ ] **操作**：先开面板，再启动宿主。
  **预期**：2 秒内自动连上（绿色"已连接"），画布切换到真实显示器尺寸；若保存过，会自动补发 `Reload`。
- [ ] **操作**：`kill` 掉设置面板。
  **预期**：桌面宠物继续动画（面板是独立进程）。

---

## 4. 需要记录进文档的数值

把实测结果写回 `docs/GUI_ROADMAP.md` §14（表格已有位置）与本文末尾的"本轮实测"表：

| 指标 | 怎么测 | G2 基线（参考） | G3 实测（2026-10-22） |
|---|---|---|---|
| `petweave-settings` release 体积 | `ls -l target/release/petweave-settings` | 12.39 MiB | **15.03 MiB**（15,761,152 B） |
| 宿主 release 体积 | `ls -l target/release/petweave` | 7.73 MiB | **8.01 MiB**（8,399,768 B，宿主零改动，差额来自 G2 后的修复提交） |
| GUI 常驻 RSS | `/proc/<pid>/status` 的 `VmRSS` | ≈190 MB（软件 GL） | **139.6 MiB**（142,968 kB，Xvfb + llvmpipe） |
| GUI CPU（布局页，10fps 预览开） | `/proc/<pid>/stat` utime+stime 差 | — | **≈139% 单核**（1110 ticks / 8s，软件光栅） |
| GUI CPU（关于页，预览关） | 同上 | — | **≈2% 单核**（16 ticks / 8s）→ 预览确实是主要开销 |
| 宿主空闲 CPU（3 宠物） | 同上 | ~0% | **0 ticks / 10s（≈0%）** ✅ |
| 宿主 RSS（1 BongoCat + 2 demo） | `VmRSS` | ≈14.9 MB（单只 scale 2） | **17.9 MiB**（18,336 kB） |
| 拖拽落点精度 | 拖到 (500,200) 后读 `ListPets` | — | **(504,200)**：y 完全一致，x 是 8px 网格吸附结果（500 落在 496/504 中间，取 504）✅ |

---

## 5. 出问题时的排查顺序

1. **画布上宠物是灰块 / 没有图**：GUI 进程里该宠物加载失败（包缺失、脚本报错）。
   去「关于」页看 `预览运行时` 下方的错误行——列表页也会显示"N 只无法预览"。
   宿主能显示说明只是 GUI 侧的包路径问题（GUI 与宿主必须能读到同一个 `$XDG_DATA_HOME/petweave/pets`）。
2. **拖拽后桌面不动**：顶栏是否"已连接"？`petweave control '{"op":"ListPets"}'` 能否拿到宠物？
   控制通道断线时面板只会写配置（状态栏会这么说）。
3. **拖拽跳变/回弹**：`PetMoved` 回流与本地拖拽打架。
   `RUST_LOG=debug ./target/release/petweave` 观察 `control: set_placement` 与 `PetMoved` 的频率；
   若宠物处于 `mode = "physics"`，物理会继续拉它——这是已知边界（画布不接管物理），
   验收时建议用 `mode = "drag"`。
4. **吸附"太软"或"太硬"**：阈值是 6 逻辑像素（`canvas::SNAP_DISTANCE`）。
   在 scale=2 的屏幕上逻辑像素=物理像素/2，若观感不对，调这个常量并补一条 `snap_axis` 单测。
5. **保存后配置出现奇怪数字**：`bottom` 锚点的宠物拖到屏幕上方时，`margin_bottom` 会变成大数
   （= 输出高度 − y − 高度）。这是设计（保留用户原本"贴底"的语义），不是 bug。
6. **跨显示器拖动**：v1 **不支持**（一次只在一个 output 的坐标系里拖，超出画布会被钳制在边内）。
   要换显示器请在「参数」页改 `显示器` 字段。这条如果被认为必须支持，记进 GUI_ROADMAP §13 新 Q。
7. **设置窗口一闪而过 / 打不开**：`RUST_LOG=info ./target/release/petweave-settings` 看错误；
   找不到 GL 时按 N04 处理（wgpu 回退仍是 G4 任务）。

---

## 6. 实机验收记录（2026-10-22，niri 26.04）

**环境**：niri 26.04，单显示器 `eDP-1` 2560×1600 @1.5（逻辑 1706×1066；宿主内部按 1707×1067 算）。
宿主 `petweave` 跑在**真实 Wayland 会话**上（三只宠物：1×BongoCat + 2×demo），
设置面板跑在 `Xvfb 1500×950` 的 **X11 后端**上（沙箱里没有输入注入设备，用 `xdotool` 驱动面板、
`import` 截面板、`grim` 截真实桌面并逐像素核对宠物位置）。面板与宿主通过同一个 `control.sock` 通信，
渲染路径与真实使用完全相同；差异只在于面板窗口显示在 Xvfb 而不是用户的 niri 桌面。

| 项 | 结果 | 证据 |
|---|---|---|
| F21 显示器分页 + 真实 rect | ✅ | 画布显示 `eDP-1（1707×1067 @2x）`，显示器矩形 + 100px 标尺 + 中心线；三只宠物框位置与 `ListPets` 的 rect 换算一致（BongoCat 722,941 → 画框 (528,440)，误差 < 1px） |
| F21 显示器几何变化（热插拔代理） | ✅ | `niri msg output eDP-1 scale 1.0` → `ListOutputs` 变 2560×1600@1x，面板 2s 内把画布重排、下拉标题同步；宠物按新尺寸重算（BongoCat 1148,1474）。恢复 1.5 后全部回到原样 |
| F22 拖拽 → 实时 `SetPlacement` | ✅ | 拖拽过程中 `ListPets` 已在变（宿主跟手）；松手后 `demo-1` rect = **(504,200,200,120)** |
| F22 桌面真的跟手 | ✅ | `grim` 截图逐像素测量：蓝宠物出现在逻辑 **(504.0,200.0)**，200×120，与画布/配置/宿主三方一致 |
| F22 落定写配置（保注释） | ✅ | 文件写入 `anchor = "bottom|left"`、`margin_bottom = 747`、`margin_left = 504`；首行注释 `# G3 验收配置…` 保留；`.bak` 生成（0600） |
| F23 8px 网格吸附 | ✅ | 两次拖拽的落点均为 8 的倍数（464,176 / 504,200） |
| F23 边缘吸附 | ✅ | 拖向左边缘 → `snapped.x = 0`，宿主 rect x=0，配置 `margin_left = 0` |
| F23 吸附参考线 | ✅ | 拖拽中截图可见橙色竖线（x=0 / x=536）与横线（y=176） |
| F23 吸附开关 | ✅ | 取消勾选「吸附」后 `canvas.snap=false`（kittest 与手工均可切换） |
| F24 「恢复默认位置」 | ✅ | `demo-1` 的 `anchor`/`margin_*` 全部从文件消失，rect 回到 `[render]` 语义的底部居中 **(754,931)** |
| F24 「放到底部居中」 | ✅ | 对已显式 `bottom|right,40/40` 的 `demo-2` 执行后，配置里**一个 placement 键都不剩**（因为 `[render]` 本身就是底部 16px），rect → (784,911) |
| F30 列表缩略图 | ✅ | 三行缩略图分别是睡眠帧 BongoCat / 蓝色 demo / 黄色 demo，与桌面同帧 |
| F30 参数页动画预览 | ✅ | 右上角 320×320 预览显示所选宠物，与桌面同帧；点击可强制刷新 |
| F30 画布上的宠物图 | ✅ | 画布框内画的是宠物自身渲染（BongoCat 可辨认、demo 颜色正确） |
| F31 保存即热重载 | ✅ | 保存后面板状态栏「已保存并通知宿主重新加载…（备份…）」，宿主 `Reload` 后 rect 与文件一致 |
| F33 宿主离线可编辑 | ✅ | 先开面板后开宿主：顶栏「宠物未运行」、画布用「未知显示器（示意 1920×1080）」+ 黄色提示；启动宿主后 **6s 内**自动变「已连接（3 只宠物）」并把画布换成真实显示器 |
| F34 关面板不影响宠物 | ✅ | 每次驱动脚本结束都会 kill 面板，宿主继续动画；最终 `grim` 截图三只宠物仍在 |
| N02 宿主空闲 CPU | ✅ | 3 宠物空闲 10s = 0 ticks |

**未覆盖**：

- 真实「拔掉一块显示器」：本机只有一块屏，用「改变缩放/逻辑尺寸」代理（几何变化走同一条 `ListOutputs` 通路）。
- 面板在真实 niri 桌面上的窗口行为（平铺/浮动规则、HiDPI 下的 `pixels_per_point`）。
- IME 中文输入（G5 事项）。
- 面板与宿主**同时**在真实 Wayland 会话里长跑的内存曲线（面板只跑了 Xvfb）。

---

## 7. 实机验收发现并修复的问题

1. **拖拽起点偏移（已修，`ui/layout.rs`）**：原来在 `response.drag_started()` 那一帧用**当前指针位置**
   算抓取偏移，而 egui 要等指针移动超过拖拽阈值才报 `drag_started`，于是宠物会按「按下到触发之间的位移」
   整体平移（合成指针一次跳 43px → 宠物直接偏 ~43px；真实鼠标约 6px，属于看得见的轻微跳动）。
   改成用 `i.pointer.press_origin()`（按下点）计算抓取偏移后，实测落点与预期完全一致。
2. **画布背景抢占按下事件（防御性修改）**：画布原先用 `Sense::click_and_drag()` 分配，
   与叠在上面的宠物框抢同一次 press。虽然实机复测没有复现"点不动"，但画布本身不需要点击/拖拽语义，
   改成 `Sense::hover()` 后交互只归宠物框，语义更干净。
3. **画布交互目前只有手工/脚本验收（待补自动化）**：`egui_kittest` 能通过 `input_mut()` 注入指针事件，
   但画布矩形依赖窗口布局，做起来要先把「画布几何」暴露成可测函数；已列进 G4 顺带补，优先级低于功能。
4. **`ListOutputs.scale` 是整数（已知限制，未改）**：sctk 的 `OutputInfo.scale_factor` 是 `i32`，
   scale 1.5 会被报成 2，于是画布下拉显示 `@2x`。画布几何按**逻辑像素**计算，不受影响；
   要显示真实小数缩放需要另找协议来源，留到 G4 诊断页一起处理。

## 8. 后续（G4）建议顺序

G3 已验收完成（§6），下一步是 **G4**：

1. 诊断页（F40）：把 `doctor` 从 `main.rs` 抽成可复用模块，面板里内嵌显示（顺便解决 §7 第 3 条的整数 scale 展示）。
2. 托盘「设置…」（F41）：`HostCommand` 增加 spawn `petweave-settings`。
3. `wgpu` 回退（N04）：`eframe` 打开 wgpu feature + `--renderer` 参数；软件 GL 下预览 10fps 的 CPU 开销（§4）
   正是这条的动机之一。
4. 打包：PKGBUILD / Nix flake 增加 `petweave-settings` 产物；README 安装章节。
5. 每次都重跑 §1 的自动化回归（186 项）与本文 §4 的数值表。
