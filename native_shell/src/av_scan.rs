//! Post-download AV scan. Windows Defender by default; optional `{file}` template.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    pub state: String,
    pub engine: String,
    pub detail: String,
}

pub fn scan_file(path: &Path, template: &str) -> ScanResult {
    if !path.is_file() {
        return ScanResult {
            state: "skipped".into(),
            engine: "none".into(),
            detail: "output is not a file".into(),
        };
    }
    let (engine, argv) = resolve_command(path, template);
    if argv.is_empty() {
        return ScanResult {
            state: "skipped".into(),
            engine,
            detail: "no scanner configured".into(),
        };
    }
    run_command(&engine, &argv)
}

fn resolve_command(path: &Path, template: &str) -> (String, Vec<String>) {
    let custom = template.trim();
    if !custom.is_empty() {
        if !custom.contains("{file}") || custom.len() > 2048 {
            return ("custom".into(), Vec::new());
        }
        let rendered = split_command(custom)
            .into_iter()
            .map(|arg| arg.replace("{file}", &path.to_string_lossy()))
            .collect::<Vec<_>>();
        return ("custom".into(), rendered);
    }
    if let Some(defender) = discover_defender() {
        let mut argv = defender;
        argv.push(path.to_string_lossy().into_owned());
        return ("defender".into(), argv);
    }
    ("none".into(), Vec::new())
}

fn discover_defender() -> Option<Vec<String>> {
    let mut candidates = Vec::new();
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(root) = std::env::var(key) {
            candidates.push(
                std::path::PathBuf::from(root)
                    .join("Windows Defender")
                    .join("MpCmdRun.exe"),
            );
        }
    }
    if let Ok(data) = std::env::var("ProgramData") {
        let platform = std::path::PathBuf::from(data)
            .join("Microsoft")
            .join("Windows Defender")
            .join("Platform");
        if let Ok(entries) = std::fs::read_dir(platform) {
            let mut dirs: Vec<_> = entries.filter_map(|item| item.ok()).collect();
            dirs.sort_by_key(|item| std::cmp::Reverse(item.file_name()));
            for entry in dirs {
                candidates.push(entry.path().join("MpCmdRun.exe"));
            }
        }
    }
    candidates.into_iter().find(|path| path.is_file()).map(|path| {
        vec![
            path.to_string_lossy().into_owned(),
            "-Scan".into(),
            "-ScanType".into(),
            "3".into(),
            "-DisableRemediation".into(),
            "-File".into(),
        ]
    })
}

fn split_command(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn run_command(engine: &str, argv: &[String]) -> ScanResult {
    if argv.is_empty() {
        return ScanResult {
            state: "skipped".into(),
            engine: engine.into(),
            detail: "empty scanner argv".into(),
        };
    }
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            return ScanResult {
                state: "error".into(),
                engine: engine.into(),
                detail: error.to_string(),
            }
        }
    };
    let code = output.status.code().unwrap_or(1);
    let text = String::from_utf8_lossy(&output.stdout).trim().replace('\n', " ");
    interpret(engine, code, &text)
}

fn interpret(engine: &str, code: i32, output: &str) -> ScanResult {
    if engine == "defender" {
        return match code {
            0 => ScanResult {
                state: "clean".into(),
                engine: engine.into(),
                detail: "Windows Defender 未发现威胁".into(),
            },
            2 => ScanResult {
                state: "threat".into(),
                engine: engine.into(),
                detail: if output.is_empty() {
                    "Windows Defender 报告发现威胁".into()
                } else {
                    output.chars().take(300).collect()
                },
            },
            _ => ScanResult {
                state: "error".into(),
                engine: engine.into(),
                detail: format!("Windows Defender exit {code}"),
            },
        };
    }
    match code {
        0 => ScanResult {
            state: "clean".into(),
            engine: engine.into(),
            detail: "扫描器未发现威胁".into(),
        },
        1 => ScanResult {
            state: "threat".into(),
            engine: engine.into(),
            detail: if output.is_empty() {
                "扫描器报告发现威胁".into()
            } else {
                output.chars().take(300).collect()
            },
        },
        _ => ScanResult {
            state: "error".into(),
            engine: engine.into(),
            detail: format!("scanner exit {code}"),
        },
    }
}

pub fn scan_timeout() -> Duration {
    Duration::from_secs(180)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_template_requires_file_placeholder() {
        let path = std::env::temp_dir().join("hls-av-scan.bin");
        std::fs::write(&path, b"ok").unwrap();
        let skipped = scan_file(&path, "echo");
        assert_eq!(skipped.state, "skipped");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn splits_quoted_argv() {
        assert_eq!(
            split_command(r#"C:\scan.exe -f "C:\a bin.dat""#),
            vec!["C:\\scan.exe", "-f", "C:\\a bin.dat"]
        );
        let path = std::env::temp_dir().join("hls av scan.bin");
        let (_, argv) = resolve_command(&path, r#"C:\scan.exe -f {file}"#);
        assert_eq!(argv, vec!["C:\\scan.exe".to_string(), "-f".into(), path.to_string_lossy().into_owned()]);
    }
}
