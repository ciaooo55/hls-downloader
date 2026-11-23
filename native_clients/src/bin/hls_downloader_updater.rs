#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::ffi::OsString;
use std::process::ExitCode;

fn main() -> ExitCode {
    run(std::env::args_os().skip(1))
}

fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    match hls_native_clients::run_update_helper(args) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("update helper failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use hls_client::update_install::InstallResult;
    use std::path::Path;

    #[test]
    fn production_updater_records_unsigned_package_rejection_before_install() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/audit-tests")
            .join(format!("production-updater-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let msi = root.join("unsigned.msi");
        let result = root.join("result.json");
        let log = root.join("install.log");
        std::fs::write(&msi, b"unsigned updater fixture").unwrap();
        let exit = run([
            "--msi".into(),
            msi.into_os_string(),
            "--version".into(),
            "10.0.0".into(),
            "--core-pid".into(),
            "0".into(),
            "--workbench-pid".into(),
            "0".into(),
            "--install-root".into(),
            root.clone().into_os_string(),
            "--workbench".into(),
            root.join("absent.exe").into_os_string(),
            "--log".into(),
            log.clone().into_os_string(),
            "--result".into(),
            result.clone().into_os_string(),
        ]);
        assert_eq!(exit, ExitCode::FAILURE);
        let recorded: InstallResult =
            serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        assert_eq!(recorded.status, "failed");
        assert_eq!(recorded.exit_code, -1);
        assert!(recorded.message.contains("WinVerifyTrust"));
        assert!(!log.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
