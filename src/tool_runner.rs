//! Constrained external-tool execution.
//!
//! Programs and arguments are passed directly to `execve` through Tokio. No shell is involved,
//! output is retained within a fixed budget, and timeout/cancellation are explicit results.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitStatus,
    time::{Duration, Instant},
};

use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct ToolCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_dir: Option<PathBuf>,
    pub environment: BTreeMap<OsString, OsString>,
    pub stdin: Option<Vec<u8>>,
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
            environment: BTreeMap::new(),
            stdin: None,
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

    /// Provide a fixed, bounded input transcript to a tool without invoking a shell.
    #[must_use]
    pub fn stdin_bytes(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    #[must_use]
    pub const fn retained_output_bytes(mut self, bytes: usize) -> Self {
        self.retained_output_bytes = bytes;
        self
    }

    /// Add an explicit environment value without involving a shell. Values are
    /// intentionally omitted from durable tool output and error messages.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.insert(key.into(), value.into());
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
    Timeout {
        program: PathBuf,
        timeout: Duration,
        stdout: String,
        stderr: String,
        output_truncated: bool,
        elapsed_ms: u128,
    },
    #[error("tool was cancelled: {program}")]
    Cancelled {
        program: PathBuf,
        stdout: String,
        stderr: String,
        output_truncated: bool,
        elapsed_ms: u128,
    },
    #[error("tool failed with exit code {code}: {program}")]
    Failed {
        program: PathBuf,
        code: i32,
        stdout: String,
        stderr: String,
        output_truncated: bool,
        elapsed_ms: u128,
    },
    #[error("could not execute {program}: {source}")]
    Io {
        program: PathBuf,
        source: std::io::Error,
    },
}

impl ToolError {
    /// Return a compact diagnostic suitable for durable status and logs.
    ///
    /// The summary includes only the executable path and tool-emitted output;
    /// command arguments, stdin, and environment values are never included.
    /// `max_output_chars` bounds the selected stderr/stdout excerpt, while the
    /// error classification remains intact even when the excerpt is empty.
    #[must_use]
    pub fn diagnostic_summary(&self, max_output_chars: usize) -> String {
        let (summary, stdout, stderr, output_truncated) = match self {
            Self::Timeout {
                program,
                timeout,
                stdout,
                stderr,
                output_truncated,
                ..
            } => (
                format!("tool timed out after {timeout:?}: {}", program.display()),
                stdout.as_str(),
                stderr.as_str(),
                *output_truncated,
            ),
            Self::Cancelled {
                program,
                stdout,
                stderr,
                output_truncated,
                ..
            } => (
                format!("tool was cancelled: {}", program.display()),
                stdout.as_str(),
                stderr.as_str(),
                *output_truncated,
            ),
            Self::Failed {
                program,
                code,
                stdout,
                stderr,
                output_truncated,
                ..
            } => (
                format!("tool failed with exit code {code}: {}", program.display()),
                stdout.as_str(),
                stderr.as_str(),
                *output_truncated,
            ),
            Self::Unavailable { .. } | Self::Io { .. } => {
                return collapse_diagnostic_text(&self.to_string());
            }
        };

        let summary = collapse_diagnostic_text(&summary);
        let stderr = collapse_diagnostic_text(stderr);
        let (stream, output) = if stderr.is_empty() {
            ("stdout", collapse_diagnostic_text(stdout))
        } else {
            ("stderr", stderr)
        };
        if output.is_empty() {
            return if output_truncated {
                format!("{summary}; tool output was truncated")
            } else {
                summary
            };
        }

        let (excerpt, excerpt_truncated) = truncate_chars(&output, max_output_chars);
        let truncation = output_truncated || excerpt_truncated;
        if excerpt.is_empty() {
            format!("{summary}; {stream}: [truncated]")
        } else if truncation {
            format!("{summary}; {stream}: {excerpt} [truncated]")
        } else {
            format!("{summary}; {stream}: {excerpt}")
        }
    }
}

fn collapse_diagnostic_text(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut separator_pending = false;
    for character in value.chars() {
        if character.is_control() || character.is_whitespace() {
            separator_pending = !collapsed.is_empty();
            continue;
        }
        if separator_pending {
            collapsed.push(' ');
            separator_pending = false;
        }
        collapsed.push(character);
    }
    collapsed
}

fn truncate_chars(value: &str, limit: usize) -> (String, bool) {
    let mut characters = value.chars();
    let excerpt = characters.by_ref().take(limit).collect();
    (excerpt, characters.next().is_some())
}

pub async fn run(
    command: &ToolCommand,
    cancellation: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    validate_program(&command.program)?;
    if cancellation.is_cancelled() {
        return Err(ToolError::Cancelled {
            program: command.program.clone(),
            stdout: String::new(),
            stderr: String::new(),
            output_truncated: false,
            elapsed_ms: 0,
        });
    }
    let started = Instant::now();
    let mut child = Command::new(&command.program);
    child
        .args(&command.args)
        .envs(&command.environment)
        .kill_on_drop(true)
        .stdin(if command.stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
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
    let stdin_writer = command.stdin.as_ref().map(|input| {
        let mut stdin = child.stdin.take().expect("stdin was configured as piped");
        let input = input.clone();
        tokio::spawn(async move {
            stdin.write_all(&input).await?;
            stdin.shutdown().await
        })
    });
    let limit = command.retained_output_bytes;
    let mut stdout_reader = tokio::spawn(read_bounded(stdout, limit));
    let mut stderr_reader = tokio::spawn(read_bounded(stderr, limit));

    let completion = tokio::select! {
        result = child.wait() => WaitCompletion::Exited(result.map_err(|source| ToolError::Io {
            program: command.program.clone(), source
        })?),
        () = tokio::time::sleep(command.timeout) => {
            let _ = child.start_kill();
            if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_err() {
                stdout_reader.abort();
                stderr_reader.abort();
                if let Some(writer) = &stdin_writer {
                    writer.abort();
                }
                return Err(ToolError::Timeout {
                    program: command.program.clone(),
                    timeout: command.timeout,
                    stdout: String::new(),
                    stderr: String::new(),
                    output_truncated: true,
                    elapsed_ms: started.elapsed().as_millis()
                });
            }
            WaitCompletion::TimedOut
        },
        () = cancellation.cancelled() => {
            let _ = child.start_kill();
            if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_err() {
                stdout_reader.abort();
                stderr_reader.abort();
                if let Some(writer) = &stdin_writer {
                    writer.abort();
                }
                return Err(ToolError::Cancelled {
                    program: command.program.clone(),
                    stdout: String::new(),
                    stderr: String::new(),
                    output_truncated: true,
                    elapsed_ms: started.elapsed().as_millis()
                });
            }
            WaitCompletion::Cancelled
        }
    };

    if let Some(writer) = stdin_writer {
        match writer.await {
            Ok(Ok(())) => {}
            Ok(Err(source)) if source.kind() == std::io::ErrorKind::BrokenPipe => {}
            Ok(Err(source)) => {
                return Err(ToolError::Io {
                    program: command.program.clone(),
                    source,
                });
            }
            Err(source) => {
                return Err(ToolError::Io {
                    program: command.program.clone(),
                    source: std::io::Error::other(source),
                });
            }
        }
    }

    let (stdout, stdout_truncated) =
        finish_reader(&mut stdout_reader)
            .await
            .map_err(|source| ToolError::Io {
                program: command.program.clone(),
                source,
            })?;
    let (stderr, stderr_truncated) =
        finish_reader(&mut stderr_reader)
            .await
            .map_err(|source| ToolError::Io {
                program: command.program.clone(),
                source,
            })?;

    let output = ToolOutput {
        exit_code: match completion {
            WaitCompletion::Exited(status) => status.code().unwrap_or(-1),
            WaitCompletion::TimedOut | WaitCompletion::Cancelled => -1,
        },
        stdout,
        stderr,
        output_truncated: stdout_truncated || stderr_truncated,
        elapsed_ms: started.elapsed().as_millis(),
    };
    match completion {
        WaitCompletion::Exited(status) if status.success() => Ok(output),
        WaitCompletion::Exited(_) => Err(ToolError::Failed {
            program: command.program.clone(),
            code: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
            output_truncated: output.output_truncated,
            elapsed_ms: output.elapsed_ms,
        }),
        WaitCompletion::TimedOut => Err(ToolError::Timeout {
            program: command.program.clone(),
            timeout: command.timeout,
            stdout: output.stdout,
            stderr: output.stderr,
            output_truncated: output.output_truncated,
            elapsed_ms: output.elapsed_ms,
        }),
        WaitCompletion::Cancelled => Err(ToolError::Cancelled {
            program: command.program.clone(),
            stdout: output.stdout,
            stderr: output.stderr,
            output_truncated: output.output_truncated,
            elapsed_ms: output.elapsed_ms,
        }),
    }
}

#[derive(Clone, Copy, Debug)]
enum WaitCompletion {
    Exited(ExitStatus),
    TimedOut,
    Cancelled,
}

async fn finish_reader(
    reader: &mut tokio::task::JoinHandle<std::io::Result<(String, bool)>>,
) -> std::io::Result<(String, bool)> {
    match tokio::time::timeout(Duration::from_secs(5), &mut *reader).await {
        Ok(result) => result.map_err(std::io::Error::other)?,
        Err(_) => {
            reader.abort();
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "tool output pipe remained open after the child exited",
            ))
        }
    }
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
    async fn fixed_stdin_is_delivered_without_a_shell() {
        let command = ToolCommand::new("/usr/bin/tee").stdin_bytes(b"test\n".to_vec());
        let output = run(&command, &CancellationToken::new())
            .await
            .expect("tee succeeds");
        assert_eq!(output.stdout, "test\n");
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
    async fn timeout_retains_bounded_partial_output() {
        let command = ToolCommand::new("/usr/bin/yes")
            .timeout(Duration::from_millis(20))
            .retained_output_bytes(1_024);
        match run(&command, &CancellationToken::new()).await {
            Err(ToolError::Timeout {
                stdout,
                output_truncated,
                ..
            }) => {
                assert!(!stdout.is_empty());
                assert!(stdout.len() <= 1_024);
                assert!(output_truncated);
            }
            other => panic!("expected timeout with partial output, got {other:?}"),
        }
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

    #[test]
    fn diagnostic_summary_prefers_and_sanitizes_stderr() {
        let error = ToolError::Failed {
            program: PathBuf::from("/usr/bin/psql"),
            code: 3,
            stdout: "stdout should not be selected".to_owned(),
            stderr: "\nERROR:\tbad\0relation\r\n".to_owned(),
            output_truncated: false,
            elapsed_ms: 12,
        };

        assert_eq!(
            error.diagnostic_summary(512),
            "tool failed with exit code 3: /usr/bin/psql; stderr: ERROR: bad relation"
        );
    }

    #[test]
    fn diagnostic_summary_falls_back_to_bounded_stdout() {
        let error = ToolError::Failed {
            program: PathBuf::from("/usr/bin/tool"),
            code: 9,
            stdout: "abcdefgh".to_owned(),
            stderr: " \n\t".to_owned(),
            output_truncated: false,
            elapsed_ms: 4,
        };

        assert_eq!(
            error.diagnostic_summary(4),
            "tool failed with exit code 9: /usr/bin/tool; stdout: abcd [truncated]"
        );
    }

    #[test]
    fn diagnostic_summary_reports_upstream_truncation() {
        let error = ToolError::Timeout {
            program: PathBuf::from("/usr/bin/tool"),
            timeout: Duration::from_secs(30),
            stdout: String::new(),
            stderr: "last retained error".to_owned(),
            output_truncated: true,
            elapsed_ms: 30_000,
        };

        assert_eq!(
            error.diagnostic_summary(512),
            "tool timed out after 30s: /usr/bin/tool; stderr: last retained error [truncated]"
        );
    }

    #[test]
    fn diagnostic_summary_preserves_cancelled_status_without_output() {
        let error = ToolError::Cancelled {
            program: PathBuf::from("/usr/bin/tool"),
            stdout: String::new(),
            stderr: String::new(),
            output_truncated: false,
            elapsed_ms: 7,
        };

        assert_eq!(
            error.diagnostic_summary(512),
            "tool was cancelled: /usr/bin/tool"
        );
    }

    #[tokio::test]
    async fn diagnostic_summary_omits_arguments_and_environment() {
        let command = ToolCommand::new("/bin/sh")
            .args(["-c", "exit 7", "argument-secret"])
            .env("TOOL_SECRET", "environment-secret");
        let error = run(&command, &CancellationToken::new())
            .await
            .expect_err("command must fail");
        let summary = error.diagnostic_summary(512);

        assert_eq!(summary, "tool failed with exit code 7: /bin/sh");
        assert!(!summary.contains("argument-secret"));
        assert!(!summary.contains("environment-secret"));
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
