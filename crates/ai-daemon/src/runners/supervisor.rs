use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::{
    read_frame, write_frame, Envelope, ManifestError, ProtocolError, RunnerDescriptor,
    RunnerManifest, RunnerRegistryError, DEFAULT_MAX_FRAME_BYTES,
};

const MAX_STDERR_CAPTURE_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub enum SupervisorError {
    Manifest(ManifestError),
    Trust(RunnerRegistryError),
    Protocol(ProtocolError),
    Spawn(std::io::Error),
    MissingPipe(&'static str),
    Deadline(&'static str),
    Identity(String),
    UnexpectedMessage { expected: String, actual: String },
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(error) => {
                write!(formatter, "invalid Runner launch configuration: {error}")
            }
            Self::Trust(error) => write!(formatter, "Runner trust verification failed: {error}"),
            Self::Protocol(error) => write!(formatter, "Runner protocol violation: {error}"),
            Self::Spawn(error) => write!(formatter, "failed to spawn Runner: {error}"),
            Self::MissingPipe(pipe) => write!(formatter, "Runner {pipe} pipe is unavailable"),
            Self::Deadline(phase) => write!(formatter, "Runner exceeded {phase} deadline"),
            Self::Identity(message) => {
                write!(formatter, "Runner handshake identity mismatch: {message}")
            }
            Self::UnexpectedMessage { expected, actual } => {
                write!(
                    formatter,
                    "expected Runner message '{expected}', received '{actual}'"
                )
            }
        }
    }
}

impl std::error::Error for SupervisorError {}

impl From<ManifestError> for SupervisorError {
    fn from(error: ManifestError) -> Self {
        Self::Manifest(error)
    }
}

impl From<RunnerRegistryError> for SupervisorError {
    fn from(error: RunnerRegistryError) -> Self {
        Self::Trust(error)
    }
}

impl From<ProtocolError> for SupervisorError {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

pub struct RunnerProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr_task: Option<JoinHandle<Vec<u8>>>,
    package_staging: PathBuf,
    max_frame_bytes: usize,
    shutdown_timeout: Duration,
}

struct StartupGuard {
    child: Option<Child>,
    stderr_task: Option<JoinHandle<Vec<u8>>>,
}

impl StartupGuard {
    fn child_mut(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("startup guard owns the child until handshake succeeds")
    }

    async fn cleanup(mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        if let Some(stderr_task) = self.stderr_task.take() {
            stderr_task.abort();
            let _ = stderr_task.await;
        }
    }
}

impl RunnerProcess {
    pub async fn spawn(
        descriptor: &RunnerDescriptor,
        environment_python: &Path,
        runtime_temp_root: &Path,
    ) -> Result<Self, SupervisorError> {
        let manifest = descriptor
            .manifest
            .as_ref()
            .ok_or_else(|| {
                SupervisorError::Trust(RunnerRegistryError::PackageStaging {
                    root: descriptor.root.clone(),
                    reason: "trusted descriptor has no manifest".to_string(),
                })
            })?
            .clone();
        let package_staging = descriptor.stage_for_execution(runtime_temp_root)?;
        match Self::spawn_staged(
            &manifest,
            environment_python,
            runtime_temp_root,
            package_staging.clone(),
        )
        .await
        {
            Ok(process) => Ok(process),
            Err(error) => {
                let _ = std::fs::remove_dir_all(&package_staging);
                Err(error)
            }
        }
    }

    async fn spawn_staged(
        manifest: &RunnerManifest,
        environment_python: &Path,
        runtime_temp_root: &Path,
        package_staging: PathBuf,
    ) -> Result<Self, SupervisorError> {
        let command =
            manifest.resolve_command(&package_staging, environment_python, runtime_temp_root)?;
        let (program, arguments) = command
            .split_first()
            .expect("validated Runner command is non-empty");
        let mut process = Command::new(program);
        process
            .args(arguments)
            .current_dir(match manifest.entrypoint.working_directory.as_str() {
                "package" => &package_staging,
                "runtime" => runtime_temp_root,
                _ => unreachable!("manifest validation restricts the working directory"),
            })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env_clear();
        for name in &manifest.security.inherit_environment {
            if let Some(value) = std::env::var_os(name) {
                process.env(name, value);
            }
        }
        let child = match process.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&package_staging);
                return Err(SupervisorError::Spawn(error));
            }
        };
        let mut startup = StartupGuard {
            child: Some(child),
            stderr_task: None,
        };
        let result = async {
            let stdin = startup
                .child_mut()
                .stdin
                .take()
                .ok_or(SupervisorError::MissingPipe("stdin"))?;
            let mut stdout = startup
                .child_mut()
                .stdout
                .take()
                .ok_or(SupervisorError::MissingPipe("stdout"))?;
            let mut stderr = startup
                .child_mut()
                .stderr
                .take()
                .ok_or(SupervisorError::MissingPipe("stderr"))?;
            startup.stderr_task = Some(tokio::spawn(async move {
                let mut captured = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let read = match stderr.read(&mut chunk).await {
                        Ok(read) => read,
                        Err(_) => break,
                    };
                    if read == 0 {
                        break;
                    }
                    let remaining = MAX_STDERR_CAPTURE_BYTES.saturating_sub(captured.len());
                    captured.extend_from_slice(&chunk[..read.min(remaining)]);
                }
                captured
            }));
            let boot_timeout = Duration::from_secs(manifest.timeouts.boot_seconds);
            let hello = timeout(
                boot_timeout,
                read_frame(&mut stdout, DEFAULT_MAX_FRAME_BYTES),
            )
            .await
            .map_err(|_| SupervisorError::Deadline("boot"))??;
            validate_hello(manifest, &hello)?;
            Ok((stdin, stdout))
        }
        .await;
        match result {
            Ok((stdin, stdout)) => Ok(Self {
                child: startup
                    .child
                    .take()
                    .expect("startup child remains after hello"),
                stdin,
                stdout,
                stderr_task: startup.stderr_task.take(),
                package_staging,
                max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
                shutdown_timeout: Duration::from_secs(manifest.timeouts.shutdown_seconds),
            }),
            Err(error) => {
                startup.cleanup().await;
                let _ = std::fs::remove_dir_all(&package_staging);
                Err(error)
            }
        }
    }

    pub async fn send(&mut self, envelope: &Envelope) -> Result<(), SupervisorError> {
        write_frame(&mut self.stdin, envelope).await?;
        Ok(())
    }

    pub async fn receive(&mut self, deadline: Duration) -> Result<Envelope, SupervisorError> {
        timeout(deadline, read_frame(&mut self.stdout, self.max_frame_bytes))
            .await
            .map_err(|_| SupervisorError::Deadline("response"))?
            .map_err(Into::into)
    }

    pub async fn request(
        &mut self,
        message_type: &str,
        id: &str,
        payload: Value,
        expected_reply: &str,
        deadline: Duration,
    ) -> Result<Envelope, SupervisorError> {
        self.send(&Envelope::new(message_type, id, payload)).await?;
        let reply = self.receive(deadline).await?;
        if reply.id != id || reply.message_type != expected_reply {
            return Err(SupervisorError::UnexpectedMessage {
                expected: format!("{expected_reply} for {id}"),
                actual: format!("{} for {}", reply.message_type, reply.id),
            });
        }
        Ok(reply)
    }

    pub async fn initialize(
        &mut self,
        runner_id: &str,
        temp_root: &Path,
    ) -> Result<(), SupervisorError> {
        self.request(
            "initialize",
            "startup",
            json!({"runner_id": runner_id, "temp_root": temp_root, "network": false}),
            "initialized",
            self.shutdown_timeout,
        )
        .await?;
        Ok(())
    }

    pub async fn shutdown(mut self) -> Result<String, SupervisorError> {
        let reply = self
            .request(
                "shutdown",
                "shutdown",
                json!({}),
                "shutdown_complete",
                self.shutdown_timeout,
            )
            .await?;
        let _ = reply;
        let _ = self.stdin.shutdown().await;
        match timeout(self.shutdown_timeout, self.child.wait()).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => return Err(SupervisorError::Spawn(error)),
            Err(_) => {
                let _ = self.child.start_kill();
                let _ = self.child.wait().await;
                return Err(SupervisorError::Deadline("shutdown"));
            }
        }
        let stderr = self
            .stderr_task
            .take()
            .expect("stderr task remains owned until shutdown")
            .await
            .unwrap_or_default();
        let _ = std::fs::remove_dir_all(&self.package_staging);
        Ok(String::from_utf8_lossy(&stderr).into_owned())
    }
}

impl Drop for RunnerProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        if let Some(stderr_task) = &self.stderr_task {
            stderr_task.abort();
        }
    }
}

fn validate_hello(manifest: &RunnerManifest, hello: &Envelope) -> Result<(), SupervisorError> {
    if hello.message_type != "hello" || hello.id != "startup" {
        return Err(SupervisorError::UnexpectedMessage {
            expected: "hello for startup".to_string(),
            actual: format!("{} for {}", hello.message_type, hello.id),
        });
    }
    let runner_id = hello
        .payload
        .get("runner_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let runner_version = hello
        .payload
        .get("runner_version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if runner_id != manifest.id || runner_version != manifest.version {
        return Err(SupervisorError::Identity(format!(
            "expected {}@{}, received {}@{}",
            manifest.id, manifest.version, runner_id, runner_version
        )));
    }
    let protocols = hello
        .payload
        .get("protocol_versions")
        .and_then(Value::as_array)
        .ok_or_else(|| SupervisorError::Identity("hello omitted protocol_versions".to_string()))?;
    if !protocols
        .iter()
        .any(|value| value.as_str() == Some("macai.runner.v1"))
    {
        return Err(SupervisorError::Identity(
            "hello does not support macai.runner.v1".to_string(),
        ));
    }
    Ok(())
}
