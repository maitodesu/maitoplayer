use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use contracts::{AppErrorV1, DependencyHealthV1, error_codes};
use ports::{MediaToolPort, ToolOutput, ToolProgressCallback, ToolRequest};

#[derive(Clone, Debug)]
pub struct ProcessRunner {
    poll_interval: Duration,
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(20),
        }
    }
}

impl ProcessRunner {
    pub fn run_cancellable(
        &self,
        request: ToolRequest,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<ToolOutput, AppErrorV1> {
        self.run_inner(request, cancelled, None)
    }

    fn run_inner(
        &self,
        request: ToolRequest,
        cancelled: &Arc<AtomicBool>,
        progress: Option<ToolProgressCallback>,
    ) -> Result<ToolOutput, AppErrorV1> {
        if request.args.len() > 256 || request.args.iter().any(|arg| arg.len() > 32_768) {
            return Err(tool_error("Tool arguments exceeded their limit.", false));
        }
        let mut child = Command::new(&request.executable)
            .args(&request.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                let code = if error.kind() == std::io::ErrorKind::NotFound {
                    error_codes::FFMPEG_NOT_FOUND
                } else {
                    error_codes::CONVERSION_FAILED
                };
                AppErrorV1::new(
                    code,
                    "The configured media tool could not be started. Check Media Tool settings.",
                    true,
                )
                .with_diagnostics(error.kind().to_string())
            })?;
        #[cfg(windows)]
        let _job_guard = attach_kill_on_close_job(&mut child)?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| tool_error("Tool stdout was not captured.", true))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| tool_error("Tool stderr was not captured.", true))?;
        let stdout_limit = request.max_stdout_bytes;
        let stderr_limit = request.max_stderr_bytes;
        let stdout_reader =
            thread::spawn(move || read_bounded_with_callback(stdout, stdout_limit, progress));
        let stderr_reader = thread::spawn(move || read_bounded(stderr, stderr_limit));
        let started = Instant::now();
        let status = loop {
            if cancelled.load(Ordering::Acquire) {
                terminate(&mut child);
                break None;
            }
            if started.elapsed() >= request.timeout {
                terminate(&mut child);
                return Err(tool_error("Media tool timed out and was stopped.", true));
            }
            match child.try_wait() {
                Ok(Some(status)) => break status.code(),
                Ok(None) => thread::sleep(self.poll_interval),
                Err(error) => {
                    terminate(&mut child);
                    return Err(tool_error(
                        &format!("Could not wait for media tool: {error}"),
                        true,
                    ));
                }
            }
        };
        let stdout = join_reader(stdout_reader)?;
        let stderr = join_reader(stderr_reader)?;
        if cancelled.load(Ordering::Acquire) {
            return Err(AppErrorV1::new(
                error_codes::CONVERSION_CANCELLED,
                "Media operation was cancelled.",
                true,
            ));
        }
        Ok(ToolOutput {
            status_code: status,
            stdout,
            stderr,
        })
    }
}

impl MediaToolPort for ProcessRunner {
    fn run(&self, request: ToolRequest) -> Result<ToolOutput, AppErrorV1> {
        self.run_cancellable(request, &Arc::new(AtomicBool::new(false)))
    }

    fn run_cancellable(
        &self,
        request: ToolRequest,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<ToolOutput, AppErrorV1> {
        ProcessRunner::run_cancellable(self, request, cancelled)
    }

    fn run_cancellable_with_progress(
        &self,
        request: ToolRequest,
        cancelled: &Arc<AtomicBool>,
        progress: ToolProgressCallback,
    ) -> Result<ToolOutput, AppErrorV1> {
        self.run_inner(request, cancelled, Some(progress))
    }
}

fn read_bounded(reader: impl Read, limit: usize) -> Result<Vec<u8>, std::io::Error> {
    let take_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    reader.take(take_limit).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "tool output limit exceeded",
        ));
    }
    Ok(bytes)
}

fn read_bounded_with_callback(
    mut reader: impl Read,
    limit: usize,
    callback: Option<ToolProgressCallback>,
) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "tool output limit exceeded",
            ));
        }
        if let Some(callback) = &callback {
            callback(&chunk[..count]);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(bytes)
}

fn join_reader(
    handle: thread::JoinHandle<Result<Vec<u8>, std::io::Error>>,
) -> Result<Vec<u8>, AppErrorV1> {
    handle
        .join()
        .map_err(|_| tool_error("Media tool output reader stopped unexpectedly.", true))?
        .map_err(|error| tool_error(&error.to_string(), false))
}

fn terminate(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let process_id = child.id().to_string();
        let _ = Command::new("taskkill.exe")
            .args(["/PID", &process_id, "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(windows)]
fn attach_kill_on_close_job(child: &mut std::process::Child) -> Result<win32job::Job, AppErrorV1> {
    use std::os::windows::io::AsRawHandle;

    let mut limits = win32job::ExtendedLimitInfo::new();
    limits.limit_kill_on_job_close();
    let job = win32job::Job::create_with_limit_info(&limits).map_err(|error| {
        tool_error(
            &format!("Could not create a kill-on-close Windows Job Object: {error}"),
            true,
        )
    })?;
    if let Err(error) = job.assign_process(child.as_raw_handle() as isize) {
        terminate(child);
        return Err(tool_error(
            &format!("Could not contain the media process in a Windows Job Object: {error}"),
            true,
        ));
    }
    Ok(job)
}

fn tool_error(detail: &str, retryable: bool) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::CONVERSION_FAILED,
        "The media tool failed safely. Review media-tool health and retry.",
        retryable,
    )
    .with_diagnostics(detail)
}

#[must_use]
pub fn discover_executable(configured: Option<&Path>, name: &str) -> Option<PathBuf> {
    if let Some(path) = configured
        && path.is_file()
    {
        return Some(path.to_path_buf());
    }
    let path_value = std::env::var_os("PATH")?;
    std::env::split_paths(&path_value)
        .flat_map(|directory| {
            executable_names(name)
                .into_iter()
                .map(move |file| directory.join(file))
        })
        .find(|path| path.is_file())
}

fn executable_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![format!("{name}.exe"), name.to_owned()]
    } else {
        vec![name.to_owned()]
    }
}

#[must_use]
pub fn tool_health(path: Option<&Path>, component: &str) -> DependencyHealthV1 {
    match path {
        Some(path) if path.is_file() => {
            let expected_name = component.to_ascii_lowercase();
            match inspect_tool(path, &expected_name) {
                Ok(version) => DependencyHealthV1 {
                    component: component.into(),
                    available: true,
                    version: Some(version),
                    action: None,
                    error: None,
                },
                Err(error) => DependencyHealthV1 {
                    component: component.into(),
                    available: false,
                    version: None,
                    action: Some("Repair or reinstall the application.".into()),
                    error: Some(error),
                },
            }
        }
        _ => DependencyHealthV1 {
            component: component.into(),
            available: false,
            version: None,
            action: Some("Repair or reinstall the application.".into()),
            error: Some(AppErrorV1::new(
                error_codes::FFMPEG_NOT_FOUND,
                format!("The bundled {component} executable is unavailable."),
                true,
            )),
        },
    }
}

pub fn inspect_tool(path: &Path, expected_name: &str) -> Result<String, AppErrorV1> {
    inspect_tool_with_runner(path, expected_name, &ProcessRunner::default())
}

pub(crate) fn inspect_tool_with_runner(
    path: &Path,
    expected_name: &str,
    runner: &dyn MediaToolPort,
) -> Result<String, AppErrorV1> {
    if !path.is_file()
        || expected_name.is_empty()
        || !expected_name.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return Err(AppErrorV1::new(
            error_codes::FFMPEG_NOT_FOUND,
            "Choose a valid FFmpeg or FFprobe executable.",
            true,
        ));
    }
    let output = runner.run(ToolRequest {
        executable: path.to_path_buf(),
        args: vec!["-version".into()],
        timeout: Duration::from_secs(5),
        max_stdout_bytes: 64 * 1024,
        max_stderr_bytes: 64 * 1024,
    })?;
    if output.status_code != Some(0) {
        return Err(tool_error(
            "The selected tool did not report its version.",
            true,
        ));
    }
    let decoded = String::from_utf8_lossy(&output.stdout);
    let first_line = decoded
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.len() <= 1_024)
        .ok_or_else(|| tool_error("The selected tool returned no bounded version line.", true))?;
    if !first_line
        .to_ascii_lowercase()
        .starts_with(&format!("{expected_name} version"))
    {
        return Err(tool_error(
            "The selected executable did not identify as the expected media tool.",
            false,
        ));
    }
    Ok(first_line.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_tool_must_be_a_file() {
        assert!(
            discover_executable(
                Some(Path::new("definitely-missing-tool")),
                "definitely-missing-tool"
            )
            .is_none()
        );
    }
}
