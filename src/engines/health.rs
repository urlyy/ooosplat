use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    engines::colmap::{detect_cli_family, ColmapCliFamily},
    process::{ProcessManager, ProcessSpec},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Ffmpeg,
    Ffprobe,
    Colmap,
    Brush,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub kind: EngineKind,
    pub path: PathBuf,
    pub exists: bool,
    pub can_start: bool,
    pub version: Option<String>,
    pub cpu_only: Option<bool>,
    pub acceleration: Option<ColmapAccelerationStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colmap_cli_family: Option<ColmapCliFamily>,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColmapBackend {
    Cpu,
    Gpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccelerationReasonCode {
    ColmapCpuOnly,
    ColmapUnavailable,
    GpuReady,
    GpuProbeFailed,
    CpuRequested,
    InvalidBackend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuDeviceInfo {
    pub index: u32,
    pub name: String,
    pub driver_version: String,
    pub compute_capability: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_memory_mb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccelerationRequirements {
    pub minimum_driver_version: String,
    pub minimum_compute_capability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColmapAccelerationStatus {
    pub backend: ColmapBackend,
    pub reason_code: AccelerationReasonCode,
    pub reason: String,
    pub device: Option<GpuDeviceInfo>,
    pub requirements: AccelerationRequirements,
    #[serde(default)]
    pub detected_nvidia_device_count: usize,
}

impl ColmapAccelerationStatus {
    pub const fn use_gpu(&self) -> bool {
        matches!(self.backend, ColmapBackend::Gpu)
    }

    /// Brush selects its Vulkan device independently of the CUDA device. Do not
    /// use COLMAP VRAM to size Brush allocations, especially on multi-GPU hosts.
    pub fn usable_gpu_total_memory_mb(&self) -> Option<u64> {
        None
    }

    pub fn gpu_index(&self) -> Option<u32> {
        self.use_gpu()
            .then(|| self.device.as_ref().map(|device| device.index))
            .flatten()
    }
}

#[derive(Debug, Clone)]
pub struct EnginePaths {
    pub root: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub colmap: PathBuf,
    pub brush: PathBuf,
}

impl EnginePaths {
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            ffmpeg: root.join("ffmpeg"),
            ffprobe: root.join("ffprobe"),
            colmap: root.join("colmap"),
            brush: root.join("brush_app"),
            root,
        }
    }

    pub fn from_candidates(root: PathBuf) -> Self {
        let defaults = Self::from_root(root.clone());
        Self {
            ffmpeg: resolve_engine(
                "OOOSPLAT_FFMPEG",
                std::slice::from_ref(&defaults.ffmpeg),
                "ffmpeg",
            ),
            ffprobe: resolve_engine(
                "OOOSPLAT_FFPROBE",
                std::slice::from_ref(&defaults.ffprobe),
                "ffprobe",
            ),
            colmap: resolve_engine(
                "OOOSPLAT_COLMAP",
                std::slice::from_ref(&defaults.colmap),
                "colmap",
            ),
            brush: resolve_engine(
                "OOOSPLAT_BRUSH",
                &[
                    defaults.brush.clone(),
                    root.join("linux").join("brush").join("brush_app"),
                    root.join("brush").join("brush_app"),
                ],
                "brush_app",
            ),
            root,
        }
    }

    pub fn discover(resource_dir: Option<&Path>) -> Self {
        if let Some(value) = std::env::var_os("OOOSPLAT_ENGINE_DIR") {
            return Self::from_candidates(value.into());
        }

        let current = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let candidates = vec![
            resource_dir.map(|path| path.join("engines")),
            Some(current.join("engines")),
            Some(current.join("..").join("engines")),
        ];
        let mut candidates = candidates;
        if let Ok(executable) = std::env::current_exe() {
            candidates.extend(
                executable
                    .ancestors()
                    .skip(1)
                    .map(|ancestor| Some(ancestor.join("engines"))),
            );
        }
        let fallback = current.join("engines");
        let root = candidates
            .into_iter()
            .flatten()
            .find(|path| path.is_dir())
            .unwrap_or(fallback);
        Self::from_candidates(root)
    }

    pub async fn check_all(&self) -> Vec<EngineStatus> {
        let (ffmpeg, ffprobe, colmap, brush) = tokio::join!(
            check_basic(EngineKind::Ffmpeg, &self.ffmpeg, &["-version"]),
            check_basic(EngineKind::Ffprobe, &self.ffprobe, &["-version"]),
            check_colmap(&self.colmap, &self.root),
            check_basic(EngineKind::Brush, &self.brush, &["--help"]),
        );
        vec![ffmpeg, ffprobe, colmap, brush]
    }
}

fn resolve_engine(env_name: &str, managed: &[PathBuf], executable_name: &str) -> PathBuf {
    if let Some(path) = std::env::var_os(env_name) {
        return path.into();
    }
    managed
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .or_else(|| find_on_path(executable_name))
        .unwrap_or_else(|| managed[0].clone())
}

fn find_on_path(executable_name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(executable_name))
        .find(|candidate| candidate.is_file())
}

fn missing(kind: EngineKind, path: &Path) -> EngineStatus {
    EngineStatus {
        kind,
        path: path.to_path_buf(),
        exists: false,
        can_start: false,
        version: None,
        cpu_only: None,
        acceleration: None,
        colmap_cli_family: None,
        detail: format!("未找到 {}", path.display()),
    }
}

async fn check_basic(kind: EngineKind, path: &Path, args: &[&str]) -> EngineStatus {
    if !path.is_file() {
        return missing(kind, path);
    }
    let manager = ProcessManager::new();
    let result = manager
        .run(ProcessSpec {
            executable: path.to_path_buf(),
            args: args.iter().map(OsString::from).collect(),
            working_directory: path.parent().map(Path::to_path_buf),
            log_path: None,
            observer: None,
        })
        .await;

    match result {
        Ok(output) => {
            let combined = format!("{}\n{}", output.stdout, output.stderr);
            let first_line = combined
                .lines()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_owned());
            EngineStatus {
                kind,
                path: path.to_path_buf(),
                exists: true,
                can_start: output.success,
                version: first_line,
                cpu_only: None,
                acceleration: None,
                colmap_cli_family: None,
                detail: if output.success {
                    "引擎可启动".into()
                } else {
                    format!("帮助命令退出码：{:?}", output.exit_code)
                },
            }
        }
        Err(error) => EngineStatus {
            kind,
            path: path.to_path_buf(),
            exists: true,
            can_start: false,
            version: None,
            cpu_only: None,
            acceleration: None,
            colmap_cli_family: None,
            detail: error.to_string(),
        },
    }
}

async fn check_colmap(path: &Path, _engines_root: &Path) -> EngineStatus {
    if !path.is_file() {
        let mut status = missing(EngineKind::Colmap, path);
        status.acceleration = Some(cpu_status(
            AccelerationReasonCode::ColmapUnavailable,
            format!("未找到 COLMAP：{}", path.display()),
        ));
        return status;
    }
    let manager = ProcessManager::new();
    let mut help = String::new();
    let mut feature_help = String::new();
    let mut matching_help = String::new();
    let mut successful = true;
    for args in [
        vec!["feature_extractor", "-h"],
        vec!["sequential_matcher", "-h"],
        vec!["mapper", "-h"],
    ] {
        let command_name = args[0];
        match manager
            .run(ProcessSpec {
                executable: path.to_path_buf(),
                args: args.into_iter().map(OsString::from).collect(),
                working_directory: path.parent().map(Path::to_path_buf),
                log_path: None,
                observer: None,
            })
            .await
        {
            Ok(output) => {
                successful &= output.success;
                let command_help = format!("{}\n{}", output.stdout, output.stderr);
                match command_name {
                    "feature_extractor" => feature_help = command_help.clone(),
                    "sequential_matcher" => matching_help = command_help.clone(),
                    _ => {}
                }
                help.push_str(&command_help);
            }
            Err(error) => {
                return EngineStatus {
                    kind: EngineKind::Colmap,
                    path: path.to_path_buf(),
                    exists: true,
                    can_start: false,
                    version: None,
                    cpu_only: None,
                    acceleration: Some(cpu_status(
                        AccelerationReasonCode::ColmapUnavailable,
                        format!("COLMAP 无法启动：{error}"),
                    )),
                    colmap_cli_family: None,
                    detail: error.to_string(),
                }
            }
        }
    }

    let cli_family = detect_cli_family(&feature_help, &matching_help);
    successful &= cli_family.is_some();
    let first_line = help
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_owned());
    let mode = std::env::var("OOOSPLAT_COLMAP_BACKEND").unwrap_or_else(|_| "auto".into());
    let cuda = has_cuda_support(&help);
    let acceleration = if !successful {
        cpu_status(
            AccelerationReasonCode::ColmapUnavailable,
            "COLMAP 必需命令无法正常启动".into(),
        )
    } else if !matches!(mode.as_str(), "auto" | "cpu" | "gpu") {
        successful = false;
        cpu_status(
            AccelerationReasonCode::InvalidBackend,
            "OOOSPLAT_COLMAP_BACKEND 必须是 auto、cpu 或 gpu".into(),
        )
    } else if mode == "cpu" {
        cpu_status(
            AccelerationReasonCode::CpuRequested,
            "已显式选择 COLMAP CPU；Brush 仍使用 Vulkan".into(),
        )
    } else if !cuda {
        cpu_status(
            AccelerationReasonCode::ColmapCpuOnly,
            "COLMAP 未报告 CUDA 支持，请安装 CUDA 构建（scripts/setup-colmap-cuda-linux.sh）"
                .into(),
        )
    } else {
        match super::colmap_gpu::probe(path).await {
            Ok(()) => {
                let mut status = cpu_status(
                    AccelerationReasonCode::GpuReady,
                    "CUDA 特征提取和匹配探测通过，使用 CUDA 可见设备 0；Mapper 主要使用 CPU".into(),
                );
                status.backend = ColmapBackend::Gpu;
                status.device = Some(GpuDeviceInfo {
                    index: 0,
                    name: "CUDA visible device 0".into(),
                    driver_version: "runtime-probed".into(),
                    compute_capability: "runtime-probed".into(),
                    total_memory_mb: None,
                });
                status
            }
            Err(error) => cpu_status(
                AccelerationReasonCode::GpuProbeFailed,
                format!("CUDA 探测失败：{error}；auto 模式回退 CPU"),
            ),
        }
    };
    if mode == "gpu" && !acceleration.use_gpu() {
        successful = false;
    }
    let family_label = cli_family.map_or("不支持的 CLI", ColmapCliFamily::label);
    let detail = format!("{family_label}；mode={mode}；{}", acceleration.reason);
    EngineStatus {
        kind: EngineKind::Colmap,
        path: path.to_path_buf(),
        exists: true,
        can_start: successful,
        version: first_line,
        cpu_only: Some(!cuda),
        acceleration: Some(acceleration),
        colmap_cli_family: cli_family,
        detail,
    }
}

fn has_cuda_support(help: &str) -> bool {
    let lower = help.to_ascii_lowercase();
    lower.contains("with cuda") && !lower.contains("without cuda")
}

pub async fn check_colmap_acceleration(paths: &EnginePaths) -> ColmapAccelerationStatus {
    check_colmap(&paths.colmap, &paths.root)
        .await
        .acceleration
        .unwrap_or_else(|| {
            cpu_status(
                AccelerationReasonCode::ColmapUnavailable,
                "无法读取 COLMAP 状态，已保持 CPU 模式".into(),
            )
        })
}

fn cpu_status(reason_code: AccelerationReasonCode, reason: String) -> ColmapAccelerationStatus {
    ColmapAccelerationStatus {
        backend: ColmapBackend::Cpu,
        reason_code,
        reason,
        device: None,
        requirements: AccelerationRequirements {
            minimum_driver_version: "not-applicable".into(),
            minimum_compute_capability: "not-applicable".into(),
        },
        detected_nvidia_device_count: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_executable_on_path() {
        assert!(find_on_path("cargo").is_some());
    }

    #[test]
    fn linux_root_is_flat_and_discovery_can_fall_back_to_path() {
        let paths = EnginePaths::from_root("/opt/ooosplat-engines");
        assert_eq!(paths.colmap, PathBuf::from("/opt/ooosplat-engines/colmap"));
        let discovered = EnginePaths::from_candidates(PathBuf::from("/missing/engines"));
        let expected = std::env::var_os("OOOSPLAT_FFMPEG")
            .map(PathBuf::from)
            .or_else(|| find_on_path("ffmpeg"))
            .unwrap_or_else(|| PathBuf::from("/missing/engines/ffmpeg"));
        assert_eq!(discovered.ffmpeg, expected);
    }

    #[test]
    fn colmap_cpu_status_never_selects_a_gpu() {
        let status = cpu_status(
            AccelerationReasonCode::ColmapCpuOnly,
            "fixed CPU mode".into(),
        );
        assert_eq!(status.backend, ColmapBackend::Cpu);
        assert!(!status.use_gpu());
        assert_eq!(status.gpu_index(), None);
        assert_eq!(status.usable_gpu_total_memory_mb(), None);
        assert_eq!(status.detected_nvidia_device_count, 0);
    }
}
