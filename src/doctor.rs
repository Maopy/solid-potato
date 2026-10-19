//! Read-only preflight diagnostics for the desktop recording runtime.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckLevel {
    Pass,
    Warn,
    Fail,
}

impl CheckLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckReport {
    pub name: &'static str,
    pub level: CheckLevel,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct DoctorConfig {
    pub ffmpeg: PathBuf,
    pub mitmdump: PathBuf,
    pub output_directory: PathBuf,
    pub network_service: String,
}

impl Default for DoctorConfig {
    fn default() -> Self {
        Self {
            ffmpeg: PathBuf::from("ffmpeg"),
            mitmdump: PathBuf::from("mitmdump"),
            output_directory: PathBuf::from("data/recordings"),
            network_service: "Wi-Fi".into(),
        }
    }
}

pub trait DoctorProbe {
    fn command_version(&self, command: &Path) -> Result<String, String>;
    fn directory_state(&self, directory: &Path) -> Result<DirectoryState, String>;
    fn mac_network_services(&self) -> Result<Vec<String>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryState {
    ExistingWritable,
    ExistingReadOnly,
    Missing,
}

pub struct ProcessDoctorProbe;
impl DoctorProbe for ProcessDoctorProbe {
    fn command_version(&self, command: &Path) -> Result<String, String> {
        let version_flag = if command.file_name().and_then(|name| name.to_str()) == Some("ffmpeg") {
            // FFmpeg accepts the single-dash form. In particular, FFmpeg 9
            // prints its version for `--version` but then exits with an error.
            "-version"
        } else {
            "--version"
        };
        let output = Command::new(command)
            .arg(version_flag)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("available")
            .to_owned())
    }
    fn directory_state(&self, directory: &Path) -> Result<DirectoryState, String> {
        match std::fs::metadata(directory) {
            Ok(metadata) if !metadata.is_dir() => Err("path exists but is not a directory".into()),
            Ok(metadata) if metadata.permissions().readonly() => {
                Ok(DirectoryState::ExistingReadOnly)
            }
            Ok(_) => Ok(DirectoryState::ExistingWritable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(DirectoryState::Missing)
            }
            Err(error) => Err(error.to_string()),
        }
    }
    fn mac_network_services(&self) -> Result<Vec<String>, String> {
        #[cfg(target_os = "macos")]
        {
            let output = Command::new("networksetup")
                .arg("-listallnetworkservices")
                .output()
                .map_err(|error| error.to_string())?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
            }
            Ok(String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|value| !value.is_empty() && !value.starts_with("An asterisk"))
                .map(str::to_owned)
                .collect())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Vec::new())
        }
    }
}

pub fn diagnose(probe: &dyn DoctorProbe, config: &DoctorConfig) -> Vec<CheckReport> {
    let mut reports = vec![
        tool_report(probe, "FFmpeg", &config.ffmpeg),
        tool_report(probe, "mitmdump", &config.mitmdump),
    ];
    reports.push(match probe.directory_state(&config.output_directory) {
        Ok(DirectoryState::ExistingWritable) => CheckReport {
            name: "Recording directory",
            level: CheckLevel::Pass,
            detail: config.output_directory.display().to_string(),
        },
        Ok(DirectoryState::ExistingReadOnly) => CheckReport {
            name: "Recording directory",
            level: CheckLevel::Fail,
            detail: format!("{} is read-only", config.output_directory.display()),
        },
        Ok(DirectoryState::Missing) => CheckReport {
            name: "Recording directory",
            level: CheckLevel::Warn,
            detail: format!(
                "{} will be created when recording starts",
                config.output_directory.display()
            ),
        },
        Err(error) => CheckReport {
            name: "Recording directory",
            level: CheckLevel::Fail,
            detail: error,
        },
    });
    #[cfg(target_os = "macos")]
    reports.push(match probe.mac_network_services() {
        Ok(services)
            if services
                .iter()
                .any(|service| service == &config.network_service) =>
        {
            CheckReport {
                name: "Network service",
                level: CheckLevel::Pass,
                detail: config.network_service.clone(),
            }
        }
        Ok(services) => CheckReport {
            name: "Network service",
            level: CheckLevel::Fail,
            detail: format!(
                "{} not found; available: {}",
                config.network_service,
                services.join(", ")
            ),
        },
        Err(error) => CheckReport {
            name: "Network service",
            level: CheckLevel::Fail,
            detail: error,
        },
    });
    reports
}

fn tool_report(probe: &dyn DoctorProbe, name: &'static str, path: &Path) -> CheckReport {
    match probe.command_version(path) {
        Ok(version) => CheckReport {
            name,
            level: CheckLevel::Pass,
            detail: version,
        },
        Err(error) => CheckReport {
            name,
            level: CheckLevel::Fail,
            detail: format!("{} ({error})", path.display()),
        },
    }
}
