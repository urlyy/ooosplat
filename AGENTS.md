# AGENTS.md

本文件适用于整个仓库。后续 Agent 在读取或修改任何文件前，必须遵守这里的项目
边界、验证要求和文档同步规则。

## 项目目标

本仓库只交付 Ubuntu 22.04/24.04 LTS x86_64 的 OOOSplat headless CLI。产品能力是把
本地 MP4/MOV 视频或 JPG/PNG 图片序列转换为 3D Gaussian Splatting `final.ply`，并支持
后台单任务、实时状态、安全暂停和断点恢复。

流水线固定为：

```text
输入 -> FFprobe/图片分析 -> 帧规划 -> FFmpeg/图片准备 -> COLMAP CUDA 优先 / CPU 回退
     -> 重建校验 -> Brush Vulkan GPU -> final.ply
```

不在范围内：React、Vite、npm、Tauri、WebKit、桌面预览/编辑器、Windows、
macOS、移动端、Web 服务、遥测上报和数据库服务。除非用户明确改变产品范围，
不要重新引入这些内容或相关依赖。

## 先读哪些文件

开始工作前按任务读取：

1. 用户使用、安装、命令或排障：`README.md`。
2. 服务器运行和断点语义：`HEADLESS_LINUX.md`。
3. 构建/发布：`scripts/build-headless-linux.sh`、
   `scripts/setup-engines-linux.sh`、`scripts/setup-colmap-cuda-linux.sh`、
   `scripts/verify-engines-linux.sh`、
   `.github/workflows/ubuntu.yml`。
4. CLI/后台任务：`src/bin/splatstudio.rs`、
   `src/bin/splatstudio/headless.rs`。
5. Agent 操作流程：`skills/ooosplat-headless/SKILL.md`。

## 平台与依赖边界

- 支持平台只有 Ubuntu 22.04/24.04 x86_64。
- Rust 使用 stable；构建必须使用已提交的 `Cargo.lock` 和 `--locked`。
- FFmpeg、FFprobe 来自 Ubuntu 系统包；COLMAP 可来自系统包或固定版本 CUDA 源码构建。
- COLMAP 默认 auto：CUDA 特征提取/匹配探测通过后用 GPU，否则回退 CPU；gpu 模式禁止回退。
- Brush 使用 Vulkan GPU，设备和显存策略与 COLMAP CUDA 独立；不要拿 CUDA 设备显存推断 Brush 显存。
- Brush 固定为 `engines/manifest.linux.json` 中的版本、URL 和 SHA-256。
- 运行时不得下载引擎；下载只允许发生在显式 setup/build 阶段。
- 不要把下载的 Brush、`.cache/`、`target/` 或 `dist-headless/` 提交进 Git。

修改 Brush 版本时，必须同步更新：

- `engines/manifest.linux.json`
- `scripts/setup-engines-linux.sh`
- `scripts/verify-engines-linux.sh`
- `licenses/THIRD_PARTY_NOTICES.txt`
- README 中的版本说明

## 运行时契约

- 默认只允许一个活动生成任务。不要删除单任务锁或绕过 `ActiveGuard`。
- `generate`、`resume` 和 `switch` 是高资源操作；先展示或记录解析后的完整命令。
  用户已经明确要求执行时直接继续；只有执行意图不明确时才询问。
- `pause` 必须走 SIGTERM/取消令牌和子进程组清理，不要改成直接 `kill -9`。
- `switch` 必须先安全暂停当前任务；60 秒超时后保留原状，不强杀。
- 项目元数据、状态、索引和活动状态必须原子写入。
- 恢复任务前必须验证实际产物，不能只相信 JSON 中的布尔字段。
- `final.ply` 只能在完整校验后原子发布。
- `health` 任一必需引擎失败时必须返回非零退出码。
- 后台 worker 的公开状态为 `active.json`；锁为 `active.lock`；日志为
  `worker-*.log`。
- 不要提供自动删除项目的命令，也不要在测试或修复中删除用户项目目录。

## 数据兼容

- 保留现有 `project.json`、`state.json`、`project-index.json` 的 serde 兼容行为。
- 新字段使用合理的 `serde(default)`，旧项目缺字段时必须可读。
- 改变阶段、状态枚举、质量参数或目录结构前，检查 resume、tasks、status 和旧
  checkpoint 的行为。
- `ProjectStatus::Cancelled` 在 CLI 中展示为 `paused`；不要无意改变该外部语义。
- Brush v0.3.0 没有本项目可用的迭代内 checkpoint。不要在文档或代码中声称
  Brush 训练可以从具体迭代继续。

## 代码组织

```text
src/bin/splatstudio.rs          参数解析和用户输出
src/bin/splatstudio/headless.rs worker、锁、信号和活动状态
src/pipeline/                   阶段、事件、断点、执行器和估时
src/project/                    项目创建、元数据、索引和文件布局
src/engines/                    外部 CLI 发现、校验和调用
src/video/                      视频/图片输入分析与准备
src/reconstruction/             COLMAP/PLY 校验
src/presets/                    fast/balanced/high 参数
```

共享流水线逻辑放在库模块中；CLI 只负责参数、调度和显示。不要把新的业务流水线
堆到 `splatstudio.rs`。

## 实现规则

- Rust 代码必须通过 rustfmt 和 clippy，不使用 `unsafe`，除非是 Linux 信号/进程
  控制所必需且有清晰安全说明。
- 所有外部进程参数使用 `Command` 的参数数组，不通过 Shell 拼接用户输入。
- 用户路径必须作为 `Path`/`PathBuf` 传递；不要假设路径没有空格或非 ASCII 字符。
- 长任务需要支持取消，并把 stdout/stderr 写入有界或持久化日志。
- 错误信息要说明失败的引擎、路径或阶段，但不要吞掉底层原因。
- 不要以通过单元测试为由放宽引擎哈希、项目归属或 PLY 完整性校验。
- Shell 脚本使用 `#!/usr/bin/env bash` 和 `set -euo pipefail`。
- 临时目录使用 `mktemp -d` 并设置清理 trap。
- 构建发行目录前先清理旧 `dist-headless/`，避免混入陈旧文件。

## 文档同步规则

README 是用户和运维的主入口，AGENTS.md 是 Agent/贡献者的约束入口。修改以下
内容时必须同步：

| 变更 | 必须同步检查的文件 |
| --- | --- |
| CLI 参数、命令或输出 | `README.md`、`HEADLESS_LINUX.md`、skill |
| 环境变量或默认路径 | `README.md`、`HEADLESS_LINUX.md`、skill |
| 任务状态/断点语义 | README、运行手册、skill、相关测试 |
| Ubuntu 系统依赖 | 构建脚本、CI、README |
| 引擎版本/哈希 | manifest、setup/verify、许可证、README |
| 仓库范围或架构 | `README.md`、`AGENTS.md`、skill description |

命令示例必须可以直接复制，路径占位符要明确。不要写已不存在的 npm、Tauri、
`src-tauri/`、Windows 或 macOS 调用方式。

## 验证矩阵

### Rust 代码

```bash
cargo fmt -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --bin splatstudio
python3 tests/colmap_backend_smoke.py target/debug/splatstudio
```

涉及 Linux 条件编译时，再执行：

```bash
rustup target add x86_64-unknown-linux-gnu
cargo check --locked --target x86_64-unknown-linux-gnu --bin splatstudio
```

### Shell/构建

```bash
bash -n scripts/*.sh
./scripts/build-headless-linux.sh --help
```

只有 Ubuntu 22.04/24.04 x86_64 可以执行完整构建：

```bash
./scripts/build-headless-linux.sh
OOOSPLAT_ENGINE_DIR="$PWD/dist-headless/engines" \
  ./dist-headless/splatstudio health
```

### Skill

使用 skill-creator 的校验器检查 `skills/ooosplat-headless/`。如果 skill 的命令、
安全边界或默认值变化，同时更新 `agents/openai.yaml`。

### 文档和最终检查

```bash
rg -n 'npm|Tauri|src-tauri|Windows|macOS' \
  README.md HEADLESS_LINUX.md AGENTS.md scripts Cargo.toml
git diff --check
```

允许文档在“不支持范围”中提到这些关键词；不允许出现可执行的旧平台步骤。

## 交付清单

完成任务前确认：

- 只保留 Ubuntu headless 产品范围。
- 没有覆盖用户已有的无关修改。
- 新命令、默认值和文件路径已有测试或可重复验证。
- README、AGENTS.md、运行手册和 skill 与代码一致。
- CI 与本地构建脚本安装相同的关键系统依赖。
- `health`、测试、clippy、Shell 语法和 skill 校验结果已记录。
- `target/`、下载缓存、临时工具链和审查中间文件没有进入仓库。
