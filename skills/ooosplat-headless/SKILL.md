---
name: ooosplat-headless
description: Build, verify, deploy, and operate the OOOSplat Ubuntu 24.04 x86_64 headless CLI. Use for FFmpeg, FFprobe, GPU-first COLMAP, Brush Vulkan setup; MP4/MOV or JPG/PNG reconstruction; probe, plan, extract, generate, tasks, status, pause, resume, switch, checkpoint diagnosis, or locating final.ply.
---

# OOOSplat Headless

Operate the repository's `splatstudio` CLI on Ubuntu 24.04 x86_64. This skill
covers only the headless server product.

## Read the repository contract

Before changing or running the project, read:

1. `AGENTS.md` for product boundaries and required validation.
2. `README.md` for supported inputs, commands, and environment variables.
3. `HEADLESS_LINUX.md` for deployment and checkpoint behavior.

Find the repository root containing all of these paths:

```text
AGENTS.md
Cargo.toml
scripts/build-headless-linux.sh
src/bin/splatstudio.rs
engines/manifest.linux.json
```

Run commands from that root. Do not introduce desktop, web, Windows, macOS,
telemetry, or database components.

## Preflight the host

Check the platform first:

```bash
uname -s
uname -m
```

Continue with builds or generation only for `Linux` and `x86_64`. For a read-only
preflight, check these commands without changing the host:

```bash
command -v cargo
command -v curl
command -v ffmpeg
command -v ffprobe
command -v colmap
command -v sha256sum
command -v tar
```

Brush needs a working Vulkan GPU and vendor driver. The presence of
`libvulkan1` or Mesa alone does not prove production GPU training works.

## Build and verify

Use the first command when dependencies already exist. Use the second when the
user requested dependency installation or the current session otherwise
already authorizes it:

```bash
./scripts/build-headless-linux.sh
./scripts/build-headless-linux.sh --install-system-deps
```

The script downloads Brush v0.3.0 during setup, verifies its pinned hashes,
builds with `cargo build --locked --release`, and recreates `dist-headless/`.
Runtime commands must not download engines.

After every build, run:

```bash
export OOOSPLAT_ENGINE_DIR="$PWD/dist-headless/engines"
./dist-headless/splatstudio health
```

Retain the health JSON. Treat any nonzero exit status or any `canStart: false`
as failure. `health` does not perform a real Vulkan training job, so report that
limit explicitly.

## Configure CUDA on an NVIDIA server

Read the GPU first-run instructions in README.md. Install the vendor driver first.
Use `scripts/setup-colmap-cuda-linux.sh --install-system-deps` to build the pinned
headless CUDA COLMAP, then export
`OOOSPLAT_COLMAP="$PWD/.cache/colmap-cuda/install/bin/colmap"` before build/health/run.
Keep that installation; it is not bundled into dist-headless.

`OOOSPLAT_COLMAP_BACKEND` defaults to `auto`: probe CUDA feature extraction and
matching on two temporary images; use GPU on success, CPU on failure. Use `gpu`
for strict deployment checks (failure is nonzero), or `cpu` to skip the probe.
The probe can take 30 seconds plus process cleanup and briefly uses the GPU.
Confirm `acceleration.backend` is `gpu`, not just `cpuOnly: false`.
CUDA-visible device 0 is selected; `CUDA_VISIBLE_DEVICES` can restrict it.
Mapper and FFmpeg remain CPU; Brush selects Vulkan independently. Do not infer
Brush VRAM from the CUDA device. Runtime GPU failures are not retried on CPU.

## Validate an input

Full generation accepts:

- an MP4 or MOV file;
- a directory of same-size JPG/JPEG/PNG images.

Use these read-only or preprocessing commands when useful:

```bash
./dist-headless/splatstudio probe <INPUT>
./dist-headless/splatstudio plan <INPUT> --quality <fast|balanced|high>
./dist-headless/splatstudio extract <INPUT> <OUTPUT_DIR> --quality <QUALITY>
```

`probe`, `plan`, and `extract` can inspect other FFprobe/FFmpeg-readable video
formats, but `generate` still rejects video extensions other than MP4/MOV.

## Start a generation job

Resolve and record the input, projects root, and quality. Use `balanced` only
when the user did not specify a quality. Prefer background mode on servers:

```bash
./dist-headless/splatstudio generate <INPUT> \
  --projects-root <PROJECTS_ROOT> \
  --quality <fast|balanced|high> \
  --background
```

The command waits up to 10 seconds for the worker to publish its state. Capture
the returned PID, task ID when present, worker log, and status path. If the user
explicitly requested generation, that request authorizes the launch; ask only
when execution intent is unclear.

Monitor with:

```bash
./dist-headless/splatstudio status --watch
```

Use `--json` for automation. `status --watch` emits NDJSON in JSON mode and exits
after publishing the final persisted task state.

## List and inspect tasks

```bash
./dist-headless/splatstudio tasks
./dist-headless/splatstudio status <TASK_ID>
./dist-headless/splatstudio --json tasks
./dist-headless/splatstudio --json status <TASK_ID>
```

A task ID can be a full UUID or an unambiguous prefix from `tasks`.

## Pause, resume, or switch

Use graceful control commands. Do not kill COLMAP, Brush, or the worker with
`kill -9`:

```bash
./dist-headless/splatstudio pause
./dist-headless/splatstudio resume <TASK_ID> --background
./dist-headless/splatstudio switch <INPUT_OR_TASK_ID> \
  --projects-root <PROJECTS_ROOT> \
  --quality <QUALITY>
```

`pause` and `switch` wait up to 60 seconds for a safe stop and do not force kill
on timeout. `switch` launches the target in background mode. A resume during
Brush training restarts the Brush stage while retaining validated frame and
COLMAP checkpoints.

## Diagnose failures

1. Run `health`; retain its JSON and exit status.
2. Run `tasks`, then `status <TASK_ID>`.
3. Inspect `<project>/project.json`, `<project>/state.json`, and `<project>/logs/`.
4. Inspect the runtime `worker-*.log` next to `active.json`.
5. Verify checkpoint artifacts exist before recommending `resume`.
6. For Vulkan failures, check the vendor driver, `/dev/dri`, and container GPU
   exposure. Do not infer GPU readiness from Brush `--help` alone.

Do not manually edit checkpoint JSON or lock files. Do not delete project
directories. Report `<project>/final.ply` as successful only when status is
`completed` and the file exists.

## Know the runtime paths

The default data root is:

```text
${XDG_DATA_HOME:-$HOME/.local/share}/SplatStudio
```

Useful overrides are `OOOSPLAT_ENGINE_DIR`, `OOOSPLAT_FFMPEG`,
`OOOSPLAT_FFPROBE`, `OOOSPLAT_COLMAP`, `OOOSPLAT_BRUSH`, `OOOSPLAT_DATA_DIR`,
`OOOSPLAT_RUNTIME_DIR`, `OOOSPLAT_COLMAP_BACKEND`, `CUDA_VISIBLE_DEVICES`, and `RUST_LOG`. If `OOOSPLAT_RUNTIME_DIR` is set, it is
the runtime directory itself; do not append `headless/`.

## Validate repository changes

Follow the matrix in `AGENTS.md`. At minimum run:

```bash
cargo fmt -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
bash -n scripts/*.sh
./scripts/build-headless-linux.sh --help
```

For Linux-target changes also run:

```bash
rustup target add x86_64-unknown-linux-gnu
cargo check --locked --target x86_64-unknown-linux-gnu --bin splatstudio
```
