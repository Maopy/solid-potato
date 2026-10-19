use async_trait::async_trait;
use live_archive_core::{
    capture_sidecar::CaptureSidecar,
    wechat_proxy::{
        LocalCertificateController, ProxyAddress, ProxyConsent, ProxySnapshot,
        SystemProxyController,
    },
    wechat_runtime::WeChatCaptureRuntime,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Proxy(Arc<Mutex<Vec<&'static str>>>);
impl SystemProxyController for Proxy {
    fn current_proxy(&self) -> anyhow::Result<ProxySnapshot> {
        Ok(ProxySnapshot::default())
    }
    fn set_proxy(&self, _proxy: &ProxyAddress) -> anyhow::Result<()> {
        self.0.lock().unwrap().push("set");
        Ok(())
    }
    fn restore_proxy(&self, _proxy: &ProxySnapshot) -> anyhow::Result<()> {
        self.0.lock().unwrap().push("restore");
        Ok(())
    }
}
struct Certificate;
impl LocalCertificateController for Certificate {
    fn is_installed(&self) -> anyhow::Result<bool> {
        Ok(true)
    }
    fn install(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
struct Sidecar {
    calls: Arc<Mutex<Vec<&'static str>>>,
}
#[async_trait]
impl CaptureSidecar for Sidecar {
    fn proxy_address(&self) -> ProxyAddress {
        ProxyAddress {
            host: "127.0.0.1".into(),
            port: 2023,
        }
    }
    async fn start(&mut self) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push("start");
        Ok(())
    }
    async fn stop(&mut self) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push("stop");
        Ok(())
    }
}

#[tokio::test]
async fn runtime_starts_sidecar_before_proxy_and_restores_before_stopping() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let proxy = Proxy(Arc::clone(&calls));
    let sidecar = Sidecar {
        calls: Arc::clone(&calls),
    };
    let mut runtime = WeChatCaptureRuntime::new(proxy, Certificate, sidecar);
    runtime
        .start(ProxyConsent {
            install_local_ca: true,
            intercept_wechat_traffic: true,
        })
        .await
        .unwrap();
    runtime.stop().await.unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["start", "set", "restore", "stop"]
    );
}
