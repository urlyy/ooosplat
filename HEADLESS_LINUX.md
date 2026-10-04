# Ubuntu Headless 运行手册

本文面向部署和运维 `splatstudio` 的人员。用户安装、命令总览和开发入口见
[README.md](README.md)，Agent/贡献者约束见 [AGENTS.md](AGENTS.md)。

## 运行边界

支持环境固定为 **Ubuntu 24.04 LTS x86_64/amd64**。进程不创建窗口，不依赖
X11、Wayland、WebKit、Node.js 或本地 Web 服务。

完整流水线如下：

```text
MP4/MOV 或 JPG/PNG 图片序列
  -> FFprobe/图片头分析
  -> FFmpeg 抽帧或图片准备
  -> COLMAP CUDA 优先特征提取/匹配与稀疏重建
  -> 重建质量校验
  -> Brush v0.3.0 Vulkan GPU 训练
  -> final.ply
```

COLMAP 默认优先使用 CUDA，探测不可用时回退 CPU。Brush 必须能访问 Vulkan GPU。`libvulkan1` 和 Mesa 提供
loader/软件实现，生产机仍需安装与显卡匹配的 NVIDIA、AMD 或 Intel 驱动。

## 从仓库构建

先安装 Rust stable，并确认 `cargo` 在 `PATH` 中：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

克隆并安装系统依赖、固定版本 Brush，然后构建：

```bash
git clone https://github.com/urlyy/ooosplat.git
cd ooosplat
./scripts/build-headless-linux.sh --install-system-deps
```

已经安装依赖时使用：

```bash
./scripts/build-headless-linux.sh
```

脚本只接受 `Linux x86_64`，使用已提交的 `Cargo.lock`，校验 Brush 下载和二进制的
SHA-256，并在重新打包发行目录前清理旧 `dist-headless/`。

发行目录结构：

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

## GPU 云服务器首次配置

默认 `OOOSPLAT_COLMAP_BACKEND=auto`：优先使用 CUDA 做 COLMAP 特征提取和匹配，
启动探测失败时回退 CPU，并在 `health` 的 `acceleration.reason` 中说明原因。
Mapper 相机重建仍主要使用 CPU，FFmpeg 抽帧也不启用硬件解码。Brush 使用独立的
Vulkan 设备选择，不受这个 COLMAP 开关控制。

推荐使用 Ubuntu 24.04 x86_64 的 NVIDIA GPU 镜像，先按云厂商说明安装或启用
NVIDIA 驱动，确认 `nvidia-smi` 能看到显卡。CUDA 和 Vulkan 都必须可用。
普通 `apt install colmap` 不保证包含 CUDA；NVIDIA 服务器首次运行按以下步骤：

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
关闭 GUI 和 OpenGL，默认用 2 个编译任务、为当前显卡编译。首次构建需要下载 CUDA
工具包和 C++ 依赖，可能花较长时间。`OOOSPLAT_BUILD_JOBS` 可调整编译并行数；
`CUDA_ARCHITECTURES` 默认 `native`，跨显卡部署时应明确指定目标架构。
较新显卡若不被 Ubuntu CUDA 工具包支持，需要先按 NVIDIA 文档安装匹配的工具包，
准备 C++ 依赖后不带 `--install-system-deps` 运行脚本。

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

## 部署到服务器

把整个 `dist-headless/` 复制到目标机，例如 `/opt/ooosplat/`。目标机安装运行依赖：

```bash
sudo apt-get update
sudo apt-get install --no-install-recommends -y \
  ca-certificates colmap ffmpeg libvulkan1 mesa-vulkan-drivers
```

按显卡厂商文档安装 GPU 驱动。然后设置引擎目录：

```bash
export OOOSPLAT_ENGINE_DIR=/opt/ooosplat/engines
/opt/ooosplat/splatstudio health
```

`health` 始终打印四个引擎的 JSON 状态。任一 `canStart` 为 `false` 时，命令在输出
JSON 后返回非零退出码。对于 Brush 只验证 `brush_app --help`，不会执行真实训练，因此生产
上线前还要跑一个小型 `fast` 任务验证 GPU、Vulkan 和显存。

## 输入要求

完整 `generate`/`switch` 任务支持：

- 本地 MP4 或 MOV 文件；
- 包含 JPG/JPEG/PNG 的目录；
- 图片序列必须尺寸一致，文件按自然顺序处理；
- 带 Alpha 的 PNG 会生成与 COLMAP 图像对应的 Mask。

`probe`、`plan` 和 `extract` 可以读取 FFprobe/FFmpeg 支持的其他视频格式，但完整
生成任务仍会拒绝 MP4/MOV 之外的文件扩展名。

## 只读检查与预处理

```bash
/opt/ooosplat/splatstudio probe /data/object.mp4
/opt/ooosplat/splatstudio plan /data/object.mp4 --quality balanced
/opt/ooosplat/splatstudio extract /data/object.mp4 /data/prepared/frames \
  --quality balanced
```

图片序列也直接把目录作为 `<INPUT>`。视频 `extract` 的 Mask 目录位于帧输出目录的
同级 `masks/`。

## 启动生成任务

后台模式适合服务器：

```bash
/opt/ooosplat/splatstudio generate /data/object.mp4 \
  --projects-root /data/ooosplat-projects \
  --quality balanced \
  --background
```

启动命令最多等待 10 秒让 worker 写入活动状态。成功输出包含 worker PID、任务 ID
（初始化足够快时）、worker 日志和 `active.json` 路径。启动失败或超时会停止刚创建
的 worker，不留下无主后台任务。

前台模式适合调试：

```bash
/opt/ooosplat/splatstudio generate /data/object.mp4 \
  --projects-root /data/ooosplat-projects \
  --quality fast
```

按 `Ctrl+C` 或向 worker 发送 `SIGTERM` 会请求取消流水线、结束外部进程组，并保存
最近有效阶段。不要直接 `kill -9` COLMAP、Brush 或 worker。

质量值为 `fast`、`balanced` 或 `high`。同一运行目录只允许一个活动生成任务，避免
多个 Brush 进程争抢 GPU。

## 状态与自动化输出

```bash
/opt/ooosplat/splatstudio tasks
/opt/ooosplat/splatstudio status --watch
/opt/ooosplat/splatstudio status 12ab34cd
```

任务 ID 可以是完整 UUID，也可以是 `tasks` 中唯一的 UUID 前缀。JSON 模式：

```bash
/opt/ooosplat/splatstudio --json tasks
/opt/ooosplat/splatstudio --json status 12ab34cd
/opt/ooosplat/splatstudio --json status --watch
```

`status --watch` 在 JSON 模式下输出 NDJSON。worker 的普通 stdout/stderr 写入独立
日志，因此不会混入状态流。活动任务结束后，watch 输出最终持久化状态并退出。

## 暂停、恢复和切换

安全暂停当前任务：

```bash
/opt/ooosplat/splatstudio pause
```

命令发送 `SIGTERM` 并最多等待 60 秒。超时会报错，不会强杀进程。

恢复已暂停、失败或异常中断的任务：

```bash
/opt/ooosplat/splatstudio resume 12ab34cd --background
```

切换任务会先安全暂停当前 worker，再以后台模式启动目标：

```bash
/opt/ooosplat/splatstudio switch /data/another-object.mov \
  --projects-root /data/ooosplat-projects \
  --quality balanced

/opt/ooosplat/splatstudio switch 12ab34cd
```

`switch` 的目标如果是存在的路径，会创建新任务；否则按 UUID/唯一前缀解析旧任务。

## 数据目录和环境变量

| 变量 | 作用 | 默认值或发现顺序 |
| --- | --- | --- |
| `OOOSPLAT_ENGINE_DIR` | 引擎根目录 | 可执行文件相邻 `engines/`、当前目录、PATH |
| `OOOSPLAT_FFMPEG` | 指定 FFmpeg | 引擎目录后回退 PATH |
| `OOOSPLAT_FFPROBE` | 指定 FFprobe | 引擎目录后回退 PATH |
| `OOOSPLAT_COLMAP_BACKEND` | auto/gpu/cpu | auto，CUDA 优先 |
| `CUDA_VISIBLE_DEVICES` | COLMAP 可见 CUDA 设备及顺序 | CUDA 默认 |
| `OOOSPLAT_COLMAP` | 指定 COLMAP | 引擎目录后回退 PATH |
| `OOOSPLAT_BRUSH` | 指定 Brush | 引擎目录后回退 PATH |
| `OOOSPLAT_DATA_DIR` | 项目、索引和设置根目录 | `${XDG_DATA_HOME:-$HOME/.local/share}/SplatStudio` |
| `OOOSPLAT_RUNTIME_DIR` | 活动状态、锁和 worker 日志目录 | `<DATA_DIR>/headless` |
| `RUST_LOG` | CLI/Brush 日志过滤 | 未设置时使用默认值 |

如果设置 `OOOSPLAT_RUNTIME_DIR`，该路径就是运行目录，不再追加 `headless/`。

默认数据布局：

```text
${XDG_DATA_HOME:-$HOME/.local/share}/SplatStudio/
  project-index.json
  settings.json
  Projects/
  headless/
    active.json
    active.lock
    worker-*.log
```

单个项目：

```text
<projects-root>/<timestamp>_<input-name>/
  project.json
  state.json
  final.ply
  source/
  work/
    frames/
    masks/
    colmap/
    brush/
  logs/
```

## 断点语义

- `state.json` 记录帧、特征、匹配、稀疏重建和 Brush 完成状态。
- 恢复前会验证实际文件；缺失、空文件或损坏产物会使对应阶段重新运行。
- `final.ply` 先写临时文件，校验通过后原子发布。
- Brush v0.3.0 没有本项目可用的迭代内 checkpoint；训练中暂停后会从 Brush 阶段
  开头重跑，但会复用已验证的帧和 COLMAP 结果。
- 不要手工编辑 `project.json`、`state.json`、`active.json` 或 `active.lock`。

## systemd 调用示例

建议让 systemd 只管理一次性启动命令，任务状态仍由 `splatstudio` 管理：

```ini
[Unit]
Description=OOOSplat generation job
After=local-fs.target

[Service]
Type=oneshot
User=ooosplat
Environment=OOOSPLAT_ENGINE_DIR=/opt/ooosplat/engines
Environment=OOOSPLAT_DATA_DIR=/var/lib/ooosplat
ExecStart=/opt/ooosplat/splatstudio generate /data/object.mp4 --projects-root /var/lib/ooosplat/projects --quality balanced --background
RemainAfterExit=no
```

查看任务进度仍使用：

```bash
sudo -u ooosplat /opt/ooosplat/splatstudio status --watch
```

## 故障处理

保留健康检查 JSON 和错误输出：

```bash
/opt/ooosplat/splatstudio health 2>health.err | tee health.json
```

重点检查 `path`、`exists`、`canStart` 和 `detail`。Brush/Vulkan 问题可先检查：

```bash
ldd /opt/ooosplat/engines/linux/brush/brush_app
ls -l /dev/dri
```

NVIDIA 环境还可以用 `nvidia-smi` 确认容器/主机是否暴露 GPU，但 OOOSplat 不依赖
该命令来选择 COLMAP 后端；实际 CUDA 探测决定是否启用。

异常重启后：

```bash
/opt/ooosplat/splatstudio tasks
/opt/ooosplat/splatstudio status <TASK_ID>
/opt/ooosplat/splatstudio resume <TASK_ID> --background
```

只有状态为 `completed` 且 `final.ply` 实际存在时才能把任务视为成功。项目不会自动
删除；确认不再恢复后，由运维人员备份结果并删除整个项目目录。
