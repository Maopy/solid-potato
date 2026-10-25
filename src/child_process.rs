//! mitmdump's Windows .exe launcher owns a separate Python interpreter.
//! Stop the whole owned tree so the interpreter cannot retain the proxy port.
use anyhow::{Context, Result};
use tokio::process::Child;

pub(crate) async fn stop_proxy_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        #[cfg(windows)]
        if let Some(pid) = child.id() {
            let output = tokio::process::Command::new("taskkill.exe")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x08000000)
                .output()
                .await
                .context("stopping proxy process tree")?;
            if !output.status.success() && child.try_wait()?.is_none() {
                anyhow::bail!(
                    "could not stop proxy process tree: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        #[cfg(not(windows))]
        child.kill().await.context("stopping proxy process")?;
    }
    child.wait().await.context("waiting for proxy process")?;
    Ok(())
}

pub(crate) fn drop_proxy_child(child: &mut Child) {
    #[cfg(windows)]
    if child.try_wait().ok().flatten().is_none() {
        if let Some(pid) = child.id() {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill.exe")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x08000000)
                .output();
        }
    }
    let _ = child.start_kill();
}
