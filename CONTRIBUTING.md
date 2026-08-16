# 加入 PetWeave 生态

PetWeave 的目标是让"桌宠"成为一个生态：**角色包创作者**提供内容，**框架开发者**打磨运行时，**用户**享受桌面伙伴。任何人都可以从任意一侧加入。

## 一、贡献角色包（宠物）

### 你要做的

1. 按 [docs/PACKAGES.md](docs/PACKAGES.md) 从零创建一个角色包（`pet.toml` + 素材 + 可选 `main.lua`）
2. 本地验证：`petweave --preview` 出图、`petweave package` 打包、重装可用
3. 发布：`.petweave` 文件上传到 GitHub Releases（或 Gitee Releases / 网盘）

### 提交到 Awesome 列表

把包信息提交到本仓库的社区列表（`docs/ECOSYSTEM.md`，建设中）：

- 提交内容：包名、作者、一句话介绍、下载链接、封面图、许可证
- 要求：`petweave list` 可正常安装运行；许可证明确（无版权风险的素材）
- 提交方式：Pull Request 修改列表文件，或提 Issue 附上下载链接

### 素材许可注意

- 你的宠物素材必须是你拥有或已获许可的（引用他人素材请保留署名与许可证）
- 推荐宽松许可：CC0 / CC-BY / MIT，方便生态复用
- BongoCat 素材来自 [wayland-bongocat](https://github.com/saatvik333/wayland-bongocat)（MIT），原始猫画作 © StrayRogue & DitzyFlama

## 二、贡献框架代码

### 环境

```bash
cargo build --release          # 构建
cargo test                     # 全部测试（55 项）
```

> 本机离线环境：`CARGO_HOME=/home/hhs/Projects/petweave/.cargo-home cargo build --offline`

### 开发约定

- **提交信息**：Conventional Commits（`feat:` / `fix:` / `docs:` / `refactor:`，如 `feat(bongo): ...`）
- **每个里程碑的完成标准**：代码 + 测试 + 文档（README/ROADMAP 勾选）+ 实测
- **新增宠物能力**：优先以"角色包"形态落地（框架能力的证明），内置宠物只是示例
- **协议/平台层改动**：先看 `docs/TECH_STACK.md` 的架构约束（如 layer-shell 生命周期、线程模型）

### 目前的开放方向（见 [docs/ROADMAP.md](docs/ROADMAP.md)）

| 方向 | 内容 |
|---|---|
| M3 交互升级 | 指针事件、拖拽、轻物理、多宠物、托盘（ksni 依赖已就绪） |
| M4 深度集成 | wgpu GPU 后端、Live2D（[docs/LIVE2D.md](docs/LIVE2D.md)）、外部宠物进程协议 |
| 生态工具 | 角色包签名、社区商店、`petweave search`、封面图自动生成 |
| 兼容性 | 各合成器（Hyprland/Sway/KWin/niri）实测与兼容矩阵维护 |

## 三、反馈与交流

- **Bug / 建议**：GitHub Issues（描述：合成器 + 宠物类型 + 复现步骤 + 日志）
- **宠物需求**：想看到某个角色/玩法？提 Issue 标记 `pet-request`
- **PR 评审**：小步、聚焦的 PR 更容易被合入；带截图或录屏的可见变更优先

## 四、行为准则

- 尊重创作者：素材许可证必须保留署名
- 桌宠是娱乐软件：不做键盘记录、不收集数据（框架已默认不落盘键码，请保持）
- 尊重合成器生态：兼容性做不到 100% 时，文档诚实标注比 hack 更重要
