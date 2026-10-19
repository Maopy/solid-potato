//! Transactional lifecycle for a consented WeChat capture session.
use crate::{
    capture_sidecar::CaptureSidecar,
    wechat_proxy::{
        LocalCertificateController, ProxyConsent, SystemProxyController, WeChatCaptureCoordinator,
    },
};
use anyhow::Result;

pub struct WeChatCaptureRuntime<
    C: SystemProxyController,
    M: LocalCertificateController,
    S: CaptureSidecar,
> {
    coordinator: WeChatCaptureCoordinator<C, M>,
    sidecar: S,
    active: bool,
}

impl<C: SystemProxyController, M: LocalCertificateController, S: CaptureSidecar>
    WeChatCaptureRuntime<C, M, S>
{
    pub fn new(proxy: C, certificate: M, sidecar: S) -> Self {
        Self {
            coordinator: WeChatCaptureCoordinator::new(proxy, certificate),
            sidecar,
            active: false,
        }
    }

    pub async fn start(&mut self, consent: ProxyConsent) -> Result<()> {
        if self.active {
            return Ok(());
        }
        self.sidecar.start().await?;
        if let Err(error) = self
            .coordinator
            .start(consent, self.sidecar.proxy_address())
        {
            let _ = self.sidecar.stop().await;
            return Err(error);
        }
        self.active = true;
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        let proxy_result = self.coordinator.stop();
        let sidecar_result = self.sidecar.stop().await;
        self.active = false;
        proxy_result?;
        sidecar_result
    }
}

impl<C: SystemProxyController, M: LocalCertificateController, S: CaptureSidecar> Drop
    for WeChatCaptureRuntime<C, M, S>
{
    fn drop(&mut self) {
        let _ = self.coordinator.stop();
    }
}
