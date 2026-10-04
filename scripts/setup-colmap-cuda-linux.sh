#!/usr/bin/env bash
set -euo pipefail

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "${1:-}" == --help || "${1:-}" == -h ]]; then
  cat <<'USAGE'
Usage: scripts/setup-colmap-cuda-linux.sh [--install-system-deps]
Build COLMAP 3.11.1 with CUDA and without GUI/OpenGL on Ubuntu 22.04/24.04 x86_64.
Requires an NVIDIA driver and a GPU-compatible CUDA toolkit. Installs to .cache/colmap-cuda/install.
--install-system-deps installs C++ dependencies; install CUDA toolkit separately.
Optional environment: OOOSPLAT_BUILD_JOBS (default 2), CUDA_ARCHITECTURES (default detected GPU compute capability).
Use the printed OOOSPLAT_COLMAP path for build, health, and generation commands.
USAGE
  exit 0
fi
if [[ $# -gt 1 || ( $# -eq 1 && "$1" != --install-system-deps ) ]]; then
  echo "Unknown arguments. Use --help." >&2
  exit 2
fi
if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
  echo "Requires Linux x86_64." >&2
  exit 1
fi
if [[ ! -f /etc/os-release ]]; then
  echo "Cannot identify the Linux distribution." >&2
  exit 1
fi
source /etc/os-release
if [[ "${ID:-}" != "ubuntu" || ( "${VERSION_ID:-}" != "22.04" && "${VERSION_ID:-}" != "24.04" ) ]]; then
  echo "Requires Ubuntu 22.04 or 24.04 x86_64." >&2
  exit 1
fi
if [[ "${1:-}" == --install-system-deps ]]; then
  apt=(apt-get)
  if [[ "$(id -u)" != 0 ]]; then apt=(sudo apt-get); fi
  "${apt[@]}" update
  "${apt[@]}" install --no-install-recommends -y \
    git cmake ninja-build build-essential gcc-12 g++-12 \
    libboost-program-options-dev libboost-graph-dev libboost-system-dev \
    libeigen3-dev libflann-dev libfreeimage-dev libmetis-dev liblz4-dev libglew-dev libgl-dev \
    libgoogle-glog-dev libgtest-dev libgmock-dev libsqlite3-dev \
    libcgal-dev libceres-dev
fi
if ! command -v nvcc >/dev/null; then
  for cuda_bin in /usr/local/cuda/bin /usr/local/cuda-12.8/bin; do
    if [[ -x "$cuda_bin/nvcc" ]]; then
      export PATH="$cuda_bin:$PATH"
      break
    fi
  done
fi
for tool in git cmake ninja nvcc gcc-12 g++-12; do
  command -v "$tool" >/dev/null || { echo "Missing $tool. Install a GPU-compatible CUDA toolkit and C++ dependencies. See --help." >&2; exit 1; }
done
cuda_architectures="${CUDA_ARCHITECTURES:-}"
if [[ -z "$cuda_architectures" ]]; then
  compute_capability="$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader 2>/dev/null | head -n 1 || true)"
  if [[ "$compute_capability" =~ ^[0-9]+\.[0-9]+$ ]]; then
    cuda_architectures="${compute_capability/.}"
  else
    echo "Cannot detect CUDA architecture; set CUDA_ARCHITECTURES to the target GPU compute capability (for example 120)." >&2
    exit 1
  fi
fi
source_dir="$workspace/.cache/colmap-cuda/source"
build_dir="$workspace/.cache/colmap-cuda/build"
prefix="$workspace/.cache/colmap-cuda/install"
if [[ ! -d "$source_dir" ]]; then
  git clone --depth 1 --branch 3.11.1 https://github.com/colmap/colmap.git "$source_dir"
fi
# Refuse an existing checkout with another version or local source edits.
[[ "$(git -C "$source_dir" rev-parse HEAD)" == 682ea9ac4020a143047758739259b3ff04dabe8d ]]
[[ -z "$(git -C "$source_dir" status --porcelain)" ]]
cmake -S "$source_dir" -B "$build_dir" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$prefix" \
  -DCMAKE_C_COMPILER=gcc-12 -DCMAKE_CXX_COMPILER=g++-12 \
  -DCMAKE_CUDA_HOST_COMPILER=/usr/bin/g++-12 \
  -DCMAKE_CUDA_ARCHITECTURES="$cuda_architectures" \
  -DCUDA_ENABLED=ON -DGUI_ENABLED=OFF -DOPENGL_ENABLED=OFF
cmake --build "$build_dir" --parallel "${OOOSPLAT_BUILD_JOBS:-2}"
cmake --install "$build_dir"
help_output="$("$prefix/bin/colmap" -h 2>&1)"
if [[ "$help_output" != *"with CUDA"* || "$help_output" == *"without CUDA"* ]]; then
  echo "COLMAP built without CUDA. Check the CMake CUDA detection output." >&2
  exit 1
fi
printf 'CUDA COLMAP installed. Set this in every shell or service environment:\n'
printf 'export OOOSPLAT_COLMAP=%q\n' "$prefix/bin/colmap"
