use async_trait::async_trait;
use chrono::Utc;
use live_archive_core::{
    credentials::{CredentialStore, MemoryCredentialStore, credential_key},
    douyin::{DouyinLookup, DouyinResolver},
    model::{Platform, RecordingTask, ResolvedStream, TaskStatus},
    platform::{ResolveError, StreamResolver},
    wechat_proxy::{
        CommandRunner, GnomeProxyController, LinuxUpdateCaCertificateController,
        LocalCaCertificate, LocalCertificateController, MacOsNetworkSetupProxyController,
        MacOsSecurityCertificateController, ProxyAddress, ProxyCaptureState, ProxyConsent,
        ProxySnapshot, SystemProxyController, WeChatCaptureCoordinator, WeChatProxySession,
        WindowsCertUtilCertificateController, WindowsInternetSettingsProxyController,
        active_macos_network_service_with,
    },
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use url::Url;
use uuid::Uuid;

fn task() -> RecordingTask {
    let now = Utc::now();
    RecordingTask {
        id: Uuid::new_v4(),
        platform: Platform::Douyin,
        room_url: Url::parse("https://live.douyin.com/123").unwrap(),
        display_name: "creator".into(),
        status: TaskStatus::Idle,
        output_dir: PathBuf::from("recordings"),
        created_at: now,
        updated_at: now,
    }
}

struct FixtureLookup;
#[async_trait]
impl DouyinLookup for FixtureLookup {
    async fn resolve_authorized(
        &self,
        _room_url: &Url,
        cookie: &str,
    ) -> Result<ResolvedStream, ResolveError> {
        if cookie != "session=authorized" {
            return Err(ResolveError::AuthenticationRequired);
        }
        Ok(ResolvedStream {
            stream_url: Url::parse("https://media.example.test/live.m3u8").unwrap(),
            title: "fixture live".into(),
            started_at: Utc::now(),
        })
    }
}

#[tokio::test]
async fn douyin_resolver_requires_and_uses_saved_session() {
    let credentials = Arc::new(MemoryCredentialStore::default());
    let resolver = DouyinResolver::new("default", credentials.clone(), Arc::new(FixtureLookup));
    assert!(matches!(
        resolver.resolve(&task()).await,
        Err(ResolveError::AuthenticationRequired)
    ));
    credentials
        .put(
            &credential_key(Platform::Douyin, "default"),
            "session=authorized",
        )
        .unwrap();
    assert_eq!(
        resolver.resolve(&task()).await.unwrap().title,
        "fixture live"
    );
}

#[derive(Default)]
struct FakeProxy {
    current: Mutex<ProxySnapshot>,
    restored: Mutex<Vec<ProxySnapshot>>,
}
impl SystemProxyController for FakeProxy {
    fn current_proxy(&self) -> anyhow::Result<ProxySnapshot> {
        Ok(self.current.lock().unwrap().clone())
    }
    fn set_proxy(&self, proxy: &ProxyAddress) -> anyhow::Result<()> {
        *self.current.lock().unwrap() = ProxySnapshot {
            web: Some(proxy.clone()),
            secure_web: Some(proxy.clone()),
        };
        Ok(())
    }
    fn restore_proxy(&self, original: &ProxySnapshot) -> anyhow::Result<()> {
        let value = original.clone();
        *self.current.lock().unwrap() = value.clone();
        self.restored.lock().unwrap().push(value);
        Ok(())
    }
}

#[test]
fn wechat_proxy_requires_consent_and_restores_prior_setting() {
    let controller = FakeProxy {
        current: Mutex::new(ProxySnapshot {
            web: Some(ProxyAddress {
                host: "corp-proxy".into(),
                port: 8080,
            }),
            secure_web: None,
        }),
        ..Default::default()
    };
    let session = WeChatProxySession::new(controller);
    let target = ProxyAddress {
        host: "127.0.0.1".into(),
        port: 2023,
    };
    assert!(
        session
            .start(
                ProxyConsent {
                    install_local_ca: false,
                    intercept_wechat_traffic: true
                },
                target.clone()
            )
            .is_err()
    );
    session
        .start(
            ProxyConsent {
                install_local_ca: true,
                intercept_wechat_traffic: true,
            },
            target,
        )
        .unwrap();
    assert_eq!(session.state(), ProxyCaptureState::Active);
    session.stop().unwrap();
    assert_eq!(session.state(), ProxyCaptureState::Idle);
}

#[derive(Clone)]
struct DropProxy(Arc<Mutex<usize>>);

impl SystemProxyController for DropProxy {
    fn current_proxy(&self) -> anyhow::Result<ProxySnapshot> {
        Ok(ProxySnapshot::default())
    }
    fn set_proxy(&self, _proxy: &ProxyAddress) -> anyhow::Result<()> {
        Ok(())
    }
    fn restore_proxy(&self, _proxy: &ProxySnapshot) -> anyhow::Result<()> {
        *self.0.lock().unwrap() += 1;
        Ok(())
    }
}

#[test]
fn active_wechat_proxy_session_restores_proxy_when_dropped() {
    let restores = Arc::new(Mutex::new(0));
    {
        let session = WeChatProxySession::new(DropProxy(Arc::clone(&restores)));
        session
            .start(
                ProxyConsent {
                    install_local_ca: true,
                    intercept_wechat_traffic: true,
                },
                ProxyAddress {
                    host: "127.0.0.1".into(),
                    port: 2023,
                },
            )
            .unwrap();
    }
    assert_eq!(*restores.lock().unwrap(), 1);
}

#[derive(Default)]
struct FakeRunner(Mutex<Vec<Vec<String>>>);
impl CommandRunner for FakeRunner {
    fn run(&self, _program: &str, arguments: &[&str]) -> anyhow::Result<String> {
        self.0
            .lock()
            .unwrap()
            .push(arguments.iter().map(|value| value.to_string()).collect());
        match arguments[0] {
            "-getwebproxy" => Ok("Enabled: Yes\nServer: old-web\nPort: 3128\n".into()),
            "-getsecurewebproxy" => Ok("Enabled: No\nServer: \nPort: 0\n".into()),
            _ => Ok(String::new()),
        }
    }
}

#[test]
fn macos_proxy_controller_preserves_distinct_web_and_secure_proxy_settings() {
    let runner = FakeRunner::default();
    let controller = MacOsNetworkSetupProxyController::with_runner("Wi-Fi", runner);
    let original = controller.current_proxy().unwrap();
    assert_eq!(original.web.as_ref().unwrap().host, "old-web");
    assert!(original.secure_web.is_none());
    controller.restore_proxy(&original).unwrap();
}

struct MacServiceRunner;
impl CommandRunner for MacServiceRunner {
    fn run(&self, program: &str, _arguments: &[&str]) -> anyhow::Result<String> {
        match program {
            "route" => Ok("   route to: default\ninterface: en7\n".into()),
            "networksetup" => Ok("(1) Wi-Fi\n(Hardware Port: Wi-Fi, Device: en0)\n(2) USB Ethernet\n(Hardware Port: USB 10/100/1000 LAN, Device: en7)\n".into()),
            _ => unreachable!(),
        }
    }
}

#[test]
fn macos_network_service_is_selected_from_the_default_route_interface() {
    assert_eq!(
        active_macos_network_service_with(&MacServiceRunner).unwrap(),
        "USB Ethernet"
    );
}

struct KeychainRecordingRunner(Arc<Mutex<Vec<Vec<String>>>>);
impl CommandRunner for KeychainRecordingRunner {
    fn run(&self, program: &str, arguments: &[&str]) -> anyhow::Result<String> {
        let mut command = vec![program.to_owned()];
        command.extend(arguments.iter().map(|argument| (*argument).to_owned()));
        self.0.lock().unwrap().push(command);
        if arguments[0] == "default-keychain" {
            return Ok("\"/synthetic/login.keychain-db\"\n".into());
        }
        if arguments[0] == "find-certificate" {
            return Ok("-----BEGIN CERTIFICATE-----\nCURRENT\n-----END CERTIFICATE-----\n".into());
        }
        Ok(String::new())
    }
}

#[test]
fn macos_certificate_controller_installs_in_the_user_default_keychain() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ca.pem");
    std::fs::write(
        &path,
        "-----BEGIN CERTIFICATE-----\nCURRENT\n-----END CERTIFICATE-----\n",
    )
    .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let controller = MacOsSecurityCertificateController::with_runner(
        LocalCaCertificate {
            common_name: "mitmproxy".into(),
            certificate_path: path.clone(),
        },
        KeychainRecordingRunner(Arc::clone(&calls)),
    );
    controller.install().unwrap();
    assert_eq!(
        calls.lock().unwrap()[1],
        [
            "security",
            "add-trusted-cert",
            "-r",
            "trustRoot",
            "-p",
            "ssl",
            "-p",
            "basic",
            "-k",
            "/synthetic/login.keychain-db",
            path.to_str().unwrap()
        ]
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>()
    );
}

struct WindowsRunner;
impl CommandRunner for WindowsRunner {
    fn run(&self, _program: &str, arguments: &[&str]) -> anyhow::Result<String> {
        if arguments.last() == Some(&"ProxyEnable") {
            return Ok("ProxyEnable    REG_DWORD    0x1\n".into());
        }
        if arguments.last() == Some(&"ProxyServer") {
            return Ok("ProxyServer    REG_SZ    http=proxy.example.test:8080;https=secure.example.test:8443\n".into());
        }
        Ok(String::new())
    }
}

#[test]
fn windows_proxy_accepts_a_fresh_profile_without_proxy_values() {
    struct FreshProfile;
    impl CommandRunner for FreshProfile {
        fn run(&self, _: &str, arguments: &[&str]) -> anyhow::Result<String> {
            if arguments.contains(&"/v") {
                anyhow::bail!("value not found");
            }
            Ok("HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings".into())
        }
    }
    let controller = WindowsInternetSettingsProxyController::with_runner(FreshProfile);
    assert_eq!(
        controller.current_proxy().unwrap(),
        ProxySnapshot::default()
    );
}

#[test]
fn windows_proxy_does_not_ignore_an_unreadable_registry_key() {
    struct Denied;
    impl CommandRunner for Denied {
        fn run(&self, _: &str, _: &[&str]) -> anyhow::Result<String> {
            anyhow::bail!("access denied");
        }
    }
    assert!(
        WindowsInternetSettingsProxyController::with_runner(Denied)
            .current_proxy()
            .is_err()
    );
}

#[test]
fn windows_proxy_controller_reads_current_user_proxy() {
    let controller = WindowsInternetSettingsProxyController::with_runner(WindowsRunner);
    let proxy = controller.current_proxy().unwrap().web.unwrap();
    assert_eq!(proxy.host, "proxy.example.test");
    assert_eq!(proxy.port, 8080);
    assert_eq!(
        controller.current_proxy().unwrap().secure_web.unwrap().port,
        8443
    );
}

struct GnomeRunner;
impl CommandRunner for GnomeRunner {
    fn run(&self, _program: &str, arguments: &[&str]) -> anyhow::Result<String> {
        match (arguments.get(1), arguments.get(2)) {
            (Some(&"org.gnome.system.proxy"), Some(&"mode")) => Ok("'manual'\n".into()),
            (Some(&"org.gnome.system.proxy.http"), Some(&"host")) => Ok("'old-http'\n".into()),
            (Some(&"org.gnome.system.proxy.http"), Some(&"port")) => Ok("3128\n".into()),
            (Some(&"org.gnome.system.proxy.https"), Some(&"host")) => Ok("'old-https'\n".into()),
            (Some(&"org.gnome.system.proxy.https"), Some(&"port")) => Ok("8443\n".into()),
            _ => Ok(String::new()),
        }
    }
}

#[test]
fn gnome_proxy_controller_reads_manual_proxy_only() {
    let controller = GnomeProxyController::with_runner(GnomeRunner);
    let snapshot = controller.current_proxy().unwrap();
    assert_eq!(snapshot.web.unwrap().port, 3128);
    assert_eq!(snapshot.secure_web.unwrap().port, 8443);
}

struct CertificateRunner;
impl CommandRunner for CertificateRunner {
    fn run(&self, program: &str, arguments: &[&str]) -> anyhow::Result<String> {
        if program == "certutil" && arguments == ["-user", "-store", "Root"] {
            return Ok("Subject: CN=Live Archive Local CA\n".into());
        }
        Ok(String::new())
    }
}

#[test]
fn windows_certificate_controller_detects_user_store_entry() {
    let certificate = LocalCaCertificate {
        common_name: "Live Archive Local CA".into(),
        certificate_path: PathBuf::from("test-ca.cer"),
    };
    let controller =
        WindowsCertUtilCertificateController::with_runner(certificate, CertificateRunner);
    assert!(controller.is_installed().unwrap());
}

#[derive(Default)]
struct RecordingRunner(Mutex<Vec<(String, Vec<String>)>>);
impl CommandRunner for RecordingRunner {
    fn run(&self, program: &str, arguments: &[&str]) -> anyhow::Result<String> {
        self.0.lock().unwrap().push((
            program.into(),
            arguments.iter().map(|value| value.to_string()).collect(),
        ));
        Ok(String::new())
    }
}

#[test]
fn linux_certificate_controller_uses_native_privilege_prompt() {
    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("missing-ca.crt");
    let runner = RecordingRunner::default();
    let controller = LinuxUpdateCaCertificateController::with_runner(
        LocalCaCertificate {
            common_name: "Live Archive Local CA".into(),
            certificate_path: temporary.path().join("source.pem"),
        },
        target,
        runner,
    );
    controller.install().unwrap();
}

#[derive(Default)]
struct FakeCertificate(Mutex<usize>);

impl LocalCertificateController for FakeCertificate {
    fn is_installed(&self) -> anyhow::Result<bool> {
        Ok(*self.0.lock().unwrap() > 0)
    }
    fn install(&self) -> anyhow::Result<()> {
        *self.0.lock().unwrap() += 1;
        Ok(())
    }
}

#[test]
fn coordinator_installs_ca_only_after_explicit_consent() {
    let coordinator =
        WeChatCaptureCoordinator::new(FakeProxy::default(), FakeCertificate::default());
    let target = ProxyAddress {
        host: "127.0.0.1".into(),
        port: 2023,
    };
    assert!(
        coordinator
            .start(
                ProxyConsent {
                    install_local_ca: false,
                    intercept_wechat_traffic: true
                },
                target.clone()
            )
            .is_err()
    );
    coordinator
        .start(
            ProxyConsent {
                install_local_ca: true,
                intercept_wechat_traffic: true,
            },
            target,
        )
        .unwrap();
    assert_eq!(coordinator.state(), ProxyCaptureState::Active);
}
