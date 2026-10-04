# OOOSplat Headless

OOOSplat Headless 是面向 **Ubuntu 22.04/24.04 LTS x86_64** 服务器的视频/图片序列转
3D Gaussian Splatting 命令行工具。仓库只保留本地 headless 生成链路，不包含
React、Tauri、WebKit、桌面预览器，也不提供 Windows 或 macOS 安装包。

```text
视频或图片序列
  -> FFprobe/图片头分析
  -> 抽帧与质量规划
  -> FFmpeg/图片序列准备
  -> COLMAP CUDA 优先特征提取/匹配与稀疏重建
  -> 重建质量校验
  -> Brush Vulkan GPU 训练
  -> final.ply
```

## 支持范围

| 项目 | 支持情况 |
| --- | --- |
| 操作系统 | Ubuntu 22.04/24.04 LTS |
| 架构 | x86_64/amd64 |
| 运行方式 | 前台 CLI、后台单任务 worker |
| 生成输入 | MP4/MOV 视频；同尺寸 JPG/PNG 图片序列目录 |
| 输出 | 3D Gaussian Splatting `final.ply` |
| COLMAP | CUDA 优先，探测失败自动回退 CPU；NVIDIA 服务器需安装 CUDA 构建 |
| Brush | 固定为 v0.3.0 Linux x86_64，使用 Vulkan GPU |
| 桌面界面 | 不支持 |
| Windows/macOS | 不支持构建和发布 |

Brush 训练必须能访问可用的 Vulkan GPU。安装 `libvulkan1` 和 Mesa 只能提供
Vulkan loader/软件实现；生产机器仍需安装与实际显卡匹配的 NVIDIA、AMD 或
Intel 驱动。COLMAP 回退 CPU 不代表 Brush 可以无 GPU 完成训练。

## 从零安装

### 1. 安装 Rust

全新系统先安装下载和版本管理工具：

```bash
sudo apt-get update
sudo apt-get install -y ca-certificates curl git
```

项目使用 Rust stable：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version
cargo --version
```

### 2. 克隆并构建

NVIDIA GPU 云服务器若要让 COLMAP 使用 GPU，请在克隆后按下方「GPU 云服务器首次配置」
安装 CUDA 版 COLMAP；只装系统 COLMAP 时可能回退 CPU。Brush 仍需要 Vulkan GPU。

下面的命令会通过 apt 安装 Ubuntu 系统依赖、下载并校验固定版本 Brush，随后
构建 release CLI：

```bash
git clone https://github.com/urlyy/ooosplat.git
cd ooosplat
./scripts/build-headless-linux.sh --install-system-deps
```

如果系统依赖已经安装：

```bash
./scripts/build-headless-linux.sh
```

构建脚本执行以下动作：

1. 拒绝非 Ubuntu 22.04/24.04 x86_64 环境；
2. 可选安装 FFmpeg、FFprobe、COLMAP、Vulkan loader、`vulkaninfo` 和构建依赖；
3. 下载 Brush v0.3.0，并校验压缩包及可执行文件 SHA-256；
4. 验证 FFmpeg、FFprobe、COLMAP 和 Brush CLI；
5. 使用 `cargo build --locked --release` 构建；
6. 清理旧的 `dist-headless/`，重新生成可部署目录。

### 3. 健康检查

```bash
export OOOSPLAT_ENGINE_DIR="$PWD/dist-headless/engines"
./dist-headless/splatstudio health
```

`health` 输出四个引擎的 JSON 状态。任一引擎不存在或无法启动时，命令返回非零
退出码，因此可以直接用于 CI、部署探针或 Shell 的 `set -e` 流程。

> `health` 验证 Brush CLI 可以启动，但不会提交真实 GPU 训练。首次生产使用前，
> 仍应运行一个小型 `fast` 任务验证 Vulkan 驱动和显存环境。

## GPU 云服务器首次配置

默认 `OOOSPLAT_COLMAP_BACKEND=auto`：优先使用 CUDA 做 COLMAP 特征提取和匹配，
启动探测失败时回退 CPU，并在 `health` 的 `acceleration.reason` 中说明原因。
Mapper 相机重建仍主要使用 CPU，FFmpeg 抽帧也不启用硬件解码。Brush 使用独立的
Vulkan 设备选择，不受这个 COLMAP 开关控制。

推荐使用 Ubuntu 22.04/24.04 x86_64 的 NVIDIA GPU 镜像，先按云厂商说明安装或启用
NVIDIA 驱动，确认 `nvidia-smi` 能看到显卡，再用 `vulkaninfo --summary` 确认
NVIDIA Vulkan 设备可见。COLMAP 使用 GPU 时还需要兼容显卡架构的 CUDA 工具包。
`nvidia-smi` 显示的 CUDA 版本是驱动能力，不代表 `nvcc` 已在 `PATH`。
如果已有 `/usr/local/cuda/bin/nvcc`，先执行
`export PATH="/usr/local/cuda/bin:$PATH"` 并运行 `nvcc --version`；无需重复安装工具包。
普通 `apt install colmap` 不保证包含 CUDA；需要 COLMAP GPU 的服务器按以下步骤：

```bash
# 在本仓库根目录，Rust stable 已安装；以下 apt 命令需要 sudo 或 root。
./scripts/setup-colmap-cuda-linux.sh --install-system-deps
export OOOSPLAT_COLMAP="$PWD/.cache/colmap-cuda/install/bin/colmap"
./scripts/build-headless-linux.sh --install-system-deps
export OOOSPLAT_ENGINE_DIR="$PWD/dist-headless/engines"

# 强制验证 GPU 可用：失败返回非零，不静默转 CPU。
OOOSPLAT_COLMAP_BACKEND=gpu ./dist-headless/splatstudio health

# 首次用短视频验证完整生成；这里也要求 COLMAP 使用 GPU。
OOOSPLAT_COLMAP_BACKEND=gpu ./dist-headless/splatstudio generate /data/object.mp4 \
  --projects-root /data/ooosplat-projects --quality fast --background
./dist-headless/splatstudio status --watch
```

CUDA 安装脚本固定 COLMAP 3.11.1（提交 `682ea9ac4020a143047758739259b3ff04dabe8d`），
关闭 GUI 和 OpenGL，默认用 2 个编译任务、为当前显卡编译。脚本的
`--install-system-deps` 安装 C++ 依赖；CUDA 工具包须按显卡架构单独安装。
首次构建可能花较长时间。`OOOSPLAT_BUILD_JOBS` 可调整编译并行数；
`CUDA_ARCHITECTURES` 默认取 `nvidia-smi` 检测到的第一张 GPU 计算能力
（如 RTX 5090 为 `120`）；跨显卡部署或无法检测时应明确指定目标架构。
较新显卡若不被 Ubuntu CUDA 工具包支持，需要先按 NVIDIA 文档安装匹配的工具包。
例如 RTX 5090 不应直接依赖 Ubuntu 22.04 仓库的旧版 `nvidia-cuda-toolkit`；
先安装支持该显卡架构的 NVIDIA CUDA 工具包并确认 `nvcc --version`，然后运行
`./scripts/setup-colmap-cuda-linux.sh --install-system-deps`。如暂不安装 CUDA，保留默认 `auto` 模式，
系统 COLMAP 可回退 CPU；这不影响 Brush 对 Vulkan GPU 的要求。
Ubuntu 22.04 上的 RTX 5090 可按 [NVIDIA 官方 CUDA 仓库](https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2204/x86_64/)
安装 CUDA 12.8 工具包（不安装驱动包）：

```bash
curl -fsSL https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2204/x86_64/cuda-keyring_1.1-1_all.deb -o /tmp/cuda-keyring_1.1-1_all.deb
sudo dpkg -i /tmp/cuda-keyring_1.1-1_all.deb
sudo apt-get update
sudo apt-get install --no-install-recommends -y cuda-toolkit-12-8
export PATH="/usr/local/cuda-12.8/bin:$PATH"
nvcc --version
```

`OOOSPLAT_COLMAP` 指向本机安装路径。该 CUDA 构建及其动态库不打包进
`dist-headless/`，在另一台服务器上需重新安装。不要在使用期间删除
`.cache/colmap-cuda/install`；新 Shell、systemd 服务都要设置同一个路径。

`health` 和任务预检会使用临时目录内的两张 128×128 图片做 CUDA 特征提取及匹配，
上限 30 秒，超时会清理子进程（可能另需约 3 秒）。它会短暂占用 GPU，完成后
删除临时文件；仍不验证真实 Brush 训练。`health` 输出 COLMAP 的
`acceleration.backend: "gpu"` 才表示 GPU 探测通过；仅 `cpuOnly: false` 不够。

模式与选卡：

- 不设置或 `OOOSPLAT_COLMAP_BACKEND=auto`：探测通过使用 GPU，否则使用 CPU。
- `OOOSPLAT_COLMAP_BACKEND=gpu`：GPU 不可用时健康检查/任务失败。
- `OOOSPLAT_COLMAP_BACKEND=cpu`：跳过 CUDA 探测，强制 COLMAP CPU。
- CUDA 使用可见设备 0；例如 `CUDA_VISIBLE_DEVICES=1` 选择物理设备 1。
  这不替代 Brush 的 Vulkan 选卡。AMD/Intel 服务器的 COLMAP 回退 CPU，Brush
  仍可使用相应的 Vulkan GPU。

自动回退仅发生在预检阶段；正式任务中途 GPU OOM、驱动错误会保留阶段断点并报错，
不会隐式重跑 CPU。必要时显式以 CPU 模式 `resume`。

## 构建产物与部署

构建结果位于 `dist-headless/`：

```text
dist-headless/
  splatstudio
  engines/
    manifest.linux.json
    linux/brush/brush_app
  licenses/
    Brush-LICENSE.txt
    COLMAP-LICENSE.txt
    FFmpeg-LGPL-2.1.txt
    MIT.txt
    MPL-2.0.txt
    THIRD_PARTY_NOTICES.txt
    Unicode-3.0.txt
    Zlib.txt
  GENERATED_OUTPUTS.md
  LICENSE
  NOTICE
  TRADEMARK_POLICY.md
```

可以把整个目录复制到另一台 Ubuntu 22.04/24.04 x86_64 机器，例如
`/opt/ooosplat/`。目标机仍需安装系统运行依赖：

```bash
sudo apt-get update
sudo apt-get install --no-install-recommends -y \
  ca-certificates colmap ffmpeg libvulkan1 mesa-vulkan-drivers vulkan-tools
```

生产机应按显卡厂商文档安装真实 GPU 驱动。运行时建议明确指定引擎目录：

```bash
export OOOSPLAT_ENGINE_DIR=/opt/ooosplat/engines
/opt/ooosplat/splatstudio health
```

## 第一个生成任务

服务器上建议使用后台模式：

```bash
./dist-headless/splatstudio generate /data/videos/object.mp4 \
  --projects-root /data/ooosplat-projects \
  --quality balanced \
  --background
```

命令成功后会输出 worker PID、任务 ID（若初始化已完成）、日志路径和状态文件路径。
随后监控进度：

```bash
./dist-headless/splatstudio status --watch
```

前台模式适合调试。按 `Ctrl+C` 会请求安全取消外部进程并保存最近的有效阶段断点：

```bash
./dist-headless/splatstudio generate /data/videos/object.mp4 \
  --projects-root /data/ooosplat-projects \
  --quality fast
```

## 输入要求

### 视频

- `generate` 和 `switch` 接受本地 MP4/MOV 文件。
- `probe`、`plan` 和 `extract` 可以分析 FFprobe/FFmpeg 能读取的其他本地视频，
  但这不代表该扩展名可以进入完整生成任务。
- 旋转信息、帧率、时长和 Alpha 通道由 FFprobe 分析。
- 抽帧计划受质量档位、视频时长和源帧率共同影响。

### 图片序列

- 输入参数传一个目录，而不是单张文件。
- 支持 JPG/JPEG 和 PNG。
- 图片会自然排序并校验尺寸；序列必须使用一致尺寸。
- 含 Alpha 的 PNG 会生成与 COLMAP 图像同名的 Mask。

示例：

```bash
./dist-headless/splatstudio generate /data/images/object-sequence \
  --projects-root /data/ooosplat-projects \
  --quality high \
  --background
```

## 质量档位

| 档位 | 视频目标/补救采样 | COLMAP 特征上限 | Brush 基础迭代 | 用途 |
| --- | ---: | ---: | ---: | --- |
| `fast` | 6 / 9 FPS | 4,096 | 8,000 | 快速验证、驱动冒烟测试 |
| `balanced` | 8 / 12 FPS | 8,192 | 15,000 | 默认生产档位 |
| `high` | 12 / 15 FPS | 16,384 | 30,000 | 高质量输出，耗时和显存需求最高 |

实际帧数会受到源帧率、最低帧数、分辨率规划和重建补救策略影响。Ubuntu
headless 版本默认让 COLMAP 优先使用 CUDA，Brush 由 Vulkan 独立选择设备。`high`
使用保守训练参数，并在 Brush OOM 时执行一次有界降级；这不等同于保证任意 GPU
都能完成任务。

## 完整命令

全局参数：

```text
--engine-dir <DIR>  覆盖引擎目录；等价环境变量为 OOOSPLAT_ENGINE_DIR
--json              tasks/status/pause 等命令输出 JSON；status --watch 输出 NDJSON
-h, --help          帮助
-V, --version       版本
```

| 命令 | 作用 | 是否修改状态 |
| --- | --- | --- |
| `health` | 检查引擎，auto/gpu 模式探测 CUDA | 临时目录与短时 GPU 探测 |
| `probe <INPUT>` | 输出视频或图片序列元数据 | 否 |
| `plan <INPUT> --quality ...` | 计算抽帧/图片计划 | 否 |
| `extract <INPUT> <OUTPUT>` | 只准备帧与可选 Mask | 写指定输出目录 |
| `generate <INPUT>` | 创建项目并运行完整流水线 | 是 |
| `tasks` | 列出已注册任务和短 ID | 否 |
| `status [TASK_ID]` | 查看活动任务或持久化任务 | 否 |
| `pause` | 向当前 worker 发送安全停止信号 | 是 |
| `resume <TASK_ID>` | 从最近有效断点继续 | 是 |
| `switch <INPUT_OR_TASK_ID>` | 安全暂停当前任务，再启动/恢复另一个任务 | 是 |

查看每个子命令的精确参数：

```bash
./dist-headless/splatstudio --help
./dist-headless/splatstudio generate --help
./dist-headless/splatstudio status --help
```

### 只分析或抽帧

```bash
./dist-headless/splatstudio probe /data/videos/object.mp4
./dist-headless/splatstudio plan /data/videos/object.mp4 --quality balanced
./dist-headless/splatstudio extract /data/videos/object.mp4 /data/prepared/frames \
  --quality balanced
```

视频 `extract` 的 Mask 目录位于输出帧目录的同级 `masks/`。

### 任务管理

```bash
./dist-headless/splatstudio tasks
./dist-headless/splatstudio status 12ab34cd
./dist-headless/splatstudio status --watch
./dist-headless/splatstudio pause
./dist-headless/splatstudio resume 12ab34cd --background
./dist-headless/splatstudio switch /data/videos/another.mp4 \
  --projects-root /data/ooosplat-projects \
  --quality balanced
./dist-headless/splatstudio switch 12ab34cd
```

任务 ID 可以使用完整 UUID，也可以使用 `tasks` 输出的唯一前缀。默认只允许一个
活动生成任务，避免多个 Brush 进程争抢同一张 GPU。`switch` 最多等待 60 秒让
当前任务安全退出；超时不会强杀任务。

### JSON/NDJSON 自动化

```bash
./dist-headless/splatstudio --json tasks
./dist-headless/splatstudio --json status 12ab34cd
./dist-headless/splatstudio --json status --watch
./dist-headless/splatstudio --json pause
```

`status --watch` 在活动任务结束后输出最终持久化状态并退出。不要把普通日志和
NDJSON 混在同一解析通道；后台 worker 的 stdout/stderr 写入独立日志文件。

## 数据、状态与环境变量

| 变量 | 作用 | 默认值/发现顺序 |
| --- | --- | --- |
| `OOOSPLAT_ENGINE_DIR` | 引擎根目录 | CLI 相邻 `engines/`、当前目录、系统 PATH |
| `OOOSPLAT_FFMPEG` | 指定 FFmpeg 可执行文件 | 引擎目录后回退 PATH |
| `OOOSPLAT_FFPROBE` | 指定 FFprobe 可执行文件 | 引擎目录后回退 PATH |
| `OOOSPLAT_COLMAP_BACKEND` | COLMAP 后端：auto/gpu/cpu | auto，CUDA 优先 |
| `CUDA_VISIBLE_DEVICES` | COLMAP 可见 CUDA 设备及顺序 | CUDA 默认可见设备 |
| `OOOSPLAT_COLMAP` | 指定 COLMAP 可执行文件 | 引擎目录后回退 PATH |
| `OOOSPLAT_BRUSH` | 指定 Brush 可执行文件 | 引擎目录后回退 PATH |
| `OOOSPLAT_DATA_DIR` | 默认项目、任务索引和设置根目录 | `${XDG_DATA_HOME:-$HOME/.local/share}/SplatStudio` |
| `OOOSPLAT_RUNTIME_DIR` | 活动状态、锁和 worker 日志目录 | `<DATA_DIR>/headless` |
| `RUST_LOG` | CLI/Brush 日志过滤级别 | 未设置时使用程序默认值 |

如果设置了 `OOOSPLAT_RUNTIME_DIR`，该值就是运行目录本身，不会再追加
`headless/`。命令行 `--engine-dir` 的优先级高于自动发现；各单引擎变量用于更细
粒度覆盖。

默认运行状态：

```text
${XDG_DATA_HOME:-$HOME/.local/share}/SplatStudio/
  project-index.json
  settings.json                 # 存在旧设置时兼容读取
  Projects/                     # 未传 --projects-root 时的默认项目目录
  headless/
    active.json                 # 原子更新的活动任务状态
    active.lock                 # 单任务锁
    worker-*.log                # 后台 worker 日志
```

单个项目目录：

```text
<projects-root>/<timestamp>_<input-name>/
  project.json
  state.json
  final.ply                     # 完成后原子发布
  source/
  work/
    frames/
    colmap/
    brush/
  logs/
```

## 暂停与断点恢复

- 抽帧、特征提取、匹配、稀疏重建和 Brush 完成状态会写入 `state.json`。
- 恢复前会验证实际文件，缺失或损坏的产物不会被误当成有效断点。
- `final.ply` 使用临时文件原子发布，半成品不会被标记为完成结果。
- Brush v0.3.0 在本项目中没有迭代内 checkpoint 接口。如果在 Brush 训练中暂停，
  恢复会重新开始 Brush 阶段，但会复用已验证的帧和 COLMAP 结果。
- 不要手工修改 `project.json`、`state.json`、`active.json` 或 `active.lock`。
- 不要直接 `kill -9` COLMAP/Brush。优先使用 `pause`，让 worker 结束子进程并保存
  最近的有效阶段。

## 常见故障

### `health` 返回非零

先保留完整 JSON 和 stderr：

```bash
./dist-headless/splatstudio health 2>health.err | tee health.json
```

检查 `path`、`exists`、`canStart` 和 `detail` 字段。引擎目录不正确时设置
`OOOSPLAT_ENGINE_DIR`，系统命令不在 PATH 时使用对应的单引擎变量。

### Brush/Vulkan 启动或训练失败

```bash
ldd ./dist-headless/engines/linux/brush/brush_app
ls -l /dev/dri
```

确认容器/虚拟机暴露 GPU 设备，并安装了与宿主显卡匹配的 Vulkan 驱动。Mesa 软件
实现只能证明 loader 存在，不能替代生产 GPU 驱动。

### 任务中断或服务器重启

```bash
./dist-headless/splatstudio tasks
./dist-headless/splatstudio status <TASK_ID>
./dist-headless/splatstudio resume <TASK_ID> --background
```

查看任务目录下的 `logs/` 和运行目录下的 `worker-*.log`。只有在状态为
`completed` 且 `final.ply` 实际存在时，才把任务视为成功。

### 磁盘空间

项目会同时保存源文件、抽取帧、COLMAP 数据库/模型、Brush 工作目录和最终 PLY。
运行前应为项目根目录预留明显高于输入文件大小的空间。项目没有自动删除命令；
确认无需恢复后，再由运维人员删除整个项目目录并备份必要结果。

## 开发与验证

修改 Rust 代码后必须运行：

```bash
cargo fmt -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --bin splatstudio
python3 tests/colmap_backend_smoke.py target/debug/splatstudio
```

修改 Shell、构建或发布逻辑后还要运行：

```bash
bash -n scripts/*.sh
./scripts/build-headless-linux.sh --help
```

Ubuntu 实机端到端验证：

```bash
./scripts/build-headless-linux.sh
export OOOSPLAT_ENGINE_DIR="$PWD/dist-headless/engines"
./dist-headless/splatstudio health
```

CI 位于 `.github/workflows/ubuntu.yml`，覆盖 Ubuntu 22.04/24.04 系统依赖、Brush 校验、
Rust 格式/测试/clippy、GPU 策略模拟集成、FFmpeg probe/extract 集成和发行目录构建。
普通 CI 没有 NVIDIA GPU，不执行 CUDA 源码构建或真实 CUDA/Vulkan 训练。

仓库修改约束、测试矩阵和文档同步规则见 [AGENTS.md](AGENTS.md)。

## Agent skill

仓库附带 [`ooosplat-headless`](skills/ooosplat-headless/SKILL.md) skill，可让
TraeCode/Codex agent 按固定流程构建、检查、生成、监控、暂停和恢复任务。

安装到用户级 skill 目录：

```bash
mkdir -p ~/.trae/skills
cp -R skills/ooosplat-headless ~/.trae/skills/
```

调用示例：

```text
使用 $ooosplat-headless 检查 Ubuntu 服务器环境并构建项目
使用 $ooosplat-headless 把 /data/object.mp4 生成为 balanced 模型并监控进度
```

## 仓库结构

```text
Cargo.toml                     Rust 项目入口
src/bin/splatstudio.rs         公共 CLI
src/bin/splatstudio/headless.rs 后台任务与运行状态管理
src/pipeline/                  生成流水线、进度、断点和耗时估计
src/project/                   项目目录、元数据和索引
src/engines/                   FFmpeg/FFprobe/COLMAP/Brush 适配
src/video/                     视频分析、抽帧和图片序列准备
scripts/                       Ubuntu 构建、引擎安装和校验
engines/manifest.linux.json    Brush 固定版本与 SHA-256
skills/ooosplat-headless/      Agent skill
licenses/                      直接依赖许可证与声明
```

## 许可证

项目使用 Apache-2.0，见 [LICENSE](LICENSE)。第三方组件见
[licenses/THIRD_PARTY_NOTICES.txt](licenses/THIRD_PARTY_NOTICES.txt)，生成文件的
许可说明见 [GENERATED_OUTPUTS.md](GENERATED_OUTPUTS.md)。

更聚焦的服务器运行手册见 [HEADLESS_LINUX.md](HEADLESS_LINUX.md)。
