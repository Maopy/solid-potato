//! Uses the recording proxy/certificate controllers. Enabled only by explicit YAML configuration.
use crate::wechat_proxy::{
    LocalCaCertificate, LocalCertificateController, ProxyAddress, ProxyConsent,
    SystemProxyController, WeChatCaptureCoordinator, WeChatProxySession,
};
use anyhow::Result;
use std::path::Path;

pub trait ProxyLease: Send + Sync {
    fn stop(&self) -> Result<()>;
}
impl<C: SystemProxyController, M: LocalCertificateController> ProxyLease
    for WeChatCaptureCoordinator<C, M>
{
    fn stop(&self) -> Result<()> {
        WeChatCaptureCoordinator::stop(self)
    }
}

impl<C: SystemProxyController> ProxyLease for WeChatProxySession<C> {
    fn stop(&self) -> Result<()> {
        WeChatProxySession::stop(self)
    }
}

pub fn start_with_controllers<
    C: SystemProxyController + 'static,
    M: LocalCertificateController + 'static,
>(
    proxy: C,
    certificate: M,
    port: u16,
    inspect_tls: bool,
) -> Result<Box<dyn ProxyLease>> {
    let consent = ProxyConsent {
        install_local_ca: true,
        intercept_wechat_traffic: true,
    };
    let address = ProxyAddress {
        host: "127.0.0.1".into(),
        port,
    };
    if inspect_tls {
        let coordinator = WeChatCaptureCoordinator::new(proxy, certificate);
        coordinator.start(consent, address)?;
        Ok(Box::new(coordinator))
    } else {
        let session = WeChatProxySession::new(proxy);
        session.start(consent, address)?;
        Ok(Box::new(session))
    }
}

pub fn start(directory: &Path, port: u16, inspect_tls: bool) -> Result<Box<dyn ProxyLease>> {
    let certificate = LocalCaCertificate {
        common_name: "mitmproxy".into(),
        certificate_path: directory.join("mitmproxy-ca-cert.pem"),
    };
    #[cfg(target_os = "macos")]
    let (proxy, certificate) = {
        use crate::wechat_proxy::{
            MacOsNetworkSetupProxyController, MacOsSecurityCertificateController,
            active_macos_network_service,
        };
        let service = active_macos_network_service()?;
        (
            super::proxy_watchdog::RecoveryController::new(
                MacOsNetworkSetupProxyController::new(&service),
                service,
            ),
            MacOsSecurityCertificateController::new(certificate),
        )
    };
    #[cfg(target_os = "windows")]
    let (proxy, certificate) = {
        use crate::wechat_proxy::{
            WindowsCertUtilCertificateController, WindowsInternetSettingsProxyController,
        };
        (
            super::proxy_watchdog::RecoveryController::new(
                WindowsInternetSettingsProxyController::new(),
                "Windows Internet Settings".into(),
            ),
            WindowsCertUtilCertificateController::new(certificate),
        )
    };
    #[cfg(target_os = "linux")]
    let (proxy, certificate) = {
        use crate::wechat_proxy::{GnomeProxyController, LinuxUpdateCaCertificateController};
        (
            GnomeProxyController::new(),
            LinuxUpdateCaCertificateController::new(
                certificate,
                "/usr/local/share/ca-certificates/live-archive-mitmproxy-ca.crt".into(),
            ),
        )
    };
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        start_with_controllers(proxy, certificate, port, inspect_tls)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    anyhow::bail!("system proxy management is unsupported on this OS")
}
