//! Subscribe before touching proxy state; terminal close is not Ctrl-C.
pub struct ShutdownSignals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    hangup: tokio::signal::unix::Signal,
    #[cfg(windows)]
    interrupt: tokio::signal::windows::CtrlC,
}
impl ShutdownSignals {
    pub fn new() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())?,
                terminate: signal(SignalKind::terminate())?,
                hangup: signal(SignalKind::hangup())?,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                interrupt: tokio::signal::windows::ctrl_c()?,
            })
        }
    }
    pub async fn recv(&mut self) -> Option<()> {
        #[cfg(unix)]
        {
            tokio::select! {v=self.interrupt.recv()=>v,v=self.terminate.recv()=>v,v=self.hangup.recv()=>v}
        }
        #[cfg(windows)]
        {
            self.interrupt.recv().await
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "subprocess fixture for signal delivery"]
    async fn signal_child() {
        let Some(directory) = std::env::var_os("LIVE_ARCHIVE_SIGNAL_FIXTURE") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        let mut signals = ShutdownSignals::new().unwrap();
        std::fs::write(directory.join("ready"), b"ready").unwrap();
        assert_eq!(signals.recv().await, Some(()));
        std::fs::write(directory.join("handled"), b"handled").unwrap();
    }
    #[test]
    fn terminal_close_terminate_and_interrupt_reach_the_shutdown_path() {
        for signal in [libc::SIGHUP, libc::SIGTERM, libc::SIGINT] {
            let directory = tempfile::tempdir().unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "moderation::shutdown::tests::signal_child",
                    "--ignored",
                ])
                .env("LIVE_ARCHIVE_SIGNAL_FIXTURE", directory.path())
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !directory.path().join("ready").exists() {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("signal fixture did not start");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            // The PID belongs to the child created above, never a user process.
            assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("shutdown signal was not handled");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(directory.path().join("handled").exists());
        }
    }
}
