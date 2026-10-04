use std::{
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use ooo_splat::{
    engines::EnginePaths,
    error::{Result, SplatError},
    pipeline::{
        event::PipelineEvent,
        runner::{PipelineFailureContext, PipelineResult, PipelineRunner},
        PipelineStage,
    },
    presets::Quality,
};
use serde::{Deserialize, Serialize};
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

const RUNTIME_SCHEMA_VERSION: u32 = 1;
const STARTUP_WAIT: Duration = Duration::from_secs(10);
const STOP_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub enum RunRequest {
    Generate {
        input: PathBuf,
        projects_root: PathBuf,
        quality: Quality,
    },
    Resume {
        project_id: Uuid,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTask {
    pub schema_version: u32,
    pub pid: u32,
    pub action: String,
    pub input: Option<PathBuf>,
    pub project_id: Option<Uuid>,
    pub project_path: Option<PathBuf>,
    pub logs_directory: Option<PathBuf>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub stopping: bool,
    pub event: Option<PipelineEvent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundLaunch {
    pub pid: u32,
    pub project_id: Option<Uuid>,
    pub log_path: PathBuf,
    pub status_path: PathBuf,
}

pub struct ProgressReporter {
    active_path: PathBuf,
    foreground: bool,
    state: Mutex<ActiveTask>,
    console: Mutex<ConsoleState>,
}

#[derive(Default)]
struct ConsoleState {
    last_progress_tenth: i32,
    last_stage: Option<PipelineStage>,
    last_message: String,
    line_open: bool,
}

impl ProgressReporter {
    fn new(request: &RunRequest, foreground: bool) -> Result<Self> {
        let (action, input, project_id) = match request {
            RunRequest::Generate { input, .. } => {
                ("generate".to_string(), Some(input.clone()), None)
            }
            RunRequest::Resume { project_id } => ("resume".to_string(), None, Some(*project_id)),
        };
        let now = Utc::now();
        let reporter = Self {
            active_path: active_status_path()?,
            foreground,
            state: Mutex::new(ActiveTask {
                schema_version: RUNTIME_SCHEMA_VERSION,
                pid: std::process::id(),
                action,
                input,
                project_id,
                project_path: None,
                logs_directory: None,
                started_at: now,
                updated_at: now,
                stopping: false,
                event: None,
            }),
            console: Mutex::new(ConsoleState {
                last_progress_tenth: -1,
                ..Default::default()
            }),
        };
        reporter.persist()?;
        Ok(reporter)
    }

    pub fn on_event(&self, event: PipelineEvent) {
        {
            let mut state = lock(&self.state);
            state.updated_at = Utc::now();
            state.event = Some(event.clone());
            let _ = write_json_atomic(&self.active_path, &*state);
        }
        if self.foreground {
            self.render(&event);
        }
    }

    pub fn update_context(&self, context: &PipelineFailureContext) {
        let mut state = lock(&self.state);
        let changed = state.project_id != context.project_id
            || state.project_path != context.project_path
            || state.logs_directory != context.logs_directory;
        if changed {
            state.project_id = context.project_id;
            state.project_path.clone_from(&context.project_path);
            state.logs_directory.clone_from(&context.logs_directory);
            state.updated_at = Utc::now();
            let _ = write_json_atomic(&self.active_path, &*state);
        }
    }

    pub fn mark_stopping(&self) {
        let mut state = lock(&self.state);
        state.stopping = true;
        state.updated_at = Utc::now();
        let _ = write_json_atomic(&self.active_path, &*state);
        if self.foreground {
            let mut console = lock(&self.console);
            if console.line_open {
                eprintln!();
                console.line_open = false;
            }
            eprintln!("收到停止信号，正在安全结束当前阶段并保存断点……");
        }
    }

    fn persist(&self) -> Result<()> {
        write_json_atomic(&self.active_path, &*lock(&self.state))
    }

    fn render(&self, event: &PipelineEvent) {
        let mut console = lock(&self.console);
        let progress_tenth = (event.progress.clamp(0.0, 100.0) * 10.0).round() as i32;
        let changed = progress_tenth != console.last_progress_tenth
            || console.last_stage != Some(event.stage)
            || console.last_message != event.message;
        if !changed {
            return;
        }
        console.last_progress_tenth = progress_tenth;
        console.last_stage = Some(event.stage);
        console.last_message.clone_from(&event.message);

        let width = 30usize;
        let filled = ((event.progress.clamp(0.0, 100.0) / 100.0) * width as f32).round() as usize;
        let bar = format!("{}{}", "#".repeat(filled), "-".repeat(width - filled));
        let line = format!(
            "[{}] {:>6.2}% {:<20} {}",
            bar,
            event.progress,
            stage_name(event.stage),
            event.message.replace(['\r', '\n'], " ")
        );
        if io::stderr().is_terminal() {
            eprint!("\r\x1b[2K{line}");
            let _ = io::stderr().flush();
            console.line_open = true;
            if matches!(
                event.stage,
                PipelineStage::Completed | PipelineStage::Failed | PipelineStage::Cancelled
            ) {
                eprintln!();
                console.line_open = false;
            }
        } else {
            eprintln!("{line}");
        }
    }
}

pub async fn run_managed(
    engines: EnginePaths,
    request: RunRequest,
    foreground: bool,
) -> Result<PipelineResult> {
    let _guard = ActiveGuard::acquire()?;
    let reporter = Arc::new(ProgressReporter::new(&request, foreground)?);
    let event_reporter = Arc::clone(&reporter);
    let runner = Arc::new(PipelineRunner::new(engines, move |event| {
        event_reporter.on_event(event);
    }));
    let pipeline_runner = Arc::clone(&runner);
    let pipeline_request = request.clone();
    let pipeline = async move {
        match pipeline_request {
            RunRequest::Generate {
                input,
                projects_root,
                quality,
            } => {
                pipeline_runner
                    .generate(&input, quality, &projects_root)
                    .await
            }
            RunRequest::Resume { project_id } => pipeline_runner.resume(project_id).await,
        }
    };
    tokio::pin!(pipeline);
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut monitor = tokio::time::interval(Duration::from_millis(250));
    monitor.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut stopping = false;

    loop {
        tokio::select! {
            result = &mut pipeline => {
                reporter.update_context(&runner.failure_context());
                if let Err(error) = &result {
                    runner.emit_terminal(error);
                }
                return result;
            }
            _ = monitor.tick() => {
                reporter.update_context(&runner.failure_context());
            }
            _ = &mut shutdown, if !stopping => {
                stopping = true;
                reporter.mark_stopping();
                runner.cancel();
            }
        }
    }
}

pub async fn spawn_background(
    engine_dir: Option<&Path>,
    request: &RunRequest,
) -> Result<BackgroundLaunch> {
    reject_if_active()?;
    let executable = std::env::current_exe()?;
    let log_path = worker_log_path()?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let mut command = Command::new(executable);
    if let Some(engine_dir) = engine_dir {
        command.arg("--engine-dir").arg(engine_dir);
    }
    match request {
        RunRequest::Generate {
            input,
            projects_root,
            quality,
        } => {
            command
                .arg("worker-generate")
                .arg(input)
                .arg("--projects-root")
                .arg(projects_root)
                .arg("--quality")
                .arg(quality_arg(*quality));
        }
        RunRequest::Resume { project_id } => {
            command.arg("worker-resume").arg(project_id.to_string());
        }
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    wait_for_worker_start(&mut child, &log_path).await
}

async fn wait_for_worker_start(child: &mut Child, log_path: &Path) -> Result<BackgroundLaunch> {
    let pid = child.id();
    let deadline = Instant::now() + STARTUP_WAIT;
    loop {
        let active = match read_active_task() {
            Ok(active) => active,
            Err(error) => {
                stop_failed_worker(child);
                return Err(error);
            }
        };
        if let Some(active) = active {
            if active.pid == pid {
                return Ok(BackgroundLaunch {
                    pid,
                    project_id: active.project_id,
                    log_path: log_path.to_path_buf(),
                    status_path: active_status_path()?,
                });
            }
        }
        if let Some(status) = child.try_wait()? {
            return Err(SplatError::Process(format!(
                "后台任务启动失败（退出码 {status}），日志：{}",
                log_path.display()
            )));
        }
        if Instant::now() >= deadline {
            stop_failed_worker(child);
            return Err(SplatError::Process(format!(
                "后台任务在 {} 秒内没有就绪，日志：{}",
                STARTUP_WAIT.as_secs(),
                log_path.display()
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn stop_failed_worker(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

pub fn read_active_task() -> Result<Option<ActiveTask>> {
    let path = active_status_path()?;
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path)?;
    let active: ActiveTask = serde_json::from_slice(&bytes)?;
    if managed_process_is_alive(active.pid) {
        Ok(Some(active))
    } else {
        cleanup_stale_runtime(active.pid)?;
        Ok(None)
    }
}

pub async fn pause_active() -> Result<Option<ActiveTask>> {
    let Some(active) = read_active_task()? else {
        return Ok(None);
    };
    signal_process(active.pid)?;
    let deadline = Instant::now() + STOP_WAIT;
    while process_is_alive(active.pid) {
        if Instant::now() >= deadline {
            return Err(SplatError::Process(format!(
                "任务进程 {} 在 {} 秒内未能安全停止；未发送强制终止信号",
                active.pid,
                STOP_WAIT.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    cleanup_stale_runtime(active.pid)?;
    Ok(Some(active))
}

pub fn active_status_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("active.json"))
}

pub fn stage_name(stage: PipelineStage) -> &'static str {
    match stage {
        PipelineStage::Created => "created",
        PipelineStage::ProbingVideo => "probe",
        PipelineStage::PlanningFrames => "plan",
        PipelineStage::ExtractingFrames => "frames",
        PipelineStage::ExtractingFeatures => "features",
        PipelineStage::Matching => "matching",
        PipelineStage::Reconstructing => "reconstruct",
        PipelineStage::ValidatingReconstruction => "validate",
        PipelineStage::TrainingSplats => "training",
        PipelineStage::Exporting => "export",
        PipelineStage::Completed => "completed",
        PipelineStage::Failed => "failed",
        PipelineStage::Cancelled => "paused",
    }
}

fn runtime_dir() -> Result<PathBuf> {
    let directory = if let Some(value) = std::env::var_os("OOOSPLAT_RUNTIME_DIR") {
        PathBuf::from(value)
    } else if let Some(value) = std::env::var_os("OOOSPLAT_DATA_DIR") {
        PathBuf::from(value).join("headless")
    } else {
        dirs::data_local_dir()
            .ok_or_else(|| SplatError::Process("无法定位本机应用数据目录".into()))?
            .join("SplatStudio")
            .join("headless")
    };
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn worker_log_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join(format!(
        "worker-{}-{}.log",
        Utc::now().format("%Y%m%d-%H%M%S"),
        Uuid::new_v4().simple()
    )))
}

fn lock_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("active.lock"))
}

fn reject_if_active() -> Result<()> {
    if let Some(active) = read_active_task()? {
        return Err(SplatError::Process(format!(
            "已有任务正在运行（PID {}{}）；请先执行 pause，或使用 switch 切换任务",
            active.pid,
            active
                .project_id
                .map(|id| format!("，任务 {id}"))
                .unwrap_or_default()
        )));
    }
    let lock = lock_path()?;
    if lock.is_file() {
        if let Ok(pid) = fs::read_to_string(&lock).map(|value| value.trim().parse::<u32>()) {
            if pid.is_ok_and(managed_process_is_alive) {
                return Err(SplatError::Process("另一个任务正在启动，请稍后重试".into()));
            }
        }
        let _ = fs::remove_file(lock);
    }
    Ok(())
}

struct ActiveGuard {
    pid: u32,
    lock_path: PathBuf,
    active_path: PathBuf,
    _lock: File,
}

impl ActiveGuard {
    fn acquire() -> Result<Self> {
        reject_if_active()?;
        let lock_path = lock_path()?;
        let active_path = active_status_path()?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(|error| SplatError::Process(format!("无法取得 headless 任务锁（{error}）")))?;
        let pid = std::process::id();
        writeln!(file, "{pid}")?;
        file.sync_all()?;
        Ok(Self {
            pid,
            lock_path,
            active_path,
            _lock: file,
        })
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if active_file_belongs_to(&self.active_path, self.pid) {
            let _ = fs::remove_file(&self.active_path);
        }
        let lock_belongs_to_us = fs::read_to_string(&self.lock_path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            == Some(self.pid);
        if lock_belongs_to_us {
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}

fn active_file_belongs_to(path: &Path, pid: u32) -> bool {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ActiveTask>(&bytes).ok())
        .is_some_and(|active| active.pid == pid)
}

fn cleanup_stale_runtime(pid: u32) -> Result<()> {
    let active = active_status_path()?;
    if active_file_belongs_to(&active, pid) {
        let _ = fs::remove_file(active);
    }
    let lock = lock_path()?;
    let lock_pid = fs::read_to_string(&lock)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    if lock_pid == Some(pid) || lock_pid.is_none() {
        let _ = fs::remove_file(lock);
    }
    Ok(())
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(&temporary, bytes)?;
    if let Err(error) = fs::rename(&temporary, path) {
        if path.exists() {
            fs::remove_file(path)?;
            fs::rename(&temporary, path)?;
        } else {
            return Err(error.into());
        }
    }
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn quality_arg(quality: Quality) -> &'static str {
    match quality {
        Quality::Fast => "fast",
        Quality::Balanced => "balanced",
        Quality::High => "high",
    }
}

fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn managed_process_is_alive(pid: u32) -> bool {
    if !process_is_alive(pid) {
        return false;
    }
    let running_name = fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .map(|name| name.trim_end_matches(" (deleted)").to_owned());
    let current_name = std::env::current_exe().ok().and_then(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    });
    let command_line = fs::read(format!("/proc/{pid}/cmdline")).ok();
    running_name.is_some()
        && running_name == current_name
        && command_line.as_deref().is_some_and(is_managed_command_line)
}

fn is_managed_command_line(command_line: &[u8]) -> bool {
    command_line.split(|byte| *byte == 0).any(|argument| {
        matches!(
            argument,
            b"generate" | b"resume" | b"worker-generate" | b"worker-resume"
        )
    })
}

fn signal_process(pid: u32) -> Result<()> {
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error().into())
    }
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_names_are_stable_for_status_output() {
        assert_eq!(stage_name(PipelineStage::TrainingSplats), "training");
        assert_eq!(stage_name(PipelineStage::Cancelled), "paused");
    }

    #[test]
    fn active_file_ownership_rejects_another_pid() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("active.json");
        let now = Utc::now();
        let active = ActiveTask {
            schema_version: RUNTIME_SCHEMA_VERSION,
            pid: 42,
            action: "generate".into(),
            input: None,
            project_id: None,
            project_path: None,
            logs_directory: None,
            started_at: now,
            updated_at: now,
            stopping: false,
            event: None,
        };
        fs::write(&path, serde_json::to_vec(&active).unwrap()).unwrap();
        assert!(active_file_belongs_to(&path, 42));
        assert!(!active_file_belongs_to(&path, 43));
    }

    #[test]
    fn recognizes_only_pipeline_command_lines() {
        assert!(is_managed_command_line(
            b"/opt/ooosplat/splatstudio\0--engine-dir\0/opt/engines\0worker-generate\0"
        ));
        assert!(is_managed_command_line(
            b"/opt/ooosplat/splatstudio\0generate\0/data/input.mp4\0"
        ));
        assert!(!is_managed_command_line(
            b"/opt/ooosplat/splatstudio\0status\0--watch\0"
        ));
    }
}
