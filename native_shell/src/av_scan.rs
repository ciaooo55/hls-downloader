//! Post-download AV scan. Windows Defender by default; optional `{file}` template.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    pub state: String,
    pub engine: String,
    pub detail: String,
}

pub fn scan_file(path: &Path, template: &str) -> ScanResult {
    if !path.is_file() {
        // 待发布的产物不见了/不是文件。这不是"没配扫描器"的跳过：
        // 扫描器要扫的东西不存在，照旧发布等于没扫就发布。
        return ScanResult {
            state: "error".into(),
            engine: "none".into(),
            detail: "output is not a file".into(),
        };
    }
    let (engine, argv) = resolve_command(path, template);
    if argv.is_empty() {
        // 只有"压根没配扫描器、机器上也没有 Defender"才是可以接受的跳过。
        // 模板填了却被校验拒绝（缺 {file}、解释器黑名单、非法字符）属于
        // "扫描没跑起来"：用户开了扫描，产品却静默不扫——必须报错。
        if engine == "custom" {
            return ScanResult {
                state: "error".into(),
                engine,
                detail: "扫描命令未通过校验，已拒绝执行".into(),
            };
        }
        return ScanResult {
            state: "skipped".into(),
            engine,
            detail: "no scanner configured".into(),
        };
    }
    run_command(&engine, &argv)
}

pub(crate) fn validate_custom_command(command: &str) -> Result<(), String> {
    let command = command.trim();
    if command.is_empty() {
        return Ok(());
    }
    if command.len() > 2048 {
        return Err("扫描命令过长".into());
    }
    if command.chars().any(|ch| ch.is_control()) || command.contains("..") || command.contains('%')
    {
        return Err("扫描命令含有无效字符".into());
    }
    let argv = split_command(command)?;
    let Some((program, args)) = argv.split_first() else {
        return Err("扫描命令无效".into());
    };
    if program.is_empty() {
        return Err("扫描命令无效".into());
    }
    if program.contains("{file}") || program.contains('{') || program.contains('}') {
        return Err("扫描程序路径不能包含占位符".into());
    }
    if !args.iter().any(|arg| arg.contains("{file}")) {
        return Err("扫描命令必须包含 {file}".into());
    }
    let normalized = program.trim_end_matches(['.', ' ']);
    let name = Path::new(normalized)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // 扫描命令按 Windows 语义解析：`\`、`/` 与盘符 `:` 都是路径分隔符。只靠宿主 `Path`
    // 时，非 Windows 平台会把 `C:\Windows\System32\mshta.exe` 整体当成文件名而漏判。
    // 两种解析任一命中黑名单即拒绝，Windows 上的判断不会放宽。
    let windows_name = normalized
        .rsplit(['\\', '/', ':'])
        .next()
        .map(|leaf| leaf.trim_end_matches(['.', ' ']))
        .map(|leaf| match leaf.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => stem,
            _ => leaf,
        })
        .unwrap_or("")
        .to_ascii_lowercase();
    const BLOCKED: &[&str] = &[
        "cmd",
        "powershell",
        "pwsh",
        "wscript",
        "cscript",
        "mshta",
        "rundll32",
        "regsvr32",
        "wmic",
        "bitsadmin",
        "certutil",
        "msiexec",
        "forfiles",
        "cmstp",
        "hh",
        "bash",
        "sh",
        "python",
        "python3",
        "pythonw",
        "perl",
        "node",
        "wsl",
        "explorer",
        "msdt",
        "control",
        "mmc",
        "reg",
        "schtasks",
        "ftp",
        "tftp",
        "ssh",
        "start",
    ];
    if BLOCKED.contains(&name.as_str()) || BLOCKED.contains(&windows_name.as_str()) {
        return Err("扫描命令不能调用系统脚本解释器".into());
    }
    Ok(())
}

fn resolve_command(path: &Path, template: &str) -> (String, Vec<String>) {
    let custom = template.trim();
    if !custom.is_empty() {
        if validate_custom_command(custom).is_err() {
            return ("custom".into(), Vec::new());
        }
        let Ok(argv) = split_command(custom) else {
            return ("custom".into(), Vec::new());
        };
        let file_path = path.to_string_lossy();
        let mut rendered = Vec::with_capacity(argv.len());
        if let Some((program, args)) = argv.split_first() {
            rendered.push(program.clone());
            for arg in args {
                rendered.push(arg.replace("{file}", &file_path));
            }
        }
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
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| {
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

fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has_token = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                has_token = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has_token {
                    out.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if quoted {
        return Err("扫描命令引号未闭合".into());
    }
    if has_token {
        out.push(current);
    }
    Ok(out)
}

fn run_command(engine: &str, argv: &[String]) -> ScanResult {
    run_command_with_timeout(engine, argv, scan_timeout())
}

fn run_command_with_timeout(engine: &str, argv: &[String], timeout: Duration) -> ScanResult {
    if argv.is_empty() {
        return ScanResult {
            state: "error".into(),
            engine: engine.into(),
            detail: "empty scanner argv".into(),
        };
    }
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ScanResult {
                state: "error".into(),
                engine: engine.into(),
                detail: error.to_string(),
            }
        }
    };
    let mut output_reader = child.stdout.take().map(|mut stdout| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        })
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code().unwrap_or(1);
                let text = collect_output(&mut output_reader);
                return interpret(engine, code, &text);
            }
            Ok(None) if started.elapsed() >= timeout => {
                let kill_error = child.kill().err();
                let wait_error = child.wait().err();
                let _ = collect_output(&mut output_reader);
                let mut detail = timeout_detail(timeout);
                if let Some(error) = kill_error {
                    detail.push_str(&format!("; kill failed: {error}"));
                }
                if let Some(error) = wait_error {
                    detail.push_str(&format!("; wait failed: {error}"));
                }
                return ScanResult {
                    state: "error".into(),
                    engine: engine.into(),
                    detail,
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = collect_output(&mut output_reader);
                return ScanResult {
                    state: "error".into(),
                    engine: engine.into(),
                    detail: format!("scanner wait failed: {error}"),
                };
            }
        }
    }
}

fn collect_output(reader: &mut Option<JoinHandle<Vec<u8>>>) -> String {
    let bytes = reader
        .take()
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).trim().replace('\n', " ")
}

fn timeout_detail(timeout: Duration) -> String {
    if timeout.as_secs() > 0 {
        format!("scanner timed out after {}s", timeout.as_secs())
    } else {
        format!("scanner timed out after {}ms", timeout.as_millis())
    }
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
        // 模板缺 {file} 被校验拒绝 = 扫描没跑起来，必须是 error。
        // 以前这里返回 skipped，发布路径照旧放行——用户开了扫描却静默没扫。
        let rejected = scan_file(&path, "echo");
        assert_eq!(rejected.state, "error");
        assert!(rejected.detail.contains("拒绝执行"));
        let _ = std::fs::remove_file(path);
    }

    /// 反证：没配置任何扫描器（模板为空）时才允许 skipped——没开扫描的用户
    /// 不能被本配置文件挡住。这一条和上面的 error 互为边界。
    #[test]
    fn no_scanner_configured_is_the_only_skipped_state() {
        let path = std::env::temp_dir().join("hls-av-scan-none.bin");
        std::fs::write(&path, b"ok").unwrap();
        let missing = std::env::temp_dir().join("hls-av-scan-vanished.bin");
        let _ = std::fs::remove_file(&missing);
        // 产物不存在 -> error（要扫的东西没了，不是"没配扫描器"）。
        let vanished = scan_file(&missing, "");
        assert_eq!(vanished.state, "error");
        assert!(vanished.detail.contains("not a file"));
        // 模板为空且机器上恰好没有 Defender 时才 skipped。
        let (engine, argv) = resolve_command(&path, "  ");
        assert!(
            engine != "custom",
            "空模板不应走进 custom 分支：{engine:?} {argv:?}"
        );
        if engine == "none" {
            assert_eq!(scan_file(&path, "  ").state, "skipped");
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn splits_quoted_argv() {
        assert_eq!(
            split_command(r#"C:\scan.exe -f "C:\a bin.dat""#).unwrap(),
            vec!["C:\\scan.exe", "-f", "C:\\a bin.dat"]
        );
        let path = std::env::temp_dir().join("hls av scan.bin");
        std::fs::write(&path, b"ok").unwrap();
        let (_, argv) = resolve_command(&path, r#"C:\scan.exe -f {file}"#);
        assert_eq!(
            argv,
            vec![
                "C:\\scan.exe".to_string(),
                "-f".into(),
                path.to_string_lossy().into_owned()
            ]
        );
        assert!(validate_custom_command("%COMSPEC% /c calc {file}").is_err());
        #[cfg(windows)]
        {
            assert!(validate_custom_command(r"C:\Windows\explorer.exe {file}").is_err());
            let rejected = scan_file(&path, r"C:\Windows\explorer.exe {file}");
            assert_eq!(rejected.state, "error");
            assert!(rejected.detail.contains("拒绝执行"));
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_quote_splitting_unbalanced_quotes_and_program_placeholder() {
        for bad in [
            r#"c"m"d.exe /c {file}"#,
            r#""c"md.exe /c {file}"#,
            r#"C:\Windows\System32\p"o"wershell.exe {file}"#,
            r#"cmd.exe. /c {file}"#,
            r#""C:\scan.exe {file}"#,
            r#"{file}"#,
            r#""{file}" --scan"#,
            r#"C:\tools\{file}.exe --scan {file}"#,
            r#""" {file}"#,
            r#"C:\scan.exe "{file}""#,
        ] {
            if bad == r#"C:\scan.exe "{file}""# {
                assert!(
                    validate_custom_command(bad).is_ok(),
                    "valid quoted arg: {bad}"
                );
                continue;
            }
            assert!(
                validate_custom_command(bad).is_err(),
                "expected rejection for custom scanner command: {bad}"
            );
        }
        let path = std::env::temp_dir().join("hls-av-argv0-guard.bin");
        std::fs::write(&path, b"ok").unwrap();
        let (_, argv) = resolve_command(&path, r#"c"m"d.exe /c {file}"#);
        assert!(argv.is_empty());
        let (_, argv) = resolve_command(&path, r#"{file}"#);
        assert!(argv.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn timeout_terminates_scanner_process() {
        let argv = vec![
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            "--ignored".into(),
            "--exact".into(),
            "av_scan::tests::scanner_timeout_fixture".into(),
        ];
        let result = run_command_with_timeout("custom", &argv, Duration::from_millis(50));
        assert_eq!(result.state, "error");
        assert!(result.detail.contains("timed out"));
    }

    #[test]
    #[ignore]
    fn scanner_timeout_fixture() {
        std::thread::sleep(Duration::from_secs(2));
    }
}
