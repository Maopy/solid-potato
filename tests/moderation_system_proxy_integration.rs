//! Fake OS commands exercise native controllers and rollback without changing real settings.
use live_archive_core::{
    moderation::{config::ModerationConfig, inject::ignore_hosts, protocol::ProbeScope},
    wechat_proxy::{
        CommandRunner, LocalCaCertificate, MacOsNetworkSetupProxyController,
        MacOsSecurityCertificateController, ProxyAddress, ProxyCaptureState, ProxyConsent,
        ProxySnapshot, SystemProxyController, WeChatCaptureCoordinator, WeChatProxySession,
    },
};
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct State {
    calls: Vec<(String, Vec<String>)>,
    fail_secure_once: bool,
    certificates: String,
    trusted: bool,
    ignore_install: bool,
    directory: Option<tempfile::TempDir>,
}
#[derive(Clone, Default)]
struct Commands(Arc<Mutex<State>>);
impl CommandRunner for Commands {
    fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<String> {
        let mut state = self.0.lock().unwrap();
        state
            .calls
            .push((program.into(), args.iter().map(|s| s.to_string()).collect()));
        if args[0] == "find-certificate" {
            return Ok(state.certificates.clone());
        }
        if args[0] == "default-keychain" {
            return Ok("\"/synthetic/login.keychain-db\"\n".into());
        }
        if args[0] == "add-trusted-cert" && !state.ignore_install {
            state.certificates = std::fs::read_to_string(args.last().unwrap())?;
            state.trusted = true;
        }
        if args[0] == "verify-cert" && !state.trusted {
            anyhow::bail!("this exact CA is not trusted");
        }
        if args[0] == "-getwebproxy" {
            return Ok("Enabled: Yes\nServer: old-http\nPort: 8080\n".into());
        }
        if args[0] == "-getsecurewebproxy" {
            return Ok("Enabled: Yes\nServer: old-https\nPort: 8443\n".into());
        }
        if args[0] == "-setsecurewebproxy"
            && args.get(2) == Some(&"127.0.0.1")
            && state.fail_secure_once
        {
            state.fail_secure_once = false;
            anyhow::bail!("simulated partial proxy failure");
        }
        Ok(String::new())
    }
}
fn coordinator(
    commands: Commands,
) -> WeChatCaptureCoordinator<
    MacOsNetworkSetupProxyController<Commands>,
    MacOsSecurityCertificateController<Commands>,
> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ca.pem");
    std::fs::write(
        &path,
        "-----BEGIN CERTIFICATE-----\nCURRENT\n-----END CERTIFICATE-----\n",
    )
    .unwrap();
    commands.0.lock().unwrap().directory = Some(directory);
    WeChatCaptureCoordinator::new(
        MacOsNetworkSetupProxyController::with_runner("Wi-Fi", commands.clone()),
        MacOsSecurityCertificateController::with_runner(
            LocalCaCertificate {
                common_name: "mitmproxy".into(),
                certificate_path: path,
            },
            commands,
        ),
    )
}
const CONSENT: ProxyConsent = ProxyConsent {
    install_local_ca: true,
    intercept_wechat_traffic: true,
};

#[test]
fn native_start_installs_current_ca_when_another_same_name_ca_is_trusted() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mitmproxy-ca-cert.pem");
    std::fs::write(
        &path,
        "-----BEGIN CERTIFICATE-----\nCURRENT\n-----END CERTIFICATE-----\n",
    )
    .unwrap();
    let commands = Commands::default();
    {
        let mut state = commands.0.lock().unwrap();
        state.certificates =
            "-----BEGIN CERTIFICATE-----\nOTHER\n-----END CERTIFICATE-----\n".into();
        state.trusted = true; // Old verify-only detection would incorrectly accept this.
    }
    let certificate = || {
        MacOsSecurityCertificateController::with_runner(
            LocalCaCertificate {
                common_name: "mitmproxy".into(),
                certificate_path: path.clone(),
            },
            commands.clone(),
        )
    };
    {
        let coordinator = WeChatCaptureCoordinator::new(
            MacOsNetworkSetupProxyController::with_runner("Wi-Fi", commands.clone()),
            certificate(),
        );
        coordinator.start(CONSENT, address()).unwrap();
    }
    {
        let mut state = commands.0.lock().unwrap();
        assert!(
            state
                .calls
                .iter()
                .any(|(_, args)| args[0] == "add-trusted-cert")
        );
        let install = state
            .calls
            .iter()
            .position(|(_, args)| args[0] == "add-trusted-cert")
            .unwrap();
        assert!(
            !state.calls[..install]
                .iter()
                .any(|(_, args)| args[0] == "verify-cert")
        );
        state.certificates =
            "-----BEGIN CERTIFICATE-----\nCUR RENT\n-----END CERTIFICATE-----\n".into();
        state.calls.clear();
    }
    {
        let coordinator = WeChatCaptureCoordinator::new(
            MacOsNetworkSetupProxyController::with_runner("Wi-Fi", commands.clone()),
            certificate(),
        );
        coordinator.start(CONSENT, address()).unwrap();
    }
    let state = commands.0.lock().unwrap();
    assert!(state.calls.iter().any(|(_, args)| args[0] == "verify-cert"));
    assert!(
        !state
            .calls
            .iter()
            .any(|(_, args)| args[0] == "add-trusted-cert")
    );
}
fn address() -> ProxyAddress {
    ProxyAddress {
        host: "127.0.0.1".into(),
        port: 2024,
    }
}
#[test]
fn native_ca_install_without_actual_keychain_import_never_enables_proxy() {
    let commands = Commands::default();
    commands.0.lock().unwrap().ignore_install = true;
    let coordinator = coordinator(commands.clone());
    let error = coordinator.start(CONSENT, address()).unwrap_err();
    assert!(error.to_string().contains("after installation"));
    let state = commands.0.lock().unwrap();
    assert!(
        state
            .calls
            .iter()
            .all(|(program, _)| program != "networksetup")
    );
}
#[test]
fn yaml_targets_wechat_tls_and_native_lifecycle_restores_original_settings_on_drop() {
    let config = ModerationConfig::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("moderation.wechat.example.yml"),
    )
    .unwrap();
    assert!(config.probe_options().system_proxy);
    let scope = ProbeScope::from_room_url(config.room.url).unwrap();
    // mitmproxy uses Python regex syntax (lookahead is unsupported by Rust regex).
    let result = std::process::Command::new("python3")
        .args(["-c", "import re,sys,json; print(json.dumps([bool(re.match(sys.argv[1], host)) for host in sys.argv[2:]]))"])
        .arg(ignore_hosts(&scope).unwrap())
        .args(["channels.weixin.qq.com:443", "live.douyin.com:443", "media.wxlivecdn.com:443"])
        .output().unwrap();
    assert!(result.status.success());
    let bypass: Vec<bool> = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(bypass, vec![false, true, true]);
    let commands = Commands::default();
    {
        let coordinator = coordinator(commands.clone());
        coordinator.start(CONSENT, address()).unwrap();
    }
    let state = commands.0.lock().unwrap();
    let calls = &state.calls;
    assert_eq!(
        calls[0].1,
        vec!["find-certificate", "-a", "-c", "mitmproxy", "-p"]
    );
    assert_eq!(calls[1].1, vec!["default-keychain", "-d", "user"]);
    assert_eq!(
        &calls[2].1[..9],
        &[
            "add-trusted-cert",
            "-r",
            "trustRoot",
            "-p",
            "ssl",
            "-p",
            "basic",
            "-k",
            "/synthetic/login.keychain-db"
        ]
    );
    assert!(
        calls
            .iter()
            .any(|(_, a)| a == &vec!["-setwebproxy", "Wi-Fi", "127.0.0.1", "2024"])
    );
    assert!(
        calls
            .iter()
            .any(|(_, a)| a == &vec!["-setwebproxy", "Wi-Fi", "old-http", "8080"])
    );
    assert!(
        calls
            .iter()
            .any(|(_, a)| a == &vec!["-setsecurewebproxy", "Wi-Fi", "old-https", "8443"])
    );
}
#[test]
fn partial_native_setup_failure_rolls_back_both_proxy_kinds() {
    let commands = Commands::default();
    commands.0.lock().unwrap().fail_secure_once = true;
    let coordinator = coordinator(commands.clone());
    assert!(coordinator.start(CONSENT, address()).is_err());
    assert_eq!(coordinator.state(), ProxyCaptureState::Idle);
    let state = commands.0.lock().unwrap();
    assert!(
        state
            .calls
            .iter()
            .any(|(_, a)| a == &vec!["-setwebproxy", "Wi-Fi", "old-http", "8080"])
    );
    assert!(
        state
            .calls
            .iter()
            .any(|(_, a)| a == &vec!["-setsecurewebproxy", "Wi-Fi", "old-https", "8443"])
    );
}

#[test]
fn passthrough_configuration_and_native_lifecycle_do_not_inspect_or_install_ca() {
    let config = ModerationConfig::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("moderation.wechat.passthrough.example.yml"),
    )
    .unwrap();
    assert!(config.probe_options().tls_passthrough);
    let commands = Commands::default();
    let proxy = MacOsNetworkSetupProxyController::with_runner("Wi-Fi", commands.clone());
    let certificate = MacOsSecurityCertificateController::with_runner(
        LocalCaCertificate {
            common_name: "mitmproxy".into(),
            certificate_path: "/synthetic/ca.pem".into(),
        },
        commands.clone(),
    );
    let lease = live_archive_core::moderation::system_proxy::start_with_controllers(
        proxy,
        certificate,
        2024,
        false,
    )
    .unwrap();
    lease.stop().unwrap();
    let state = commands.0.lock().unwrap();
    assert!(
        state
            .calls
            .iter()
            .all(|(program, _)| program == "networksetup")
    );
    assert!(
        state
            .calls
            .iter()
            .any(|(_, a)| a == &vec!["-setwebproxy", "Wi-Fi", "old-http", "8080"])
    );
}
#[derive(Clone)]
struct RetryProxy {
    snapshots: Arc<Mutex<Vec<ProxySnapshot>>>,
}
impl SystemProxyController for RetryProxy {
    fn current_proxy(&self) -> anyhow::Result<ProxySnapshot> {
        Ok(ProxySnapshot {
            web: Some(ProxyAddress {
                host: "original".into(),
                port: 3128,
            }),
            secure_web: None,
        })
    }
    fn set_proxy(&self, _: &ProxyAddress) -> anyhow::Result<()> {
        Ok(())
    }
    fn restore_proxy(&self, snapshot: &ProxySnapshot) -> anyhow::Result<()> {
        let mut history = self.snapshots.lock().unwrap();
        history.push(snapshot.clone());
        if history.len() == 1 {
            anyhow::bail!("temporary restore failure");
        }
        Ok(())
    }
}
#[test]
fn restoration_failure_preserves_snapshot_for_retry_instead_of_disabling_previous_proxy() {
    let history = Arc::new(Mutex::new(Vec::new()));
    let session = WeChatProxySession::new(RetryProxy {
        snapshots: history.clone(),
    });
    session.start(CONSENT, address()).unwrap();
    assert!(session.stop().is_err());
    assert_eq!(session.state(), ProxyCaptureState::Active);
    session.stop().unwrap();
    assert_eq!(session.state(), ProxyCaptureState::Idle);
    let snapshots = history.lock().unwrap();
    assert_eq!(snapshots[0], snapshots[1]);
    assert_eq!(snapshots[1].web.as_ref().unwrap().host, "original");
}
