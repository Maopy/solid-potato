//! Reuses the existing same-origin mitmproxy injection approach, without
//! changing the recorder addon or system proxy/certificate settings.
use super::protocol::{PREFIX, ProbeScope};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::{Child, Command};

pub fn render_addon(scope: &ProbeScope, bridge: SocketAddr, relay_token: &str) -> String {
    render_addon_with_injection(scope, bridge, relay_token, true)
}

pub fn render_addon_with_injection(
    scope: &ProbeScope,
    bridge: SocketAddr,
    relay_token: &str,
    inject_page: bool,
) -> String {
    render_addon_with_prefix(scope, bridge, relay_token, inject_page, PREFIX)
}

pub fn render_addon_with_prefix(
    scope: &ProbeScope,
    bridge: SocketAddr,
    relay_token: &str,
    inject_page: bool,
    prefix: &str,
) -> String {
    let config = json!({"host":scope.room_url.host_str(),"room_path":scope.room_url.path(),"prefix":prefix,"bridge_host":bridge.ip().to_string(),"bridge_port":bridge.port(),"relay_token":relay_token,"inject_page":inject_page});
    let template = include_str!("../../assets/moderation/proxy/addon.py");
    // JSON decoded from a JSON string literal also handles Python booleans.
    let addon = template.replace(
        "__CONFIG_JSON_STRING__",
        &serde_json::to_string(&config.to_string()).expect("serializing config string"),
    );
    format!("{}\n{addon}", include_str!("../../assets/moderation/proxy/windows_asyncio.py"))
}

pub fn ignore_hosts(scope: &ProbeScope) -> Result<String> {
    let host = scope
        .room_url
        .host_str()
        .context("missing injection host")?;
    if !matches!(host, "live.douyin.com" | "channels.weixin.qq.com") {
        bail!("unsupported injection host");
    }
    Ok(format!("^(?!{}:443$).*", regex::escape(host)))
}

// Keep a stable guard inode: unlinking an OS lock file would let a second
// contender lock a different inode while an existing waiter owns the first.
struct KernelGuard(fs::File);
impl Drop for KernelGuard {
    fn drop(&mut self) {
        // Release explicitly even if a concurrently forked child has not exec'd yet.
        let _ = self.0.unlock();
    }
}
struct DirectoryLease {
    _guard: KernelGuard,
    pid_path: PathBuf,
}
impl DirectoryLease {
    fn acquire(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let guard = options
            .open(directory.join("moderation-probe.guard"))
            .context("open probe directory guard")?;
        guard.try_lock().context(
            "another running instance owns this directory; use its control page or stop it first",
        )?;
        let guard = KernelGuard(guard);
        let pid_path = directory.join("moderation-probe.lock");
        match fs::read(&pid_path) {
            Ok(original) => {
                let text = std::str::from_utf8(&original)
                    .context("unrecognized old probe lock; cannot safely recover")?;
                let pid: u32 = text
                    .trim()
                    .parse()
                    .context("unrecognized old probe PID; cannot safely recover")?;
                if process_exists(pid)? {
                    bail!(
                        "场控进程 {pid} 仍在运行，请使用已打开的控制页，或先在原终端按 Ctrl-C 退出；未删除现用锁"
                    );
                }
                if fs::read(&pid_path)? != original {
                    bail!(
                        "probe lock changed during recovery; retry after the other startup finishes"
                    );
                }
                fs::remove_file(&pid_path).context("remove exited owner's probe lock")?;
                eprintln!("已恢复异常退出进程 {pid} 留下的场控锁");
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("read existing probe lock"),
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut pid_file = options
            .open(&pid_path)
            .context("another probe claimed this directory during startup")?;
        if let Err(error) = writeln!(pid_file, "{}", std::process::id()) {
            let _ = fs::remove_file(&pid_path);
            return Err(error.into());
        }
        Ok(Self {
            _guard: guard,
            pid_path,
        })
    }
}
impl Drop for DirectoryLease {
    fn drop(&mut self) {
        if fs::read_to_string(&self.pid_path)
            .ok()
            .is_some_and(|text| text.trim() == std::process::id().to_string())
        {
            let _ = fs::remove_file(&self.pid_path);
        }
    }
}
#[cfg(unix)]
fn process_exists(pid: u32) -> Result<bool> {
    if pid == 0 || pid > i32::MAX as u32 {
        bail!("invalid existing probe PID; cannot safely recover");
    }
    // Signal zero checks existence without delivering a signal.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return Ok(true);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => bail!("cannot verify existing probe PID; lock preserved"),
    }
}
#[cfg(windows)]
fn process_exists(pid: u32) -> Result<bool> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    if pid == 0 {
        bail!("invalid existing probe PID; cannot safely recover");
    }
    // Query only; access denied is treated conservatively as a live owner.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        return Ok(
            std::io::Error::last_os_error().raw_os_error() != Some(ERROR_INVALID_PARAMETER as i32)
        );
    }
    let status = unsafe { WaitForSingleObject(process, 0) };
    unsafe { CloseHandle(process) };
    match status {
        WAIT_OBJECT_0 => Ok(false),
        WAIT_TIMEOUT => Ok(true),
        _ => bail!("cannot verify existing probe PID; lock preserved"),
    }
}
#[cfg(not(any(unix, windows)))]
fn process_exists(_pid: u32) -> Result<bool> {
    bail!(
        "existing legacy probe lock requires manual process verification on this platform; lock preserved"
    )
}

pub struct ProbeSidecar {
    child: Option<Child>,
    addon_path: PathBuf,
    lease: Option<DirectoryLease>,
}

impl ProbeSidecar {
    pub async fn start(
        executable: &Path,
        directory: &Path,
        port: u16,
        source: &str,
        scope: &ProbeScope,
        tls_passthrough: bool,
        http2: bool,
    ) -> Result<Self> {
        Self::start_many(
            executable,
            directory,
            port,
            source,
            std::slice::from_ref(scope),
            tls_passthrough,
            http2,
        )
        .await
    }

    pub async fn start_many(
        executable: &Path,
        directory: &Path,
        port: u16,
        source: &str,
        scopes: &[ProbeScope],
        tls_passthrough: bool,
        http2: bool,
    ) -> Result<Self> {
        Self::start_many_with_recording(
            executable,
            directory,
            port,
            source,
            scopes,
            tls_passthrough,
            http2,
            false,
        )
        .await
    }

    pub async fn start_many_with_recording(
        executable: &Path,
        directory: &Path,
        port: u16,
        source: &str,
        scopes: &[ProbeScope],
        tls_passthrough: bool,
        http2: bool,
        recording: bool,
    ) -> Result<Self> {
        if scopes.is_empty() {
            bail!("at least one injection scope is required");
        }
        let mut hosts = std::collections::BTreeSet::new();
        for scope in scopes {
            ignore_hosts(scope)?;
            hosts.insert(scope.room_url.host_str().unwrap().to_owned());
        }
        if recording && hosts.contains("channels.weixin.qq.com") {
            hosts.insert("res.wx.qq.com".to_owned());
            hosts.insert("finder.video.qq.com".to_owned());
        }
        let ignored = format!(
            "^(?!(?:{}):443$).*",
            hosts
                .iter()
                .map(|h| regex::escape(h))
                .collect::<Vec<_>>()
                .join("|")
        );
        if port == 0 {
            bail!("probe proxy port must be nonzero");
        }
        let probe = std::net::TcpListener::bind(("127.0.0.1", port))
            .context("probe proxy port is already occupied")?;
        drop(probe);
        fs::create_dir_all(directory)?;
        let lease = DirectoryLease::acquire(directory)?;
        let addon_path = directory.join("moderation_probe.py");
        let mut sidecar = Self {
            child: None,
            addon_path,
            lease: Some(lease),
        };
        let result = async {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options
                .open(&sidecar.addon_path)?
                .write_all(source.as_bytes())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&sidecar.addon_path, fs::Permissions::from_mode(0o600))?;
            }
            sidecar.child = Some(
                Command::new(executable)
                    .args(["-q", "--listen-host", "127.0.0.1", "--listen-port"])
                    .arg(port.to_string())
                    .args([
                        "--set",
                        "connection_strategy=lazy",
                        "--set",
                        "block_global=false",
                        "--set",
                        "termlog_verbosity=warn",
                    ])
                    .arg("--set")
                    .arg(format!("http2={http2}"))
                    .arg("--set")
                    .arg(format!("confdir={}", directory.display()))
                    .arg("--ignore-hosts")
                    .arg(if tls_passthrough {
                        ".*".to_owned()
                    } else {
                        ignored
                    })
                    .arg("-s")
                    .arg(&sidecar.addon_path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .kill_on_drop(true)
                    .spawn()
                    .context("starting moderation injection proxy (no chat submission)")?,
            );
            for _ in 0..50 {
                if sidecar
                    .child
                    .as_mut()
                    .expect("spawned")
                    .try_wait()?
                    .is_some()
                {
                    bail!("probe proxy exited during startup");
                }
                if tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .is_ok()
                {
                    return Ok(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            bail!("probe proxy did not become ready within 5 seconds");
        }
        .await;
        if let Err(error) = result {
            let _ = sidecar.stop().await;
            return Err(error);
        }
        Ok(sidecar)
    }

    pub async fn wait_for_exit(&mut self) -> Result<()> {
        let status = self
            .child
            .as_mut()
            .context("probe proxy is not running")?
            .wait()
            .await?;
        bail!("probe proxy exited unexpectedly: {status}");
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some(child) = self.child.as_mut() {
            crate::child_process::stop_proxy_child(child).await?;
        }
        self.child.take();
        if self.lease.is_some() && self.addon_path.exists() {
            fs::remove_file(&self.addon_path)?;
        }
        self.lease.take();
        Ok(())
    }
}

impl Drop for ProbeSidecar {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            crate::child_process::drop_proxy_child(child);
        }
        self.child.take();
        if self.lease.is_some() {
            let _ = fs::remove_file(&self.addon_path);
        }
        self.lease.take();
    }
}

#[cfg(all(test, unix))]
mod directory_lock_tests {
    use super::*;
    #[test]
    fn active_owner_and_concurrent_contenders_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("moderation-probe.lock");
        fs::write(&path, format!("{}\n", std::process::id())).unwrap();
        assert!(DirectoryLease::acquire(dir.path()).is_err());
        assert!(path.exists());
        fs::remove_file(&path).unwrap();
        let first = DirectoryLease::acquire(dir.path()).unwrap();
        assert!(DirectoryLease::acquire(dir.path()).is_err());
        drop(first);
        assert!(!path.exists());
        assert!(dir.path().join("moderation-probe.guard").exists());
        let next = DirectoryLease::acquire(dir.path()).unwrap();
        drop(next);
        assert!(!path.exists());
    }
    #[test]
    fn killed_owner_releases_kernel_guard_and_next_start_recovers_pid() {
        use std::io::BufRead;
        let dir = tempfile::tempdir().unwrap();
        let script = r#"import fcntl,os,pathlib,sys
root=pathlib.Path(sys.argv[1])
guard=(root/'moderation-probe.guard').open('a+')
fcntl.flock(guard,fcntl.LOCK_EX)
(root/'moderation-probe.lock').write_text(str(os.getpid())+'\n')
print('ready',flush=True)
sys.stdin.read()
"#;
        let mut child =
            std::process::Command::new(if cfg!(windows) { "python" } else { "python3" })
                .args(["-X", "utf8"])
                .args(["-c", script])
                .arg(dir.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready");
        assert!(DirectoryLease::acquire(dir.path()).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
        let recovered = DirectoryLease::acquire(dir.path()).unwrap();
        drop(recovered);
        assert!(!dir.path().join("moderation-probe.lock").exists());
    }
    #[test]
    fn exited_owner_is_recovered_but_invalid_lock_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("moderation-probe.lock");
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        fs::write(&path, format!("{pid}\n")).unwrap();
        let lease = DirectoryLease::acquire(dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap().trim(),
            std::process::id().to_string()
        );
        drop(lease);
        fs::write(&path, "unknown").unwrap();
        assert!(DirectoryLease::acquire(dir.path()).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "unknown");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn recovers_exited_owner_and_keeps_live_owner_lock() {
        assert!(process_exists(std::process::id()).unwrap());
        assert!(!process_exists(u32::MAX).unwrap());
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("moderation-probe.lock");
        fs::write(&pid_path, u32::MAX.to_string()).unwrap();
        let lease = DirectoryLease::acquire(dir.path()).unwrap();
        assert!(DirectoryLease::acquire(dir.path()).is_err());
        drop(lease);
        assert!(!pid_path.exists());
        fs::write(&pid_path, std::process::id().to_string()).unwrap();
        assert!(DirectoryLease::acquire(dir.path()).is_err());
        assert!(pid_path.exists());
    }
}
