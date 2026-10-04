use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    engines::ColmapAccelerationStatus,
    error::{Result, SplatError},
    presets::BrushTrainingPreset,
    process::{ProcessManager, ProcessObserver, ProcessSpec},
};

const BRUSH_GPU_LOG_FILTER: &str = "cubecl_wgpu=info,burn_wgpu=info";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrushGpuLaunchPolicy {
    selection_reason: &'static str,
}

pub struct BrushTrainingOptions<'a> {
    pub preset: BrushTrainingPreset,
    pub log_path: PathBuf,
    pub gpu_launch_policy: &'a BrushGpuLaunchPolicy,
}

impl BrushGpuLaunchPolicy {
    pub fn from_acceleration(_acceleration: &ColmapAccelerationStatus) -> Self {
        Self {
            selection_reason: "linux_vulkan_automatic",
        }
    }

    fn environment(&self) -> Vec<(OsString, OsString)> {
        self.environment_with_rust_log(std::env::var_os("RUST_LOG"))
    }

    fn environment_with_rust_log(
        &self,
        existing_rust_log: Option<OsString>,
    ) -> Vec<(OsString, OsString)> {
        let mut environment = Vec::new();
        let rust_log = match existing_rust_log {
            Some(value) if !value.is_empty() => {
                let mut combined = value;
                combined.push(",");
                combined.push(BRUSH_GPU_LOG_FILTER);
                combined
            }
            _ => OsString::from(BRUSH_GPU_LOG_FILTER),
        };
        environment.push((OsString::from("RUST_LOG"), rust_log));
        environment
    }

    pub fn log_summary(&self) -> String {
        format!("backend=Vulkan selection={}", self.selection_reason)
    }
}

fn train_args(
    dataset: &Path,
    output_directory: &Path,
    preset: BrushTrainingPreset,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--total-steps"),
        preset.total_steps.to_string().into(),
        OsString::from("--max-resolution"),
        preset.max_resolution.to_string().into(),
        OsString::from("--refine-every"),
        preset.refine_every.to_string().into(),
    ];
    if let Some(max_splats) = preset.max_splats {
        args.extend([
            OsString::from("--max-splats"),
            max_splats.to_string().into(),
        ]);
    }
    if let Some(densification) = preset.densification {
        args.extend([
            OsString::from("--growth-grad-threshold"),
            densification.growth_grad_threshold.to_string().into(),
            OsString::from("--growth-select-fraction"),
            densification.growth_select_fraction.to_string().into(),
            OsString::from("--growth-stop-iter"),
            densification.growth_stop_iter.to_string().into(),
        ]);
    }
    args.extend([
        OsString::from("--export-every"),
        preset.total_steps.to_string().into(),
        OsString::from("--export-path"),
        output_directory.into(),
        OsString::from("--export-name"),
        OsString::from("final.ply.tmp"),
        dataset.into(),
    ]);
    args
}

pub fn require_verified_cli(executable: &Path) -> Result<()> {
    if executable.is_file() {
        Ok(())
    } else {
        Err(SplatError::EngineMissing(executable.display().to_string()))
    }
}

pub async fn train(
    executable: &Path,
    dataset: &Path,
    output_directory: &Path,
    options: BrushTrainingOptions<'_>,
    manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(output_directory).await?;
    let candidate = output_directory.join("final.ply.tmp");
    let alternate = output_directory.join("final.ply.tmp.ply");
    for partial in [&candidate, &alternate] {
        if partial.exists() {
            tokio::fs::remove_file(partial).await?;
        }
    }
    let environment = options.gpu_launch_policy.environment();
    let output = manager
        .run_with_environment(
            ProcessSpec {
                executable: executable.to_path_buf(),
                args: train_args(dataset, output_directory, options.preset),
                working_directory: Some(output_directory.to_path_buf()),
                log_path: Some(options.log_path),
                observer,
            },
            &environment,
        )
        .await?;
    if !output.success {
        let detail = output.failure_detail();
        if is_out_of_memory_detail(&detail) {
            return Err(SplatError::BrushOutOfMemory(detail));
        }
        if is_device_lost_detail(&detail) {
            return Err(SplatError::BrushDeviceLost(detail));
        }
        return Err(SplatError::Process(format!(
            "Brush 退出码 {:?}{}",
            output.exit_code,
            if detail.is_empty() {
                String::new()
            } else {
                format!("\n{detail}")
            }
        )));
    }
    let candidate = if candidate.is_file() {
        candidate
    } else {
        if alternate.is_file() {
            alternate
        } else {
            candidate
        }
    };
    if !candidate.is_file() {
        return Err(SplatError::Process(format!(
            "Brush 未生成预期文件：{}",
            candidate.display()
        )));
    }
    Ok(candidate)
}

fn is_out_of_memory_detail(detail: &str) -> bool {
    let normalized = detail.to_ascii_lowercase();
    [
        "out of memory",
        "outofmemory",
        "buffertoobig",
        "buffer too big",
        "failed to allocate",
        "allocation failed",
    ]
    .iter()
    .any(|token| normalized.contains(token))
}

fn is_device_lost_detail(detail: &str) -> bool {
    let normalized = detail.to_ascii_lowercase();
    [
        "devicelost",
        "device lost",
        "device_lost",
        "parent device is lost",
        "vk_error_device_lost",
    ]
    .iter()
    .any(|token| normalized.contains(token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engines::{AccelerationReasonCode, AccelerationRequirements, ColmapBackend},
        presets::{resolve_brush_training_preset, Quality},
    };

    fn args_as_strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn cpu_acceleration() -> ColmapAccelerationStatus {
        ColmapAccelerationStatus {
            backend: ColmapBackend::Cpu,
            reason_code: AccelerationReasonCode::ColmapCpuOnly,
            reason: String::new(),
            device: None,
            requirements: AccelerationRequirements {
                minimum_driver_version: "not-applicable".into(),
                minimum_compute_capability: "not-applicable".into(),
            },
            detected_nvidia_device_count: 0,
        }
    }

    fn environment_value<'a>(
        environment: &'a [(OsString, OsString)],
        key: &str,
    ) -> Option<&'a str> {
        environment
            .iter()
            .find(|(candidate, _)| candidate == key)
            .and_then(|(_, value)| value.to_str())
    }

    #[test]
    fn linux_vulkan_policy_preserves_rust_log_and_uses_automatic_device_selection() {
        let policy = BrushGpuLaunchPolicy::from_acceleration(&cpu_acceleration());
        let environment = policy.environment_with_rust_log(Some("app=debug".into()));

        assert_eq!(
            environment_value(&environment, "RUST_LOG"),
            Some("app=debug,cubecl_wgpu=info,burn_wgpu=info")
        );
        assert_eq!(
            policy.log_summary(),
            "backend=Vulkan selection=linux_vulkan_automatic"
        );
    }

    #[test]
    fn default_training_does_not_override_densification() {
        let args = args_as_strings(train_args(
            Path::new("dataset"),
            Path::new("output"),
            resolve_brush_training_preset(Quality::Balanced, false, None, 1_600, 0).preset,
        ));

        assert!(!args.iter().any(|arg| arg == "--growth-select-fraction"));
        assert!(!args.iter().any(|arg| arg == "--growth-stop-iter"));
        assert!(!args.iter().any(|arg| arg == "--max-splats"));
    }

    #[test]
    fn high_profile_passes_all_bounded_training_overrides() {
        let preset =
            resolve_brush_training_preset(Quality::High, true, Some(8_192), 3_840, 60_000).preset;
        let args = args_as_strings(train_args(
            Path::new("dataset"),
            Path::new("output"),
            preset,
        ));

        assert!(args
            .windows(2)
            .any(|pair| pair == ["--growth-grad-threshold", "0.00002"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--growth-select-fraction", "0.3"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--growth-stop-iter", "25000"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--refine-every", "200"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--max-splats", "1500000"]));
    }

    #[test]
    fn fast_and_balanced_pass_efficiency_densification_profiles() {
        let fast = args_as_strings(train_args(
            Path::new("dataset"),
            Path::new("output"),
            resolve_brush_training_preset(Quality::Fast, true, Some(8_192), 3_840, 0).preset,
        ));
        assert!(fast
            .windows(2)
            .any(|pair| pair == ["--growth-select-fraction", "0.15"]));
        assert!(fast
            .windows(2)
            .any(|pair| pair == ["--growth-stop-iter", "6000"]));

        let balanced = args_as_strings(train_args(
            Path::new("dataset"),
            Path::new("output"),
            resolve_brush_training_preset(Quality::Balanced, true, Some(8_192), 3_840, 0).preset,
        ));
        assert!(balanced
            .windows(2)
            .any(|pair| pair == ["--growth-grad-threshold", "0.00003"]));
        assert!(balanced
            .windows(2)
            .any(|pair| pair == ["--growth-stop-iter", "12000"]));
        assert!(!balanced.iter().any(|arg| arg == "--max-splats"));
    }

    #[test]
    fn gpu_failure_classifiers_keep_oom_and_device_loss_separate() {
        assert!(is_out_of_memory_detail("OutOfMemory while allocating"));
        assert!(is_out_of_memory_detail("BufferTooBig(2290420416)"));
        assert!(is_out_of_memory_detail("GPU allocation failed"));
        assert!(!is_out_of_memory_detail("DeviceLost: driver reset"));
        assert!(is_device_lost_detail("DeviceLost: driver reset"));
        assert!(is_device_lost_detail("Parent device is lost"));
        assert!(is_device_lost_detail("VK_ERROR_DEVICE_LOST"));
        assert!(!is_device_lost_detail("process exited with code 101"));
        assert!(!is_device_lost_detail(
            "process exited with code -1073740791"
        ));
        assert!(!is_out_of_memory_detail("process exited with code 1"));
    }
}
