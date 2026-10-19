use live_archive_core::doctor::{CheckLevel, DirectoryState, DoctorConfig, DoctorProbe, diagnose};
use std::path::Path;

struct Probe;
impl DoctorProbe for Probe {
    fn command_version(&self, command: &Path) -> Result<String, String> {
        if command == Path::new("ffmpeg") {
            Ok("ffmpeg 7".into())
        } else {
            Err("not found".into())
        }
    }
    fn directory_state(&self, _directory: &Path) -> Result<DirectoryState, String> {
        Ok(DirectoryState::Missing)
    }
    fn mac_network_services(&self) -> Result<Vec<String>, String> {
        Ok(vec!["Wi-Fi".into()])
    }
}

#[test]
fn doctor_reports_missing_dependency_and_missing_output_as_actionable_states() {
    let reports = diagnose(&Probe, &DoctorConfig::default());
    assert_eq!(reports[0].level, CheckLevel::Pass);
    assert_eq!(reports[1].level, CheckLevel::Fail);
    assert_eq!(reports[2].level, CheckLevel::Warn);
}
