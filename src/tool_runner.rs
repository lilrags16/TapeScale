//! Constrained external-tool execution.
//!
//! Programs and arguments are passed directly to `execve` through Tokio. No shell is involved,
//! output is retained within a fixed budget, and timeout/cancellation are explicit results.

use std::{
    path::{Path, PathBuf},
    process::ExitStatus,
    time::{Duration, Instant},
};

use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct ToolCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_dir: Option<PathBuf>,
    pub timeout: Duration,
    pub retained_output_bytes: usize,
}

impl ToolCommand {
    #[must_use]
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_dir: None,
            timeout: Duration::from_secs(30),
            retained_output_bytes: 64 * 1024,
        }
    }

    #[must_use]
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
    pub elapsed_ms: u128,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool is unavailable: {program}")]
    Unavailable { program: PathBuf },
    #[error("tool timed out after {timeout:?}: {program}")]
    Timeout { program: PathBuf, timeout: Duration },
    #[error("tool was cancelled: {program}")]
    Cancelled { program: PathBuf },
    #[error("tool failed with exit code {code}: {program}")]
    Failed {
        program: PathBuf,
        code: i32,
        stderr: String,
    },
    #[error("could not execute {program}: {source}")]
    Io {
        program: PathBuf,
        source: std::io::Error,
    },
}

pub async fn run(
    command: &ToolCommand,
    cancellation: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    validate_program(&command.program)?;
    if cancellation.is_cancelled() {
        return Err(ToolError::Cancelled {
            program: command.program.clone(),
        });
    }
    let started = Instant::now();
    let mut child = Command::new(&command.program);
    child
        .args(&command.args)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(working_dir) = &command.working_dir {
        child.current_dir(working_dir);
    }

    let mut child = child
        .spawn()
        .map_err(|source| map_spawn_error(&command.program, source))?;
    let stdout = child.stdout.take().expect("stdout was configured as piped");
    let stderr = child.stderr.take().expect("stderr was configured as piped");
    let limit = command.retained_output_bytes;
    let stdout_reader = tokio::spawn(read_bounded(stdout, limit));
    let stderr_reader = tokio::spawn(read_bounded(stderr, limit));

    let status = tokio::select! {
        result = child.wait() => result.map_err(|source| ToolError::Io {
            program: command.program.clone(), source
        })?,
        () = tokio::time::sleep(command.timeout) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            stdout_reader.abort();
            stderr_reader.abort();
            return Err(ToolError::Timeout {
                program: command.program.clone(), timeout: command.timeout
            });
        },
        () = cancellation.cancelled() => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            stdout_reader.abort();
            stderr_reader.abort();
            return Err(ToolError::Cancelled { program: command.program.clone() });
        }
    };

    let (stdout, stdout_truncated) = stdout_reader
        .await
        .map_err(|source| ToolError::Io {
            program: command.program.clone(),
            source: std::io::Error::other(source),
        })?
        .map_err(|source| ToolError::Io {
            program: command.program.clone(),
            source,
        })?;
    let (stderr, stderr_truncated) = stderr_reader
        .await
        .map_err(|source| ToolError::Io {
            program: command.program.clone(),
            source: std::io::Error::other(source),
        })?
        .map_err(|source| ToolError::Io {
            program: command.program.clone(),
            source,
        })?;

    ensure_success(&command.program, status, &stderr)?;
    Ok(ToolOutput {
        exit_code: status.code().unwrap_or(-1),
        stdout,
        stderr,
        output_truncated: stdout_truncated || stderr_truncated,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

fn validate_program(program: &Path) -> Result<(), ToolError> {
    if program.as_os_str().is_empty() {
        return Err(ToolError::Unavailable {
            program: program.to_owned(),
        });
    }
    Ok(())
}

fn map_spawn_error(program: &Path, source: std::io::Error) -> ToolError {
    if source.kind() == std::io::ErrorKind::NotFound {
        ToolError::Unavailable {
            program: program.to_owned(),
        }
    } else {
        ToolError::Io {
            program: program.to_owned(),
            source,
        }
    }
}

fn ensure_success(program: &Path, status: ExitStatus, stderr: &str) -> Result<(), ToolError> {
    if status.success() {
        return Ok(());
    }
    Err(ToolError::Failed {
        program: program.to_owned(),
        code: status.code().unwrap_or(-1),
        stderr: stderr.to_owned(),
    })
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(String, bool)> {
    let mut retained = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let available = limit.saturating_sub(retained.len());
        let keep = available.min(count);
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    Ok((String::from_utf8_lossy(&retained).into_owned(), truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn arguments_are_not_interpreted_by_a_shell() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let marker_path = temporary.path().join("shell-marker");
        let payload = format!("$(touch {})", marker_path.display());
        let command = ToolCommand::new("/usr/bin/printf").args(["%s", payload.as_str()]);
        let output = run(&command, &CancellationToken::new())
            .await
            .expect("printf succeeds");
        assert_eq!(output.stdout, payload);
        assert!(!marker_path.exists());
    }

    #[tokio::test]
    async fn nonzero_exit_is_a_failure() {
        let command = ToolCommand::new("/usr/bin/false");
        assert!(matches!(
            run(&command, &CancellationToken::new()).await,
            Err(ToolError::Failed { code: 1, .. })
        ));
    }

    #[tokio::test]
    async fn hung_tool_is_killed_at_deadline() {
        let command = ToolCommand::new("/usr/bin/sleep")
            .args(["5"])
            .timeout(Duration::from_millis(20));
        assert!(matches!(
            run(&command, &CancellationToken::new()).await,
            Err(ToolError::Timeout { .. })
        ));
    }

    #[tokio::test]
    async fn pre_cancelled_tool_never_starts() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let marker = temporary.path().join("cancelled-command-marker");
        let command = ToolCommand::new("/usr/bin/touch").args([marker.display().to_string()]);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            run(&command, &cancellation).await,
            Err(ToolError::Cancelled { .. })
        ));
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn missing_tool_is_actionable() {
        let command = ToolCommand::new("/definitely/missing/tapescale-tool");
        assert!(matches!(
            run(&command, &CancellationToken::new()).await,
            Err(ToolError::Unavailable { .. })
        ));
    }
}
