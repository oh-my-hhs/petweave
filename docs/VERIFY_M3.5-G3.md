# M3.5 · G3 验收手册（布局画布 + 实时预览）

> **用途**：本轮（2026-10-22）在没有桌面会话的环境里实现了 G3 的代码与自动化测试，
> **桌面视觉验收留到下一轮**。本文是下一轮照着做就能验收/回归的清单：
> 每一节都写清"怎么跑、看什么、什么算通过、失败先看哪里"。
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

| 指标 | 怎么测 | G2 基线（参考） | G3 实测 |
|---|---|---|---|
| `petweave-settings` release 体积 | `ls -l target/release/petweave-settings` | 12.39 MiB | ⬜ |
| 宿主 release 体积 | `ls -l target/release/petweave` | 7.73 MiB | ⬜（本轮宿主零改动，预期不变） |
| GUI 常驻 RSS（空闲） | `ps -o rss= -p $(pgrep -f petweave-settings)` | ≈190 MB（软件 GL 回退） | ⬜ |
| GUI CPU（预览运行中 / 关于页） | `top -p …` 采样 10s | — | ⬜ |
| 拖拽感知延迟 | 目测 + `RUST_LOG=debug` 看 `SetPlacement` 频率 | 目标 < 50ms | ⬜ |
| 三宠物同屏宿主 RSS | `ps -o rss= -p $(pgrep -x petweave)` | ≈14.9 MB（单只，scale 2） | ⬜ |

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

## 6. 本轮（2026-10-22）实测

| 项目 | 结果 |
|---|---|
| `cargo build --offline`（全 workspace） | ✅ |
| `cargo test --offline` | ✅ 186 项全绿（103 host / 38 core / 45 settings） |
| `cargo clippy --offline --all-targets` | ✅ `petweave-settings` 零告警；其余为 §1 列出的既有告警 |
| 宿主 GUI 依赖（N01） | ✅ `cargo tree -p petweave` 无 egui/eframe/gtk/winit |
| 桌面（合成器）实测 | ⬜ **未做**（本轮无桌面会话）→ 下一轮按 §3 执行 |
| 手工视觉验收（跟手/吸附线/同帧） | ⬜ 同上 |
| GUI RSS / CPU / 拖拽延迟 | ⬜ 同上，填进 §4 |
| release 体积 | ✅ `petweave` **8.01 MiB**（8,399,768 B）、`petweave-settings` **15.03 MiB**（15,761,152 B）；面板比 G2 记录 +2.64 MiB 是 D7（GUI 侧跑宿主 runtime）的必然代价 |

---

## 7. 下一轮建议顺序

1. §1 自动化回归（5 分钟，先确认代码状态没被动过）。
2. §3.1 → §3.2 → §3.3（画布与拖拽是 G3 的核心，先跑通这三节）。
3. §3.4 → §3.5（表单/预设/预览）。
4. §3.6 → §3.7（热插拔与进程边界）。
5. 把 §4 的数值补进 `docs/GUI_ROADMAP.md` §14 与本文件 §6，然后把 GUI_ROADMAP 里 G3 的
   "（待桌面实测）"改成"✅ 完成"，并更新 §15 变更记录（v0.8）。
6. 接着做 **G4**：诊断页（F40，把 `doctor` 从 `main.rs` 抽成可复用模块）、托盘「设置…」（F41）、
   `wgpu` 回退（N04）、PKGBUILD/Nix 产物。
