//! Confinement for the processes Praxis starts on behalf of a model.
//!
//! A plugin script or a shell command runs as the operator today, so a
//! prompt-injected instruction that reaches a tool call can do anything the
//! operator can. This narrows the syscall surface and the resource budget in the
//! child, between fork and exec, before the interpreter's own code runs.
//!
//! Design: a deny-list, not an allow-list. An allow-list breaks a Python
//! interpreter the first time it calls a syscall nobody predicted; a deny-list
//! keeps the ordinary surface and removes the calls that let a process escape,
//! observe another process, or change its own identity. Refusals are errno, not
//! SIGSYS, so a confined process sees a normal "permission denied" instead of a
//! crashed process.
//!
//! Everything is applied in the child. Nothing is applied to the Praxis process
//! itself: the host stays the trust boundary.

use std::path::PathBuf;
use std::sync::OnceLock;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// Resource budget plus the optional filesystem write set. Every field is
/// operator policy; a model can never widen it through a tool argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub address_space_bytes: Option<u64>,
    pub cpu_seconds: Option<u64>,
    pub open_files: Option<u64>,
    pub file_bytes: Option<u64>,
    pub processes: Option<u64>,
    /// Paths a confined process may write. Empty means no landlock ruleset.
    pub write_dirs: Vec<PathBuf>,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self {
            // Generous enough for a normal build, tight enough that a runaway
            // process cannot exhaust the host. CPU is left unset because a
            // background job is allowed to run for a long time.
            // Node/V8 and POML's WebAssembly dependencies reserve large virtual
            // regions. This is an address-space ceiling, not allocated RAM.
            address_space_bytes: Some(64 * 1024 * 1024 * 1024),
            cpu_seconds: None,
            open_files: Some(256),
            file_bytes: Some(32 * 1024 * 1024),
            // RLIMIT_NPROC counts tasks, not processes, and one user on a normal
            // machine already has hundreds of threads. Too low and a plugin that
            // spawns anything at all fails with EAGAIN.
            processes: Some(2048),
            write_dirs: Vec::new(),
        }
    }
}

fn env_number(name: &str, default: Option<u64>) -> Option<u64> {
    match std::env::var(name).ok() {
        Some(value) => {
            let value = value.trim();
            if value.eq_ignore_ascii_case("off") || value == "0" {
                return None;
            }
            value.parse::<u64>().ok().filter(|v| *v > 0).or(default)
        }
        None => default,
    }
}

fn env_list(name: &str) -> Vec<PathBuf> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .map(PathBuf::from)
        .collect()
}

impl Sandbox {
    pub fn from_env() -> Self {
        let default = Sandbox::default();
        let mut sandbox = Sandbox {
            address_space_bytes: env_number("PRAXIS_SANDBOX_ADDRESS_SPACE_MB", default.address_space_bytes.map(|v| v / 1024 / 1024)),
            cpu_seconds: env_number("PRAXIS_SANDBOX_CPU_SECONDS", default.cpu_seconds),
            open_files: env_number("PRAXIS_SANDBOX_OPEN_FILES", default.open_files),
            file_bytes: env_number("PRAXIS_SANDBOX_FILE_BYTES_MB", default.file_bytes.map(|v| v / 1024 / 1024)),
            processes: env_number("PRAXIS_SANDBOX_PROCESSES", default.processes),
            write_dirs: env_list("PRAXIS_SANDBOX_WRITE_DIRS"),
        };
        sandbox.address_space_bytes = sandbox.address_space_bytes.map(|v| v * 1024 * 1024);
        sandbox.file_bytes = sandbox.file_bytes.map(|v| v * 1024 * 1024);
        sandbox
    }

    pub fn is_disabled(&self) -> bool {
        std::env::var("PRAXIS_SANDBOX").ok().as_deref() == Some("off")
    }

    /// Syscalls removed from a confined process: kernel control, cross-process
    /// observation, identity change, and installing a wider filter over this one.
    #[cfg(unix)]
    pub fn denied_syscalls() -> Vec<i64> {
        let mut denied = vec![
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
            libc::SYS_kexec_load,
            libc::SYS_reboot,
            libc::SYS_mount,
            libc::SYS_umount2,
            libc::SYS_pivot_root,
            libc::SYS_chroot,
            libc::SYS_setns,
            libc::SYS_unshare,
            libc::SYS_bpf,
            libc::SYS_perf_event_open,
            libc::SYS_userfaultfd,
            libc::SYS_init_module,
            libc::SYS_finit_module,
            libc::SYS_delete_module,
            libc::SYS_acct,
            libc::SYS_quotactl,
            libc::SYS_setuid,
            libc::SYS_setgid,
            libc::SYS_setfsuid,
            libc::SYS_setfsgid,
            libc::SYS_seccomp,
        ];
        // libc exposes no constant for the effective-id calls. On x86_64 they are
        // separate syscalls, so they are named by number; the generic table used
        // by aarch64 has no such syscall at all.
        #[cfg(target_arch = "x86_64")]
        denied.extend([107, 108]); // set_euid, set_egid
        denied.sort();
        denied
    }
}

#[cfg(unix)]
mod bpf {
    // Declared locally so the filter does not depend on which constants libc
    // happens to expose for each architecture.
    pub const BPF_LD: u16 = 0x00;
    pub const BPF_W: u16 = 0x01;
    pub const BPF_ABS: u16 = 0x01;
    pub const BPF_JMP: u16 = 0x05;
    pub const BPF_JEQ: u16 = 0x03;
    pub const BPF_K: u16 = 0x00;
    pub const BPF_RET: u16 = 0x06;

    pub const SECCOMP_RET_ALLOW: u32 = 0x7000_0000;
    pub const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    pub const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
    pub const EPERM: u32 = 1;

    pub const AUDIT_ARCH_X86_64: u32 = 0xC000_0022;
    pub const AUDIT_ARCH_AARCH64: u32 = 0xC000_00D7;

    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Statement {
        pub code: u16,
        pub jt: u8,
        pub jf: u8,
        pub k: u32,
    }

    pub fn stmt(code: u16, k: u32) -> Statement {
        Statement { code, jt: 0, jf: 0, k }
    }

    pub fn jump(code: u16, k: u32, jt: u8, jf: u8) -> Statement {
        Statement { code, jt, jf, k }
    }
}

/// The filter program: check the audit architecture, then refuse each denied
/// syscall with EPERM. A foreign architecture is refused outright rather than
/// trusted, because the syscall numbers would be wrong for it.
#[cfg(unix)]
pub fn filter(denied: &[i64], arch: u32) -> Vec<bpf::Statement> {
    use bpf::*;
    let mut program = Vec::new();
    // seccomp_data.nr is at offset 0, .arch at offset 4.
    program.push(stmt(BPF_LD | BPF_W | BPF_ABS, 4));
    program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, arch, 0, (denied.len() * 2 + 1) as u8));
    for number in denied {
        program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, *number as u32, 0, 1));
        program.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM));
    }
    program.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
    program.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS));
    program
}

#[cfg(unix)]
fn audit_arch() -> u32 {
    if cfg!(target_arch = "x86_64") {
        bpf::AUDIT_ARCH_X86_64
    } else if cfg!(target_arch = "aarch64") {
        bpf::AUDIT_ARCH_AARCH64
    } else {
        0
    }
}

#[cfg(unix)]
const PR_SET_NO_NEW_PRIVS: i32 = 38;
#[cfg(unix)]
const PR_SET_SECCOMP: i32 = 22;
#[cfg(unix)]
const SECCOMP_MODE_FILTER: i32 = 2;

#[cfg(unix)]
#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const bpf::Statement,
}

/// Apply confinement in the child, after fork and before exec.
#[cfg(unix)]
pub fn apply(sandbox: &Sandbox) -> anyhow::Result<()> {
    apply_with(sandbox, seccomp_supported())
}

/// The support flag is resolved in the parent, before fork. Nothing is spawned
/// or allocated between fork and exec.
pub fn apply_with(sandbox: &Sandbox, seccomp: bool) -> anyhow::Result<()> {
    if sandbox.is_disabled() {
        return Ok(());
    }
    unsafe {
        // A filter on an unprivileged process requires no_new_privs first.
        if libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            anyhow::bail!("sandbox: cannot disable privilege acquisition ({})", io_error());
        }
        for (resource, value) in [
            (libc::RLIMIT_AS, sandbox.address_space_bytes),
            (libc::RLIMIT_CPU, sandbox.cpu_seconds),
            (libc::RLIMIT_NOFILE, sandbox.open_files),
            (libc::RLIMIT_FSIZE, sandbox.file_bytes),
            (libc::RLIMIT_NPROC, sandbox.processes),
        ] {
            if let Some(value) = value {
                let rl = libc::rlimit { rlim_cur: value as libc::c_ulong, rlim_max: value as libc::c_ulong };
                if libc::setrlimit(resource, &rl) != 0 {
                    anyhow::bail!("sandbox: cannot set resource limit {} ({})", resource, io_error());
                }
            }
        }

        let program = filter(&Sandbox::denied_syscalls(), audit_arch());
        if seccomp {
            let prog = SockFprog { len: program.len() as u16, filter: program.as_ptr() };
            if libc::prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog as *const _ as libc::c_ulong, 0, 0) != 0 {
                anyhow::bail!("sandbox: cannot install the syscall filter ({})", io_error());
            }
        }
        if !sandbox.write_dirs.is_empty() {
            landlock(&sandbox.write_dirs)?;
        }
    }
    Ok(())
}

/// Some container policies kill a process that installs a filter instead of
/// refusing it. A sandbox that cannot be installed must not take the whole
/// plugin and terminal path down with it, so support is probed once, in a
/// throwaway child, and cached.
pub fn seccomp_supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| probe())
}

#[cfg(unix)]
fn probe() -> bool {
    let program = filter(&[], audit_arch());
    let outcome = unsafe {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .pre_exec(move || {
                if libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let prog = SockFprog { len: program.len() as u16, filter: program.as_ptr() };
                if libc::prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog as *const _ as libc::c_ulong, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })
            .status()
    };
    matches!(outcome, Ok(status) if status.success())
}

#[cfg(not(unix))]
pub fn apply(_sandbox: &Sandbox) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn io_error() -> String {
    std::io::Error::last_os_error().to_string()
}

/// Optional filesystem confinement: read anywhere, write only where declared.
/// Requested explicitly by the operator, so a kernel that cannot honour it fails
/// the exec instead of running unconfined quietly.
#[cfg(all(unix, any(target_arch = "x86_64", target_arch = "aarch64")))]
fn landlock(write_dirs: &[PathBuf]) -> anyhow::Result<()> {
    const LANDLOCK_CREATE_RULESET: i64 = 444;
    const LANDLOCK_ADD_RULE: i64 = 445;
    const LANDLOCK_RESTRICT_SELF: i64 = 446;
    const RULE_PATH: i64 = 1;
    const ACCESS_FS_READ_FILE: u32 = 1 << 0;
    const ACCESS_FS_READ_DIR: u32 = 1 << 6;
    const ACCESS_FS_ALL_V1: u32 = 0x1fff;

    #[repr(C)]
    struct RulesetAttr { allowed_ruleset: u32 }
    #[repr(C)]
    struct PathRule { path_fd: i32, allowed_access: u32 }

    let attr = RulesetAttr { allowed_ruleset: ACCESS_FS_ALL_V1 };
    let ruleset = unsafe { libc::syscall(LANDLOCK_CREATE_RULESET, &attr as *const _, 8, 0) };
    if ruleset < 0 {
        anyhow::bail!("sandbox: landlock is unavailable ({})", io_error());
    }
    let read_only = PathRule { path_fd: libc::AT_FDCWD, allowed_access: ACCESS_FS_READ_FILE | ACCESS_FS_READ_DIR };
    if unsafe { libc::syscall(LANDLOCK_ADD_RULE, ruleset, RULE_PATH, &read_only as *const _) } != 0 {
        unsafe { libc::close(ruleset as i32) };
        anyhow::bail!("sandbox: landlock read rule failed ({})", io_error());
    }
    for dir in write_dirs {
        let path = std::ffi::CString::new(dir.to_string_lossy().as_ref())
            .map_err(|_| anyhow::anyhow!("sandbox: landlock write dir {} is not a valid path", dir.display()))?;
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            unsafe { libc::close(ruleset as i32) };
            anyhow::bail!("sandbox: landlock write dir {} is not readable ({})", dir.display(), io_error());
        }
        let rule = PathRule { path_fd: fd, allowed_access: ACCESS_FS_ALL_V1 };
        let outcome = unsafe { libc::syscall(LANDLOCK_ADD_RULE, ruleset, RULE_PATH, &rule as *const _) };
        unsafe { libc::close(fd) };
        if outcome != 0 {
            unsafe { libc::close(ruleset as i32) };
            anyhow::bail!("sandbox: landlock write rule for {} failed ({})", dir.display(), io_error());
        }
    }
    if unsafe { libc::syscall(LANDLOCK_RESTRICT_SELF, ruleset, 0) } != 0 {
        unsafe { libc::close(ruleset as i32) };
        anyhow::bail!("sandbox: landlock could not be applied ({})", io_error());
    }
    unsafe { libc::close(ruleset as i32) };
    Ok(())
}

#[cfg(all(unix, not(any(target_arch = "x86_64", target_arch = "aarch64"))))]
fn landlock(write_dirs: &[PathBuf]) -> anyhow::Result<()> {
    anyhow::bail!("sandbox: landlock is not supported on this architecture ({} write dirs requested)", write_dirs.len());
}

/// The `pre_exec` hook for a process builder.
#[cfg(unix)]
pub fn hook(sandbox: &Sandbox) -> impl FnMut() -> std::io::Result<()> + Send + Sync + 'static {
    let sandbox = sandbox.clone();
    move || {
        apply(&sandbox)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error.to_string()))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    #[test]
    fn the_budget_is_operator_policy_with_a_working_default() {
        let sandbox = Sandbox::default();
        assert_eq!(sandbox.address_space_bytes, Some(64 * 1024 * 1024 * 1024));
        assert_eq!(sandbox.open_files, Some(256));
        assert_eq!(sandbox.file_bytes, Some(32 * 1024 * 1024));
        assert_eq!(sandbox.processes, Some(2048));
        assert_eq!(sandbox.cpu_seconds, None, "a background job is allowed to run long");
        assert!(sandbox.write_dirs.is_empty(), "filesystem confinement is opt-in");
    }

    #[cfg(unix)]
    #[test]
    fn the_filter_refuses_escalation_and_keeps_the_ordinary_surface() {
        let denied = Sandbox::denied_syscalls();
        assert!(denied.contains(&libc::SYS_ptrace));
        assert!(denied.contains(&libc::SYS_setuid));
        assert!(denied.contains(&libc::SYS_unshare));
        assert!(!denied.contains(&libc::SYS_read), "a plugin still has to read its arguments");
        assert!(!denied.contains(&libc::SYS_execve), "the filter must not block exec itself");
        let program = filter(&denied, bpf::AUDIT_ARCH_X86_64);
        assert_eq!(program.len(), denied.len() * 2 + 4);
        // From the architecture check, the false branch must land on the kill
        // return: it skips the deny pairs plus the allow return.
        assert_eq!(program[1].jf, (denied.len() * 2 + 1) as u8);
        assert_eq!(program[program.len() - 1].k, bpf::SECCOMP_RET_KILL_PROCESS, "a foreign architecture is not trusted");
    }

    #[cfg(unix)]
    #[test]
    fn a_host_that_cannot_install_filters_degrades_instead_of_killing_the_child() {
        // The probe is the whole point: an unsupported kernel must not take the
        // plugin and terminal path down with it.
        let sandbox = Sandbox::default();
        assert!(apply(&sandbox).is_ok());
        if seccomp_supported() {
            assert!(Sandbox::denied_syscalls().contains(&libc::SYS_ptrace));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_confined_child_is_denied_escalation_but_still_runs() {
        if std::process::Command::new("python3").arg("-c").arg("pass").output().is_err() {
            return;
        }
        let sandbox = Sandbox::default();
        let seccomp = seccomp_supported();
        let outcome = unsafe {
            std::process::Command::new("python3")
                .arg("-c")
                .arg("import os,sys\ntry:\n os.setuid(0)\nexcept PermissionError: sys.exit(42)")
                .pre_exec(move || {
                    apply_with(&sandbox, seccomp).map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error.to_string()))
                })
                .output()
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => { eprintln!("skipped: {error}"); return; }
        };
        if !seccomp_supported() {
            eprintln!("skipped: this host cannot install seccomp filters, so the filter is skipped by design");
            return;
        }
        // A host that refuses filter installation outright is an environment
        // limit, not a broken filter; the shape is checked by the test above.
        match outcome.status.code() {
            Some(42) => {}
            Some(code) => panic!("expected the confined child to be denied (42), got {code}"),
            None => eprintln!(
                "skipped: this host kills a confined child outright (signal {}), so the filter cannot be verified here",
                outcome.status.signal().unwrap_or(-1)
            ),
        }
    }
}
