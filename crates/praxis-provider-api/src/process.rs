//! Private-stdio worker ownership. Registry trust and executable resolution must
//! be checked by the host before calling this module. No inherited credentials.
use crate::{
    handshake::{self, WorkerIdentity},
    WireCapabilities, WireLimits,
};
use std::{collections::BTreeMap, path::PathBuf, process::Stdio, time::Duration};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio_util::sync::CancellationToken;

pub struct WorkerLaunch {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    /// Explicit non-secret process environment. Secrets require a separate grant protocol.
    pub environment: BTreeMap<String, String>,
    pub identity: WorkerIdentity,
    pub capabilities: WireCapabilities,
    pub limits: WireLimits,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("invalid worker launch configuration")]
    InvalidLaunch,
    #[error("worker process operation failed")]
    Io,
    #[error("worker readiness failed")]
    Readiness,
    #[error("worker launch cancelled")]
    Cancelled,
}

pub struct WorkerProcess {
    identity: WorkerIdentity,
    child: OwnedChild,
    pub input: ChildStdin,
    pub output: ChildStdout,
}

// Keep group ownership alive across readiness awaits and future drops. On Unix
// this contains ordinary descendants, not helpers that deliberately escape the
// group. Other platforms currently retain direct-child cleanup only.
struct OwnedChild {
    child: Child,
    #[cfg(unix)]
    group: Option<i32>,
}

impl OwnedChild {
    fn kill_group(&mut self) -> Result<(), ProcessError> {
        #[cfg(unix)]
        if let Some(group) = self.group {
            // SAFETY: group is the positive PID of our child, launched into a
            // fresh process group. Negative PID targets that group, never ours.
            let result = unsafe { libc::kill(-group, libc::SIGKILL) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                return Err(ProcessError::Io);
            }
            self.group = None;
        }
        Ok(())
    }
}

impl std::ops::Deref for OwnedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}
impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.kill_group();
    }
}

impl WorkerLaunch {
    pub fn validate(&self) -> Result<(), ProcessError> {
        self.identity
            .validate()
            .map_err(|_| ProcessError::InvalidLaunch)?;
        self.limits
            .validate()
            .map_err(|_| ProcessError::InvalidLaunch)?;
        if !self.executable.is_absolute()
            || self.arguments.len() > 64
            || self
                .arguments
                .iter()
                .any(|arg| arg.len() > 4096 || arg.contains('\0'))
            || self.environment.iter().any(|(key, value)| {
                !matches!(key.as_str(), "PATH" | "LANG" | "LC_ALL" | "TZ")
                    || value.len() > 4096
                    || value.contains('\0')
            })
        {
            return Err(ProcessError::InvalidLaunch);
        }
        Ok(())
    }

    pub async fn spawn(self) -> Result<WorkerProcess, ProcessError> {
        self.spawn_cancellable(&CancellationToken::new()).await
    }

    /// Cancellation before launch spawns nothing; during readiness it forces
    /// termination and bounded reaping. This does not provide graceful shutdown.
    pub async fn spawn_cancellable(
        self,
        cancel: &CancellationToken,
    ) -> Result<WorkerProcess, ProcessError> {
        self.validate()?;
        if cancel.is_cancelled() {
            return Err(ProcessError::Cancelled);
        }
        let mut command = Command::new(&self.executable);
        #[cfg(unix)]
        command.process_group(0);
        let child = command
            .args(&self.arguments)
            .env_clear()
            .envs(&self.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Never forward potentially sensitive worker diagnostics to host logs.
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| ProcessError::Io)?;
        #[cfg(unix)]
        let group = child.id().and_then(|id| i32::try_from(id).ok());
        let mut child = OwnedChild {
            child,
            #[cfg(unix)]
            group,
        };
        let mut input = child.stdin.take().ok_or(ProcessError::Io)?;
        let mut output = child.stdout.take().ok_or(ProcessError::Io)?;
        let readiness = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProcessError::Cancelled),
            result = handshake::negotiate(
                &mut output, &mut input, self.identity.clone(), self.capabilities, self.limits,
            ) => result.map_err(|_| ProcessError::Readiness),
        };
        if let Err(error) = readiness {
            let _ = child.kill_group();
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            return Err(error);
        }
        Ok(WorkerProcess {
            identity: self.identity,
            child,
            input,
            output,
        })
    }
}

impl WorkerProcess {
    pub(crate) fn matches(&self, context: &crate::RequestContext) -> bool {
        self.identity.matches(context)
    }

    /// Forced termination after the host's graceful protocol budget expires.
    pub async fn terminate(&mut self) -> Result<(), ProcessError> {
        let group_result = self.child.kill_group();
        self.child.start_kill().map_err(|_| ProcessError::Io)?;
        tokio::time::timeout(Duration::from_secs(2), self.child.wait())
            .await
            .map_err(|_| ProcessError::Io)?
            .map_err(|_| ProcessError::Io)?;
        group_result
    }

    pub fn exited(&mut self) -> Result<bool, ProcessError> {
        self.child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|_| ProcessError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn launch() -> WorkerLaunch {
        WorkerLaunch {
            executable: PathBuf::from("/nonexistent-worker"),
            arguments: vec![],
            environment: BTreeMap::new(),
            identity: WorkerIdentity {
                provider: "test".into(),
                generation: 1,
                nonce: [1; 16],
            },
            capabilities: WireCapabilities {
                supports_output_limit: false,
                supports_list_models: false,
                supports_embeddings: false,
            },
            limits: WireLimits::default(),
        }
    }
    #[tokio::test]
    async fn pre_cancelled_launch_never_looks_up_executable() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            launch().spawn_cancellable(&cancel).await,
            Err(ProcessError::Cancelled)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_during_readiness_is_bounded() {
        let mut config = launch();
        config.executable = PathBuf::from("/bin/sh");
        config.arguments = vec!["-c".into(), "exec sleep 30".into()];
        let cancel = CancellationToken::new();
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            let cancellation = async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                cancel.cancel();
            };
            let (result, ()) = tokio::join!(config.spawn_cancellable(&cancel), cancellation);
            result
        })
        .await
        .unwrap();
        assert!(matches!(result, Err(ProcessError::Cancelled)));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn readiness_cancellation_stops_forked_helper() {
        let marker = std::env::temp_dir().join(format!(
            "praxis-helper-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut config = launch();
        config.executable = PathBuf::from("/bin/sh");
        config.arguments = vec![
            "-c".into(),
            "sleep 30 & echo $! > \"$1\"; wait".into(),
            "fixture".into(),
            marker.to_string_lossy().into_owned(),
        ];
        let cancel = CancellationToken::new();
        let cancellation = async {
            let stop = tokio::time::Instant::now() + Duration::from_secs(2);
            let pid = loop {
                if let Ok(text) = std::fs::read_to_string(&marker) {
                    if let Ok(pid) = text.trim().parse::<i32>() {
                        break pid;
                    }
                }
                if tokio::time::Instant::now() >= stop {
                    cancel.cancel();
                    panic!("helper did not start");
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            cancel.cancel();
            pid
        };
        let (result, pid) = tokio::join!(config.spawn_cancellable(&cancel), cancellation);
        assert!(matches!(result, Err(ProcessError::Cancelled)));
        tokio::time::sleep(Duration::from_millis(30)).await;
        let running = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat
                .rsplit_once(')')
                .unwrap()
                .1
                .trim_start()
                .starts_with('Z')
        });
        // Clean up the helper even when exercising the unfixed implementation.
        if running {
            // SAFETY: the PID was supplied by our test shell for its own helper.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        let _ = std::fs::remove_file(marker);
        assert!(!running, "forked helper survived readiness cancellation");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn ready_worker_termination_and_drop_stop_helpers() {
        for explicit in [true, false] {
            let marker = std::env::temp_dir().join(format!(
                "praxis-ready-helper-{}-{}-{explicit}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let mut config = launch();
            config.executable = PathBuf::from("/usr/bin/python3");
            config.arguments = vec![
                "-c".into(),
                r#"
import sys, json, struct, subprocess, time
n = struct.unpack('>I', sys.stdin.buffer.read(4))[0]
hello = json.loads(sys.stdin.buffer.read(n))
helper = subprocess.Popen(['/bin/sleep', '30'])
with open(sys.argv[1], 'w') as marker: marker.write(str(helper.pid))
hello['type'] = 'ready'
body = json.dumps(hello).encode()
sys.stdout.buffer.write(struct.pack('>I', len(body)) + body)
sys.stdout.buffer.flush()
time.sleep(30)
"#
                .into(),
                marker.to_string_lossy().into_owned(),
            ];
            let mut worker = config.spawn().await.unwrap();
            let parent = worker.child.id().unwrap();
            let pid: i32 = std::fs::read_to_string(&marker).unwrap().parse().unwrap();
            if explicit {
                worker.terminate().await.unwrap();
            }
            drop(worker);
            tokio::time::sleep(Duration::from_millis(50)).await;
            let running = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                !stat
                    .rsplit_once(')')
                    .unwrap()
                    .1
                    .trim_start()
                    .starts_with('Z')
            });
            if running {
                // SAFETY: this PID was supplied by our test worker for its own helper.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
            let _ = std::fs::remove_file(marker);
            assert!(!running, "helper survived explicit termination={explicit}");
            assert!(
                !PathBuf::from(format!("/proc/{parent}")).exists(),
                "direct worker was not reaped"
            );
        }
    }

    #[tokio::test]
    async fn invalid_identity_and_limits_fail_before_executable_lookup() {
        // The executable does not exist. Io would prove we tried to spawn it.
        for case in 0..5 {
            let mut config = launch();
            match case {
                0 => config.identity.provider.clear(),
                1 => config.identity.provider = "bad/provider".into(),
                2 => config.identity.provider = "x".repeat(65),
                3 => config.identity.nonce = [0; 16],
                _ => config.limits.frame_bytes = 0,
            }
            assert!(matches!(
                config.spawn().await,
                Err(ProcessError::InvalidLaunch)
            ));
        }
    }

    #[test]
    fn launch_rejects_relative_paths_and_credential_environment() {
        let mut config = launch();
        config.executable = "worker".into();
        assert!(matches!(
            config.validate(),
            Err(ProcessError::InvalidLaunch)
        ));
        config.executable = "/worker".into();
        config
            .environment
            .insert("OPENAI_API_KEY".into(), "secret".into());
        assert!(matches!(
            config.validate(),
            Err(ProcessError::InvalidLaunch)
        ));
    }
    // External interpreter fixture exercises real OS pipes, not duplex mocks.
    #[cfg(unix)]
    #[tokio::test]
    async fn real_process_readiness_and_termination() {
        let python = PathBuf::from("/usr/bin/python3");
        if !python.is_file() {
            eprintln!("cross-process fixture unavailable: /usr/bin/python3");
            return;
        }
        let mut config = launch();
        config.executable = python;
        config.arguments = vec![
            "-c".into(),
            r#"
import sys, json, struct, os, time
assert set(os.environ).issubset({'LC_CTYPE'})
header = sys.stdin.buffer.read(4)
n = struct.unpack('>I', header)[0]
hello = json.loads(sys.stdin.buffer.read(n))
assert hello['type'] == 'hello'
hello['type'] = 'ready'
body = json.dumps(hello).encode()
sys.stdout.buffer.write(struct.pack('>I', len(body)) + body)
sys.stdout.buffer.flush()
time.sleep(30)
"#
            .into(),
        ];
        let mut worker = config.spawn().await.unwrap();
        assert!(!worker.exited().unwrap());
        worker.terminate().await.unwrap();
        assert!(worker.exited().unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_process_death_during_readiness_is_rejected() {
        let mut config = launch();
        config.executable = PathBuf::from("/bin/sh");
        config.arguments = vec!["-c".into(), "exit 0".into()];
        assert!(matches!(config.spawn().await, Err(ProcessError::Readiness)));
    }

    #[tokio::test]
    async fn missing_worker_has_safe_failure() {
        assert!(matches!(launch().spawn().await, Err(ProcessError::Io)));
    }
}
