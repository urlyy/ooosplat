# Linux engines

OOOSplat headless uses system `ffmpeg`, `ffprobe`, and `colmap` (CUDA preferred) packages.
The bundled managed binary is Brush v0.3.0 for Linux x86_64.
For NVIDIA servers, build CUDA COLMAP with `scripts/setup-colmap-cuda-linux.sh`
and set `OOOSPLAT_COLMAP` to its printed path. This local installation is not
copied into dist-headless. See the GPU first-run instructions in the root README.

Install and verify it with:

```bash
./scripts/setup-engines-linux.sh
./scripts/verify-engines-linux.sh
```

The pinned source URL and SHA-256 values are recorded in
[`manifest.linux.json`](manifest.linux.json). Downloaded archives live under
`.cache/engines/`, and the extracted Brush binary lives under
`engines/linux/brush/`; both are ignored by Git.
