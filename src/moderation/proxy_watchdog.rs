//! macOS fallback restores routing when the owner disappears without unwinding.
use crate::wechat_proxy::{ProxyAddress, ProxySnapshot, SystemProxyController};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct Recovery {
    service: String,
    original: ProxySnapshot,
    installed: ProxyAddress,
}

// Only revert values still owned by this capture. Keep later user changes.
fn restore_owned(controller: &impl SystemProxyController, recovery: &Recovery) -> Result<()> {
    let current = controller.current_proxy()?;
    let mut target = current.clone();
    if current.web.as_ref() == Some(&recovery.installed) {
        target.web = recovery.original.web.clone();
    }
    if current.secure_web.as_ref() == Some(&recovery.installed) {
        target.secure_web = recovery.original.secure_web.clone();
    }
    if current != target {
        controller.restore_proxy(&target)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
struct Watchdog {
    child: std::process::Child,
    input: Option<std::process::ChildStdin>,
    recovery: Recovery,
}
#[cfg(target_os = "macos")]
impl Watchdog {
    fn start(recovery: &Recovery) -> Result<Self> {
        use std::{
            io::{BufRead, Write},
            os::unix::process::CommandExt,
            process::{Command, Stdio},
        };
        let mut child = Command::new(std::env::current_exe()?)
            .args(["moderation", "proxy-watchdog"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0)
            .spawn()
            .context("starting proxy recovery watchdog")?;
        let mut input = child.stdin.take().context("watchdog input")?;
        serde_json::to_writer(&mut input, recovery)?;
        writeln!(input)?;
        input.flush()?;
        let mut ready = String::new();
        std::io::BufReader::new(child.stdout.take().context("watchdog ready pipe")?)
            .read_line(&mut ready)?;
        if ready.trim() != "ready" {
            let _ = child.wait();
            bail!("proxy recovery watchdog did not become ready");
        }
        Ok(Self {
            child,
            input: Some(input),
            recovery: recovery.clone(),
        })
    }
    fn disarm(&mut self) -> Result<()> {
        use std::io::Write;
        if let Some(mut input) = self.input.take() {
            writeln!(input, "restored")?;
            input.flush()?;
        }
        let status = self.child.wait()?;
        if !status.success() {
            bail!("proxy watchdog failed: {status}");
        }
        Ok(())
    }
}
#[cfg(target_os = "macos")]
impl Drop for Watchdog {
    fn drop(&mut self) {
        // EOF is the recovery trigger. Do not kill/wait: the main process may
        // itself be disappearing, and the detached helper must finish alone.
        self.input.take();
    }
}

#[cfg(target_os = "macos")]
pub struct RecoveryController<C> {
    controller: C,
    service: String,
    snapshot: std::sync::Mutex<Option<ProxySnapshot>>,
    watchdog: std::sync::Mutex<Option<Watchdog>>,
}
#[cfg(target_os = "macos")]
impl<C> RecoveryController<C> {
    pub fn new(controller: C, service: String) -> Self {
        Self {
            controller,
            service,
            snapshot: Default::default(),
            watchdog: Default::default(),
        }
    }
}
#[cfg(target_os = "macos")]
impl<C: SystemProxyController> SystemProxyController for RecoveryController<C> {
    fn current_proxy(&self) -> Result<ProxySnapshot> {
        let snapshot = self.controller.current_proxy()?;
        *self.snapshot.lock().unwrap() = Some(snapshot.clone());
        Ok(snapshot)
    }
    fn set_proxy(&self, proxy: &ProxyAddress) -> Result<()> {
        let original = self
            .snapshot
            .lock()
            .unwrap()
            .clone()
            .context("missing original proxy snapshot")?;
        let mut watchdog = self.watchdog.lock().unwrap();
        if watchdog.is_none() {
            *watchdog = Some(Watchdog::start(&Recovery {
                service: self.service.clone(),
                original,
                installed: proxy.clone(),
            })?);
        }
        self.controller.set_proxy(proxy)
    }
    fn restore_proxy(&self, original: &ProxySnapshot) -> Result<()> {
        let mut slot = self.watchdog.lock().unwrap();
        if let Some(watcher) = slot.as_ref() {
            let recovery = Recovery {
                original: original.clone(),
                ..watcher.recovery.clone()
            };
            restore_owned(&self.controller, &recovery)?;
        }
        // Disarm only after OS restoration succeeded; EOF still handles errors.
        if let Some(mut watcher) = slot.take() {
            watcher.disarm()?;
        }
        Ok(())
    }
}

pub fn run() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::io::{BufRead, Write};
        let mut input = std::io::stdin().lock();
        let mut line = String::new();
        input.read_line(&mut line)?;
        let recovery: Recovery = serde_json::from_str(&line)?;
        if recovery.installed.host != "127.0.0.1"
            || recovery.installed.port == 0
            || recovery.service.is_empty()
            || recovery.service.len() > 256
            || recovery.service.chars().any(char::is_control)
        {
            bail!("invalid proxy recovery scope");
        }
        println!("ready");
        std::io::stdout().flush()?;
        line.clear();
        input.read_line(&mut line)?;
        if line.trim() == "restored" {
            return Ok(());
        }
        let controller =
            crate::wechat_proxy::MacOsNetworkSetupProxyController::new(&recovery.service);
        let mut last = None;
        for _ in 0..5 {
            match restore_owned(&controller, &recovery) {
                Ok(()) => return Ok(()),
                Err(e) => last = Some(e),
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        return Err(last.unwrap())
            .context("proxy recovery watchdog could not restore system settings");
    }
    #[cfg(not(target_os = "macos"))]
    bail!("proxy recovery watchdog is currently supported on macOS")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Fake(Mutex<ProxySnapshot>);
    impl SystemProxyController for Fake {
        fn current_proxy(&self) -> Result<ProxySnapshot> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn set_proxy(&self, _: &ProxyAddress) -> Result<()> {
            unreachable!()
        }
        fn restore_proxy(&self, v: &ProxySnapshot) -> Result<()> {
            *self.0.lock().unwrap() = v.clone();
            Ok(())
        }
    }
    #[test]
    fn restores_only_owned_settings_including_partial_apply_and_preserves_later_changes() {
        let local = ProxyAddress {
            host: "127.0.0.1".into(),
            port: 2024,
        };
        let other = ProxyAddress {
            host: "user-proxy".into(),
            port: 7890,
        };
        let r = Recovery {
            service: "Wi-Fi".into(),
            original: ProxySnapshot {
                web: Some(other.clone()),
                secure_web: None,
            },
            installed: local.clone(),
        };
        let f = Fake(Mutex::new(ProxySnapshot {
            web: Some(local.clone()),
            secure_web: Some(local.clone()),
        }));
        restore_owned(&f, &r).unwrap();
        assert_eq!(f.current_proxy().unwrap(), r.original);
        *f.0.lock().unwrap() = ProxySnapshot {
            web: Some(local.clone()),
            secure_web: Some(other.clone()),
        };
        restore_owned(&f, &r).unwrap();
        assert_eq!(
            f.current_proxy().unwrap(),
            ProxySnapshot {
                web: Some(other.clone()),
                secure_web: Some(other.clone())
            }
        );
        restore_owned(&f, &r).unwrap();
        assert_eq!(f.current_proxy().unwrap().secure_web, Some(other));
    }
}
