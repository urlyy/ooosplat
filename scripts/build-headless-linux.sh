#!/usr/bin/env bash
set -euo pipefail

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
install_system_deps=false

usage() {
  cat <<'EOF'
Usage: scripts/build-headless-linux.sh [--install-system-deps]

Build the OOOSplat command-line runner for Ubuntu 24.04 x86_64.

Options:
  --install-system-deps  Install Ubuntu 24.04 build/runtime packages with apt.
  -h, --help             Show this help.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-system-deps)
      install_system_deps=true
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  echo "This build is supported only on Linux x86_64 (validated on Ubuntu 24.04)." >&2
  exit 1
fi

if $install_system_deps; then
  if [[ "$(id -u)" -eq 0 ]]; then
    apt=(apt-get)
  else
    apt=(sudo apt-get)
  fi
  "${apt[@]}" update
  "${apt[@]}" install -y \
    build-essential ca-certificates colmap curl ffmpeg libssl-dev libvulkan1 \
    mesa-vulkan-drivers pkg-config xz-utils
fi

for command_name in cargo curl ffmpeg ffprobe sha256sum tar; do
  command -v "$command_name" >/dev/null || {
    echo "Missing $command_name. Install dependencies or rerun with --install-system-deps." >&2
    exit 1
  }
done

"$workspace/scripts/setup-engines-linux.sh"

cargo build \
  --locked \
  --release \
  --manifest-path "$workspace/Cargo.toml" \
  --bin splatstudio

destination="$workspace/dist-headless"
rm -rf -- "$destination"
mkdir -p "$destination/engines/linux/brush" "$destination/licenses"
install -m 0755 "$workspace/target/release/splatstudio" "$destination/splatstudio"
install -m 0755 "$workspace/engines/linux/brush/brush_app" "$destination/engines/linux/brush/brush_app"
install -m 0644 "$workspace/engines/manifest.linux.json" "$destination/engines/manifest.linux.json"
install -m 0644 "$workspace/LICENSE" "$destination/LICENSE"
install -m 0644 "$workspace/NOTICE" "$destination/NOTICE"
install -m 0644 "$workspace/TRADEMARK_POLICY.md" "$destination/TRADEMARK_POLICY.md"
install -m 0644 "$workspace/GENERATED_OUTPUTS.md" "$destination/GENERATED_OUTPUTS.md"
install -m 0644 "$workspace/licenses/Brush-LICENSE.txt" "$destination/licenses/Brush-LICENSE.txt"
install -m 0644 "$workspace/licenses/COLMAP-LICENSE.txt" "$destination/licenses/COLMAP-LICENSE.txt"
install -m 0644 "$workspace/licenses/FFmpeg-LGPL-2.1.txt" "$destination/licenses/FFmpeg-LGPL-2.1.txt"
install -m 0644 "$workspace/licenses/MIT.txt" "$destination/licenses/MIT.txt"
install -m 0644 "$workspace/licenses/MPL-2.0.txt" "$destination/licenses/MPL-2.0.txt"
install -m 0644 "$workspace/licenses/THIRD_PARTY_NOTICES.txt" "$destination/licenses/THIRD_PARTY_NOTICES.txt"
install -m 0644 "$workspace/licenses/Unicode-3.0.txt" "$destination/licenses/Unicode-3.0.txt"
install -m 0644 "$workspace/licenses/Zlib.txt" "$destination/licenses/Zlib.txt"

echo "Built headless runner: $destination/splatstudio"
echo "Verify with: OOOSPLAT_ENGINE_DIR=$destination/engines $destination/splatstudio health"
