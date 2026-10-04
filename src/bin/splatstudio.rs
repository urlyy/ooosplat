#[path = "splatstudio/headless.rs"]
mod headless;

use std::{path::PathBuf, time::Duration};

use clap::{Parser, Subcommand};
use ooo_splat::{
    engines::{ffmpeg::extract_uniform_frames, ffprobe::probe_video, EngineStatus},
    error::{Result, SplatError},
    pipeline::{progress::stage_progress_range, runner::default_engine_paths, PipelineStage},
    presets::Quality,
    process::ProcessManager,
    project::{catalog, PipelineStateFile, ProjectStatus},
    video::{
        analyze_image_sequence, create_image_plan, prepare_image_sequence, FrameSelectionStrategy,
        UniformRatioFrameSelection,
    },
};
use uuid::Uuid;

use headless::{pause_active, read_active_task, run_managed, spawn_background, RunRequest};

#[derive(Debug, Parser)]
#[command(name = "splatstudio", version, about = "OOOSplat local pipeline CLI")]
struct Cli {
    /// Override the bundled engine directory (also supports OOOSPLAT_ENGINE_DIR).
    #[arg(long, global = true)]
    engine_dir: Option<PathBuf>,
    /// Print command results as JSON. With `status --watch`, emits NDJSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Validate FFmpeg, FFprobe, auto GPU/CPU COLMAP and Brush.
    Health,
    /// Read video metadata or image-sequence information.
    Probe { input: PathBuf },
    /// Show the video frame plan or image-sequence plan.
    Plan {
        input: PathBuf,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
    },
    /// Prepare video frames or an image sequence and optional COLMAP masks.
    Extract {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
    },
    /// Run the end-to-end pipeline after all fixed engine CLIs are verified.
    Generate {
        input: PathBuf,
        /// Override the remembered projects root (useful for diagnostics).
        #[arg(long)]
        projects_root: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
        /// Run in the background. Use `status --watch` to follow progress.
        #[arg(long)]
        background: bool,
    },
    /// Continue an interrupted task from its latest validated checkpoint.
    Resume {
        /// Full task UUID or an unambiguous UUID prefix from `tasks`.
        task: String,
        /// Run in the background. Use `status --watch` to follow progress.
        #[arg(long)]
        background: bool,
    },
    /// List all known tasks and their IDs.
    Tasks,
    /// Show the active task or one persisted task.
    Status {
        /// Optional full task UUID or an unambiguous UUID prefix.
        task: Option<String>,
        /// Keep displaying progress until the active task exits.
        #[arg(long)]
        watch: bool,
    },
    /// Gracefully pause the active task and preserve resumable checkpoints.
    Pause,
    /// Pause the active task, then start a new video or resume a task ID.
    Switch {
        /// Existing video/image directory, or a task UUID/prefix.
        target: String,
        #[arg(long)]
        projects_root: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
    },
    /// Internal background worker for a new task.
    #[command(hide = true)]
    WorkerGenerate {
        input: PathBuf,
        #[arg(long)]
        projects_root: PathBuf,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
    },
    /// Internal background worker for a resumed task.
    #[command(hide = true)]
    WorkerResume { task: Uuid },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();
    if let Err(error) = execute(Cli::parse()).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn execute(cli: Cli) -> Result<()> {
    let engine_dir = cli.engine_dir;
    let json = cli.json;
    let engines = default_engine_paths(engine_dir.clone());
    match cli.command {
        Commands::Health => {
            let statuses = engines.check_all().await;
            println!("{}", serde_json::to_string_pretty(&statuses)?);
            ensure_healthy(&statuses)?;
        }
        Commands::Probe { input } => {
            if input.is_dir() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&analyze_image_sequence(&input)?)?
                );
            } else {
                let video =
                    probe_video(&engines.ffprobe, &input, None, &ProcessManager::new()).await?;
                println!("{}", serde_json::to_string_pretty(&video)?);
            }
        }
        Commands::Plan { input, quality } => {
            let plan = if input.is_dir() {
                create_image_plan(&analyze_image_sequence(&input)?, &quality.preset())
            } else {
                let video =
                    probe_video(&engines.ffprobe, &input, None, &ProcessManager::new()).await?;
                UniformRatioFrameSelection.create_plan(&video, &quality.preset())
            };
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
        Commands::Extract {
            input,
            output,
            quality,
        } => {
            let masks = output
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("masks");
            if input.is_dir() {
                let extraction = prepare_image_sequence(&input, &output, &masks)?;
                println!(
                    "prepared {} images in {} and {} masks in {}",
                    extraction.image_count,
                    output.display(),
                    extraction.mask_count,
                    masks.display()
                );
                return Ok(());
            }
            ensure_engine(&engines.ffprobe)?;
            ensure_engine(&engines.ffmpeg)?;
            let video = probe_video(&engines.ffprobe, &input, None, &ProcessManager::new()).await?;
            let plan = UniformRatioFrameSelection.create_plan(&video, &quality.preset());
            let extraction = extract_uniform_frames(
                &engines.ffmpeg,
                &input,
                &output,
                &masks,
                &plan,
                video.has_alpha,
                None,
                None,
                &ProcessManager::new(),
                None,
            )
            .await?;
            if extraction.has_alpha {
                println!(
                    "extracted {} RGBA frames to {} and {} masks to {}",
                    extraction.frame_count,
                    output.display(),
                    extraction.mask_count,
                    masks.display()
                );
            } else {
                println!(
                    "extracted {} frames to {}",
                    extraction.frame_count,
                    output.display()
                );
            }
        }
        Commands::Generate {
            input,
            projects_root,
            quality,
            background,
        } => {
            let request = RunRequest::Generate {
                input,
                projects_root: resolve_projects_root(projects_root).await?,
                quality,
            };
            if background {
                print_launch(
                    spawn_background(engine_dir.as_deref(), &request).await?,
                    json,
                )?;
            } else {
                let result = run_managed(engines, request, true).await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }
        Commands::Resume { task, background } => {
            let project_id = resolve_task_id(&task).await?;
            let request = RunRequest::Resume { project_id };
            if background {
                print_launch(
                    spawn_background(engine_dir.as_deref(), &request).await?,
                    json,
                )?;
            } else {
                let result = run_managed(engines, request, true).await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }
        Commands::Tasks => print_tasks(json).await?,
        Commands::Status { task, watch } => show_status(task.as_deref(), watch, json).await?,
        Commands::Pause => {
            let paused = pause_active().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&paused)?);
            } else if let Some(active) = paused {
                if let Some(id) = active.project_id {
                    println!(
                        "已暂停任务 {id}（PID {}），可使用 resume {id} 继续。",
                        active.pid
                    );
                } else {
                    println!(
                        "已暂停 PID {}；项目初始化刚完成，请运行 tasks 查看任务 ID。",
                        active.pid
                    );
                }
            } else {
                println!("当前没有运行中的任务。");
            }
        }
        Commands::Switch {
            target,
            projects_root,
            quality,
        } => {
            let target_path = PathBuf::from(&target);
            let request = if target_path.exists() {
                RunRequest::Generate {
                    input: target_path,
                    projects_root: resolve_projects_root(projects_root).await?,
                    quality,
                }
            } else {
                RunRequest::Resume {
                    project_id: resolve_task_id(&target).await?,
                }
            };
            if let Some(active) = pause_active().await? {
                eprintln!(
                    "已安全暂停 PID {}{}。",
                    active.pid,
                    active
                        .project_id
                        .map(|id| format!("（任务 {id}）"))
                        .unwrap_or_default()
                );
            }
            print_launch(
                spawn_background(engine_dir.as_deref(), &request).await?,
                json,
            )?;
        }
        Commands::WorkerGenerate {
            input,
            projects_root,
            quality,
        } => {
            let result = run_managed(
                engines,
                RunRequest::Generate {
                    input,
                    projects_root,
                    quality,
                },
                false,
            )
            .await?;
            println!("{}", serde_json::to_string(&result)?);
        }
        Commands::WorkerResume { task } => {
            let result =
                run_managed(engines, RunRequest::Resume { project_id: task }, false).await?;
            println!("{}", serde_json::to_string(&result)?);
        }
    }
    Ok(())
}

async fn resolve_projects_root(root: Option<PathBuf>) -> Result<PathBuf> {
    Ok(match root {
        Some(root) => root,
        None => catalog::load_settings().await?.projects_root,
    })
}

async fn resolve_task_id(value: &str) -> Result<Uuid> {
    let overview = catalog::get_overview().await?;
    if let Ok(id) = Uuid::parse_str(value) {
        if overview.projects.iter().any(|project| project.id == id) {
            return Ok(id);
        }
    }
    let matches = overview
        .projects
        .iter()
        .filter(|project| project.id.to_string().starts_with(value))
        .map(|project| project.id)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [id] => Ok(*id),
        [] => Err(SplatError::Process(format!("找不到任务：{value}"))),
        _ => Err(SplatError::Process(format!(
            "任务 ID 前缀不唯一，请输入更多字符：{value}"
        ))),
    }
}

fn print_launch(launch: headless::BackgroundLaunch, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&launch)?);
    } else {
        println!("后台任务已启动，PID {}。", launch.pid);
        if let Some(id) = launch.project_id {
            println!("任务 ID：{id}");
        }
        println!("实时进度：splatstudio status --watch");
        println!("运行日志：{}", launch.log_path.display());
    }
    Ok(())
}

async fn print_tasks(json: bool) -> Result<()> {
    let mut overview = catalog::get_overview().await?;
    let active_id = read_active_task()?.and_then(|active| active.project_id);
    for project in &mut overview.projects {
        if project.status == ProjectStatus::Running && active_id != Some(project.id) {
            project.status = ProjectStatus::Interrupted;
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&overview)?);
        return Ok(());
    }
    if overview.projects.is_empty() {
        println!("还没有任务。项目目录：{}", overview.projects_root.display());
        return Ok(());
    }
    println!("{:<10}  {:<12}  {:<10}  PROJECT", "ID", "STATUS", "QUALITY");
    for project in overview.projects {
        println!(
            "{:<10}  {:<12}  {:<10}  {}",
            &project.id.to_string()[..8],
            project_status_name(project.status),
            format!("{:?}", project.quality).to_lowercase(),
            project.project_path.display()
        );
    }
    Ok(())
}

async fn show_status(task: Option<&str>, watch: bool, json: bool) -> Result<()> {
    let task_id = match task {
        Some(value) => Some(resolve_task_id(value).await?),
        None => None,
    };
    let mut last_active_project = None;
    loop {
        let active = read_active_task()?;
        let selected_active = active
            .as_ref()
            .filter(|active| task_id.is_none() || active.project_id == task_id);
        if let Some(active) = selected_active {
            if active.project_id.is_some() {
                last_active_project = active.project_id;
            }
            if json {
                println!("{}", serde_json::to_string(active)?);
            } else {
                print_active_status(active);
            }
        } else if let Some(project_id) = task_id.or(last_active_project) {
            print_persisted_status(project_id, json, watch).await?;
            break;
        } else {
            if json {
                println!("null");
            } else {
                println!("当前没有运行中的任务。使用 `splatstudio tasks` 查看历史任务。");
            }
            break;
        }
        if !watch {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Ok(())
}

fn print_active_status(active: &headless::ActiveTask) {
    let event = active.event.as_ref();
    let progress = event.map_or(0.0, |event| event.progress);
    let width = 30usize;
    let filled = ((progress.clamp(0.0, 100.0) / 100.0) * width as f32).round() as usize;
    let bar = format!("{}{}", "#".repeat(filled), "-".repeat(width - filled));
    let task = active
        .project_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "initializing".into());
    let stage = event
        .map(|event| headless::stage_name(event.stage))
        .unwrap_or("starting");
    let message = event.map_or("正在启动", |event| event.message.as_str());
    println!(
        "[{}] {:>6.2}% {:<12} task={} pid={} {}{}",
        bar,
        progress,
        stage,
        task,
        active.pid,
        message.replace(['\r', '\n'], " "),
        if active.stopping {
            "（正在暂停）"
        } else {
            ""
        }
    );
}

async fn print_persisted_status(project_id: Uuid, json: bool, compact_json: bool) -> Result<()> {
    let (root, mut metadata) = catalog::load_registered_project(project_id).await?;
    if metadata.status == ProjectStatus::Running {
        metadata.status = ProjectStatus::Interrupted;
    }
    let state: Option<PipelineStateFile> = tokio::fs::read(root.join("state.json"))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    if json {
        let value = serde_json::json!({
            "active": false,
            "project": metadata,
            "checkpoint": state,
        });
        println!(
            "{}",
            if compact_json {
                serde_json::to_string(&value)?
            } else {
                serde_json::to_string_pretty(&value)?
            }
        );
        return Ok(());
    }
    let (stage, progress) = state
        .as_ref()
        .map(checkpoint_progress)
        .unwrap_or((PipelineStage::Created, 0.0));
    println!("任务 ID：{}", metadata.id);
    println!("状态：{}", project_status_name(metadata.status));
    println!(
        "最近断点：{}（约 {:.0}%）",
        headless::stage_name(stage),
        progress
    );
    println!("项目目录：{}", root.display());
    if let Some(output) = metadata.output {
        println!("结果：{}", output.final_ply.display());
    }
    if let Some(error) = metadata.failure_message {
        println!("最近信息：{}", error.replace(['\r', '\n'], " "));
    }
    Ok(())
}

fn checkpoint_progress(state: &PipelineStateFile) -> (PipelineStage, f32) {
    let stage = if state.stage == PipelineStage::Completed {
        PipelineStage::Completed
    } else if state.brush_complete {
        PipelineStage::TrainingSplats
    } else if state.reconstruction_complete {
        PipelineStage::ValidatingReconstruction
    } else if state.matching_complete {
        PipelineStage::Matching
    } else if state.features_complete {
        PipelineStage::ExtractingFeatures
    } else if state
        .frames
        .as_ref()
        .and_then(|frames| frames.extracted_frames)
        .is_some()
    {
        PipelineStage::ExtractingFrames
    } else if state.video.is_some() || state.image_sequence.is_some() {
        PipelineStage::PlanningFrames
    } else {
        PipelineStage::Created
    };
    let (_, end) = stage_progress_range(stage);
    (stage, end)
}

fn ensure_healthy(statuses: &[EngineStatus]) -> Result<()> {
    let failed = statuses
        .iter()
        .filter(|status| !status.can_start)
        .map(|status| format!("{:?}: {}", status.kind, status.detail))
        .collect::<Vec<_>>();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(SplatError::Process(format!(
            "引擎健康检查失败：{}",
            failed.join("；")
        )))
    }
}

fn project_status_name(status: ProjectStatus) -> &'static str {
    match status {
        ProjectStatus::Running => "running",
        ProjectStatus::Completed => "completed",
        ProjectStatus::Failed => "failed",
        ProjectStatus::Cancelled => "paused",
        ProjectStatus::Interrupted => "interrupted",
    }
}

fn ensure_engine(path: &std::path::Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(SplatError::EngineMissing(path.display().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ooo_splat::engines::EngineKind;

    fn engine_status(kind: EngineKind, can_start: bool) -> EngineStatus {
        EngineStatus {
            kind,
            path: PathBuf::from("/tmp/engine"),
            exists: can_start,
            can_start,
            version: None,
            cpu_only: None,
            acceleration: None,
            colmap_cli_family: None,
            detail: if can_start {
                "ok".into()
            } else {
                "missing".into()
            },
        }
    }

    #[test]
    fn health_fails_if_any_engine_cannot_start() {
        let statuses = [
            engine_status(EngineKind::Ffmpeg, true),
            engine_status(EngineKind::Brush, false),
        ];
        assert!(ensure_healthy(&statuses).is_err());
    }

    #[test]
    fn completed_checkpoint_reports_one_hundred_percent() {
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.stage = PipelineStage::Completed;
        state.brush_complete = true;
        assert_eq!(
            checkpoint_progress(&state),
            (PipelineStage::Completed, 100.0)
        );
    }
}
