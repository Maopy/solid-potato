//! Explicit-consent lifecycle for a local WeChat Channels capture proxy.
//!
//! This module intentionally does not silently install a CA certificate or
//! change a system proxy. OS-specific implementations are injected by the
//! desktop shell and must prompt through the native privilege mechanism.
use anyhow::{Context, Result, bail};
use std::{path::PathBuf, process::Command, sync::Mutex};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProxyAddress {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProxySnapshot {
    pub web: Option<ProxyAddress>,
    pub secure_web: Option<ProxyAddress>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyConsent {
    pub install_local_ca: bool,
    pub intercept_wechat_traffic: bool,
}

pub trait SystemProxyController: Send + Sync {
    fn current_proxy(&self) -> Result<ProxySnapshot>;
    fn set_proxy(&self, proxy: &ProxyAddress) -> Result<()>;
    fn restore_proxy(&self, original: &ProxySnapshot) -> Result<()>;
}

/// Platform shell supplies this implementation. It must use the native
/// privilege prompt and install only an application-generated local CA.
pub trait LocalCertificateController: Send + Sync {
    fn is_installed(&self) -> Result<bool>;
    fn install(&self) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct LocalCaCertificate {
    /// The app-generated CA's Common Name, used only for lookup/display.
    pub common_name: String,
    /// DER or PEM certificate; private keys must never be passed to this API.
    pub certificate_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyCaptureState {
    Idle,
    Active,
}

/// Owns one explicit capture session. `stop` is idempotent; callers should also
/// invoke it during application shutdown to restore the prior proxy setting.
pub struct WeChatProxySession<C: SystemProxyController> {
    controller: C,
    original: Mutex<Option<ProxySnapshot>>,
    state: Mutex<ProxyCaptureState>,
}

impl<C: SystemProxyController> WeChatProxySession<C> {
    pub fn new(controller: C) -> Self {
        Self {
            controller,
            original: Mutex::new(None),
            state: Mutex::new(ProxyCaptureState::Idle),
        }
    }

    pub fn start(&self, consent: ProxyConsent, local_proxy: ProxyAddress) -> Result<()> {
        if !consent.install_local_ca || !consent.intercept_wechat_traffic {
            bail!(
                "WeChat capture requires explicit consent for its local CA and traffic interception"
            );
        }
        let mut state = self.state.lock().unwrap();
        if *state == ProxyCaptureState::Active {
            return Ok(());
        }
        let original = self.controller.current_proxy()?;
        *self.original.lock().unwrap() = Some(original.clone());
        *state = ProxyCaptureState::Active;
        if let Err(error) = self.controller.set_proxy(&local_proxy) {
            match self.controller.restore_proxy(&original) {
                Ok(()) => {
                    self.original.lock().unwrap().take();
                    *state = ProxyCaptureState::Idle;
                    return Err(error);
                }
                Err(rollback) => {
                    return Err(error.context(format!("proxy rollback failed: {rollback}")));
                }
            }
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if *state == ProxyCaptureState::Idle {
            return Ok(());
        }
        let original = self.original.lock().unwrap().clone().unwrap_or_default();
        self.controller.restore_proxy(&original)?;
        self.original.lock().unwrap().take();
        *state = ProxyCaptureState::Idle;
        Ok(())
    }

    pub fn state(&self) -> ProxyCaptureState {
        *self.state.lock().unwrap()
    }
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, arguments: &[&str]) -> Result<String>;
}

pub struct ProcessCommandRunner;

impl CommandRunner for ProcessCommandRunner {
    fn run(&self, program: &str, arguments: &[&str]) -> Result<String> {
        let output = Command::new(program)
            .args(arguments)
            .output()
            .with_context(|| format!("starting {program}"))?;
        if !output.status.success() {
            bail!(
                "{program} exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        String::from_utf8(output.stdout).context("reading command output as UTF-8")
    }
}

/// macOS Keychain implementation. The local CA belongs in the current user's
/// login keychain so capture does not require machine-wide administrator trust.
pub struct MacOsSecurityCertificateController<R: CommandRunner = ProcessCommandRunner> {
    certificate: LocalCaCertificate,
    runner: R,
}
impl MacOsSecurityCertificateController<ProcessCommandRunner> {
    pub fn new(certificate: LocalCaCertificate) -> Self {
        Self {
            certificate,
            runner: ProcessCommandRunner,
        }
    }
}
impl<R: CommandRunner> MacOsSecurityCertificateController<R> {
    pub fn with_runner(certificate: LocalCaCertificate, runner: R) -> Self {
        Self {
            certificate,
            runner,
        }
    }
}
impl<R: CommandRunner> LocalCertificateController for MacOsSecurityCertificateController<R> {
    fn is_installed(&self) -> Result<bool> {
        // verify-cert with a CA leaf is not an identity check: it can succeed
        // even when this directory's certificate is absent from the keychain.
        // Compare complete public certificates before evaluating trust.
        let installed = match self.runner.run(
            "security",
            &[
                "find-certificate",
                "-a",
                "-c",
                &self.certificate.common_name,
                "-p",
            ],
        ) {
            Ok(value) => value,
            Err(_) => return Ok(false),
        };
        if installed.trim().is_empty() {
            return Ok(false);
        }
        let expected = std::fs::read_to_string(&self.certificate.certificate_path)
            .context("reading public proxy CA certificate")?;
        let certificates = |pem: &str| -> Vec<String> {
            pem.split("-----BEGIN CERTIFICATE-----")
                .skip(1)
                .filter_map(|part| part.split_once("-----END CERTIFICATE-----"))
                .map(|(body, _)| body.chars().filter(|c| !c.is_whitespace()).collect())
                .filter(|body: &String| !body.is_empty())
                .collect()
        };
        let expected = certificates(&expected);
        if expected.len() != 1 || !certificates(&installed).contains(&expected[0]) {
            return Ok(false);
        }
        Ok(self
            .runner
            .run(
                "security",
                &[
                    "verify-cert",
                    "-L",
                    "-l",
                    "-c",
                    &self.certificate.certificate_path.to_string_lossy(),
                ],
            )
            .is_ok())
    }
    fn install(&self) -> Result<()> {
        let path = self.certificate.certificate_path.to_string_lossy();
        let keychain = self
            .runner
            .run("security", &["default-keychain", "-d", "user"])?;
        let keychain = keychain.trim().trim_matches('"');
        if !std::path::Path::new(keychain).is_absolute() || keychain.contains(['\n', '\r']) {
            bail!("cannot determine the current user's default keychain");
        }
        self.runner.run(
            "security",
            &[
                "add-trusted-cert",
                "-r",
                "trustRoot",
                "-p",
                "ssl",
                "-p",
                "basic",
                "-k",
                keychain,
                &path,
            ],
        )?;
        if !self.is_installed()? {
            bail!(
                "current proxy CA was not found or trusted after installation; system proxy was not enabled"
            );
        }
        Ok(())
    }
}

/// Windows Current User root-store implementation. It intentionally uses the
/// user store (`-user`) rather than a machine-wide store.
pub struct WindowsCertUtilCertificateController<R: CommandRunner = ProcessCommandRunner> {
    certificate: LocalCaCertificate,
    runner: R,
}

/// Debian/Ubuntu-style system trust-store controller. It uses `pkexec` so the
/// desktop environment, not this process, presents the privilege prompt.
pub struct LinuxUpdateCaCertificateController<R: CommandRunner = ProcessCommandRunner> {
    certificate: LocalCaCertificate,
    installed_path: PathBuf,
    runner: R,
}
impl LinuxUpdateCaCertificateController<ProcessCommandRunner> {
    pub fn new(certificate: LocalCaCertificate, installed_path: PathBuf) -> Self {
        Self {
            certificate,
            installed_path,
            runner: ProcessCommandRunner,
        }
    }
}
impl<R: CommandRunner> LinuxUpdateCaCertificateController<R> {
    pub fn with_runner(
        certificate: LocalCaCertificate,
        installed_path: PathBuf,
        runner: R,
    ) -> Self {
        Self {
            certificate,
            installed_path,
            runner,
        }
    }
}
impl<R: CommandRunner> LocalCertificateController for LinuxUpdateCaCertificateController<R> {
    fn is_installed(&self) -> Result<bool> {
        Ok(self.installed_path.exists())
    }
    fn install(&self) -> Result<()> {
        let source = self.certificate.certificate_path.to_string_lossy();
        let target = self.installed_path.to_string_lossy();
        self.runner
            .run("pkexec", &["install", "-m", "0644", &source, &target])?;
        self.runner.run("pkexec", &["update-ca-certificates"])?;
        Ok(())
    }
}
impl WindowsCertUtilCertificateController<ProcessCommandRunner> {
    pub fn new(certificate: LocalCaCertificate) -> Self {
        Self {
            certificate,
            runner: ProcessCommandRunner,
        }
    }
}
impl<R: CommandRunner> WindowsCertUtilCertificateController<R> {
    pub fn with_runner(certificate: LocalCaCertificate, runner: R) -> Self {
        Self {
            certificate,
            runner,
        }
    }
}
impl<R: CommandRunner> LocalCertificateController for WindowsCertUtilCertificateController<R> {
    fn is_installed(&self) -> Result<bool> {
        let output = self.runner.run("certutil", &["-user", "-store", "Root"])?;
        Ok(output
            .lines()
            .any(|line| line.contains(&self.certificate.common_name)))
    }
    fn install(&self) -> Result<()> {
        let path = self.certificate.certificate_path.to_string_lossy();
        self.runner
            .run("certutil", &["-user", "-addstore", "Root", &path])?;
        Ok(())
    }
}

/// Resolves the network service associated with macOS's current default route.
/// This avoids assuming that a user's Wi-Fi service has the default name.
pub fn active_macos_network_service() -> Result<String> {
    active_macos_network_service_with(&ProcessCommandRunner)
}

pub fn active_macos_network_service_with<R: CommandRunner>(runner: &R) -> Result<String> {
    let route = runner.run("route", &["-n", "get", "default"])?;
    let interface = route
        .lines()
        .find_map(|line| line.trim().strip_prefix("interface: "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("macOS default route did not report a network interface")?;
    let services = runner.run("networksetup", &["-listnetworkserviceorder"])?;
    network_service_for_interface(&services, interface).with_context(|| {
        format!("no macOS network service is associated with default-route interface {interface}")
    })
}

fn network_service_for_interface(services: &str, interface: &str) -> Option<String> {
    let mut current_service = None;
    for line in services.lines() {
        let line = line.trim();
        if line.starts_with('(') && !line.starts_with("(Hardware Port:") {
            current_service = line
                .find(')')
                .map(|index| {
                    line[index + 1..]
                        .trim()
                        .trim_start_matches('*')
                        .trim()
                        .to_owned()
                })
                .filter(|service| !service.is_empty());
            continue;
        }
        if let Some(device) = line
            .split("Device: ")
            .nth(1)
            .and_then(|value| value.strip_suffix(')'))
            .map(str::trim)
            && device == interface
        {
            return current_service;
        }
    }
    None
}

/// macOS implementation based on `networksetup`. The service is detected from
/// the default route before this controller is created.
pub struct MacOsNetworkSetupProxyController<R: CommandRunner = ProcessCommandRunner> {
    service: String,
    runner: R,
}

impl MacOsNetworkSetupProxyController<ProcessCommandRunner> {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            runner: ProcessCommandRunner,
        }
    }
}

impl<R: CommandRunner> MacOsNetworkSetupProxyController<R> {
    pub fn with_runner(service: impl Into<String>, runner: R) -> Self {
        Self {
            service: service.into(),
            runner,
        }
    }

    fn read_proxy(&self, command: &str) -> Result<Option<ProxyAddress>> {
        let output = self.runner.run("networksetup", &[command, &self.service])?;
        parse_networksetup_proxy(&output)
    }

    fn set_kind(
        &self,
        command: &str,
        state_command: &str,
        value: Option<&ProxyAddress>,
    ) -> Result<()> {
        match value {
            Some(proxy) => {
                self.runner.run(
                    "networksetup",
                    &[command, &self.service, &proxy.host, &proxy.port.to_string()],
                )?;
                self.runner
                    .run("networksetup", &[state_command, &self.service, "on"])?;
            }
            None => {
                self.runner
                    .run("networksetup", &[state_command, &self.service, "off"])?;
            }
        }
        Ok(())
    }
}

impl<R: CommandRunner> SystemProxyController for MacOsNetworkSetupProxyController<R> {
    fn current_proxy(&self) -> Result<ProxySnapshot> {
        Ok(ProxySnapshot {
            web: self.read_proxy("-getwebproxy")?,
            secure_web: self.read_proxy("-getsecurewebproxy")?,
        })
    }
    fn set_proxy(&self, proxy: &ProxyAddress) -> Result<()> {
        self.set_kind("-setwebproxy", "-setwebproxystate", Some(proxy))?;
        self.set_kind("-setsecurewebproxy", "-setsecurewebproxystate", Some(proxy))
    }
    fn restore_proxy(&self, original: &ProxySnapshot) -> Result<()> {
        self.set_kind("-setwebproxy", "-setwebproxystate", original.web.as_ref())?;
        self.set_kind(
            "-setsecurewebproxy",
            "-setsecurewebproxystate",
            original.secure_web.as_ref(),
        )
    }
}

fn parse_networksetup_proxy(output: &str) -> Result<Option<ProxyAddress>> {
    let mut enabled = false;
    let mut host = None;
    let mut port = None;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("Enabled: ") {
            enabled = value.trim().eq_ignore_ascii_case("yes");
        }
        if let Some(value) = line.strip_prefix("Server: ") {
            host = Some(value.trim().to_owned());
        }
        if let Some(value) = line.strip_prefix("Port: ") {
            port = value.trim().parse::<u16>().ok();
        }
    }
    if !enabled {
        return Ok(None);
    }
    match (host.filter(|value| !value.is_empty()), port) {
        (Some(host), Some(port)) => Ok(Some(ProxyAddress { host, port })),
        _ => bail!("networksetup reported an enabled proxy without a server and port"),
    }
}

/// Windows user-level proxy implementation. It controls the same Internet
/// Settings used by many desktop applications; it deliberately does not touch
/// machine-wide WinHTTP settings. The caller must run it only after consent.
pub struct WindowsInternetSettingsProxyController<R: CommandRunner = ProcessCommandRunner> {
    runner: R,
}

impl WindowsInternetSettingsProxyController<ProcessCommandRunner> {
    pub fn new() -> Self {
        Self {
            runner: ProcessCommandRunner,
        }
    }
}

impl<R: CommandRunner> WindowsInternetSettingsProxyController<R> {
    pub fn with_runner(runner: R) -> Self {
        Self { runner }
    }

    fn query_value(&self, value: &str) -> Result<String> {
        self.runner.run(
            "reg",
            &[
                "query",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings",
                "/v",
                value,
            ],
        )
    }

    fn set_value(&self, value: &str, kind: &str, data: &str) -> Result<()> {
        self.runner.run(
            "reg",
            &[
                "add",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings",
                "/v",
                value,
                "/t",
                kind,
                "/d",
                data,
                "/f",
            ],
        )?;
        // Broadcast the change so applications that use WinINet can reload it.
        self.runner.run(
            "rundll32.exe",
            &["user32.dll,UpdatePerUserSystemParameters"],
        )?;
        Ok(())
    }
}

impl<R: CommandRunner> SystemProxyController for WindowsInternetSettingsProxyController<R> {
    fn current_proxy(&self) -> Result<ProxySnapshot> {
        let enabled = parse_windows_registry_value(&self.query_value("ProxyEnable")?)
            .map(|value| value == "0x1" || value == "1")
            .unwrap_or(false);
        let server = parse_windows_registry_value(&self.query_value("ProxyServer")?);
        if !enabled {
            return Ok(ProxySnapshot::default());
        }
        Ok(server
            .map(|value| parse_windows_proxy_servers(&value))
            .unwrap_or_default())
    }

    fn set_proxy(&self, proxy: &ProxyAddress) -> Result<()> {
        self.set_value(
            "ProxyServer",
            "REG_SZ",
            &format!("{}:{}", proxy.host, proxy.port),
        )?;
        self.set_value("ProxyEnable", "REG_DWORD", "1")
    }

    fn restore_proxy(&self, original: &ProxySnapshot) -> Result<()> {
        match (&original.web, &original.secure_web) {
            (None, None) => self.set_value("ProxyEnable", "REG_DWORD", "0"),
            (web, secure_web) => {
                let parts = [
                    web.as_ref()
                        .map(|proxy| format!("http={}:{}", proxy.host, proxy.port)),
                    secure_web
                        .as_ref()
                        .map(|proxy| format!("https={}:{}", proxy.host, proxy.port)),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                self.set_value("ProxyServer", "REG_SZ", &parts.join(";"))?;
                self.set_value("ProxyEnable", "REG_DWORD", "1")
            }
        }
    }
}

fn parse_windows_registry_value(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let _name = fields.next()?;
        let kind = fields.next()?;
        if !kind.starts_with("REG_") {
            return None;
        }
        Some(fields.collect::<Vec<_>>().join(" "))
    })
}

fn parse_windows_proxy_address(value: &str) -> Option<ProxyAddress> {
    let value = value.trim();
    let (host, port) = value.rsplit_once(':')?;
    Some(ProxyAddress {
        host: host.to_owned(),
        port: port.parse().ok()?,
    })
}

fn parse_windows_proxy_servers(value: &str) -> ProxySnapshot {
    if !value.contains('=') {
        let proxy = parse_windows_proxy_address(value);
        return ProxySnapshot {
            web: proxy.clone(),
            secure_web: proxy,
        };
    }
    let mut snapshot = ProxySnapshot::default();
    for item in value.split(';') {
        if let Some((protocol, address)) = item.split_once('=') {
            match protocol.trim().to_ascii_lowercase().as_str() {
                "http" => snapshot.web = parse_windows_proxy_address(address),
                "https" => snapshot.secure_web = parse_windows_proxy_address(address),
                _ => {}
            }
        }
    }
    snapshot
}

/// GNOME/Linux implementation through `gsettings`. Automatic PAC modes are
/// rejected rather than overwritten, because this core cannot faithfully
/// restore their additional configuration without a platform settings plugin.
pub struct GnomeProxyController<R: CommandRunner = ProcessCommandRunner> {
    runner: R,
}

impl GnomeProxyController<ProcessCommandRunner> {
    pub fn new() -> Self {
        Self {
            runner: ProcessCommandRunner,
        }
    }
}
impl<R: CommandRunner> GnomeProxyController<R> {
    pub fn with_runner(runner: R) -> Self {
        Self { runner }
    }
    fn get(&self, schema: &str, key: &str) -> Result<String> {
        self.runner.run("gsettings", &["get", schema, key])
    }
    fn set(&self, schema: &str, key: &str, value: &str) -> Result<()> {
        self.runner
            .run("gsettings", &["set", schema, key, value])
            .map(|_| ())
    }
    fn read_address(&self, schema: &str) -> Result<Option<ProxyAddress>> {
        let host = self
            .get(schema, "host")?
            .trim()
            .trim_matches('\'')
            .to_owned();
        let port = self.get(schema, "port")?.trim().parse::<u16>().ok();
        Ok(match (host.is_empty(), port) {
            (false, Some(port)) => Some(ProxyAddress { host, port }),
            _ => None,
        })
    }
    fn set_address(&self, schema: &str, proxy: &ProxyAddress) -> Result<()> {
        self.set(schema, "host", &format!("'{}'", proxy.host))?;
        self.set(schema, "port", &proxy.port.to_string())
    }
}
impl<R: CommandRunner> SystemProxyController for GnomeProxyController<R> {
    fn current_proxy(&self) -> Result<ProxySnapshot> {
        let raw_mode = self.get("org.gnome.system.proxy", "mode")?;
        let mode = raw_mode.trim().trim_matches('\'');
        match mode {
            "none" => Ok(ProxySnapshot::default()),
            "manual" => Ok(ProxySnapshot {
                web: self.read_address("org.gnome.system.proxy.http")?,
                secure_web: self.read_address("org.gnome.system.proxy.https")?,
            }),
            other => bail!(
                "GNOME proxy mode '{other}' is not safe to replace; switch to none/manual first"
            ),
        }
    }
    fn set_proxy(&self, proxy: &ProxyAddress) -> Result<()> {
        self.set_address("org.gnome.system.proxy.http", proxy)?;
        self.set_address("org.gnome.system.proxy.https", proxy)?;
        self.set("org.gnome.system.proxy", "mode", "'manual'")
    }
    fn restore_proxy(&self, original: &ProxySnapshot) -> Result<()> {
        match (&original.web, &original.secure_web) {
            (None, None) => self.set("org.gnome.system.proxy", "mode", "'none'"),
            (web, secure_web) => {
                if let Some(proxy) = web {
                    self.set_address("org.gnome.system.proxy.http", proxy)?;
                }
                if let Some(proxy) = secure_web {
                    self.set_address("org.gnome.system.proxy.https", proxy)?;
                }
                self.set("org.gnome.system.proxy", "mode", "'manual'")
            }
        }
    }
}

impl<C: SystemProxyController> Drop for WeChatProxySession<C> {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Coordinates consent, local certificate setup and proxy restoration. The CA
/// is deliberately left installed after `stop`; removal is a separate,
/// explicit user action because it affects future capture sessions.
pub struct WeChatCaptureCoordinator<C: SystemProxyController, M: LocalCertificateController> {
    proxy: WeChatProxySession<C>,
    certificate: M,
}

impl<C: SystemProxyController, M: LocalCertificateController> Drop
    for WeChatCaptureCoordinator<C, M>
{
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl<C: SystemProxyController, M: LocalCertificateController> WeChatCaptureCoordinator<C, M> {
    pub fn new(proxy_controller: C, certificate: M) -> Self {
        Self {
            proxy: WeChatProxySession::new(proxy_controller),
            certificate,
        }
    }

    pub fn start(&self, consent: ProxyConsent, local_proxy: ProxyAddress) -> Result<()> {
        if !consent.install_local_ca || !consent.intercept_wechat_traffic {
            bail!(
                "WeChat capture requires explicit consent for its local CA and traffic interception"
            );
        }
        if !self.certificate.is_installed()? {
            self.certificate.install()?;
        }
        self.proxy.start(consent, local_proxy)
    }

    pub fn stop(&self) -> Result<()> {
        self.proxy.stop()
    }

    pub fn state(&self) -> ProxyCaptureState {
        self.proxy.state()
    }
}
