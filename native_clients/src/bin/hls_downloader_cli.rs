//! Console client. The resident Core owns all task state and transfers.
use hls_native_clients::{
    install_root, spawn_core, CoreCommand, CoreEvent, CoreIpcClient, CorePipeRequest,
    CorePipeResponse, CrawlRequest, CrawlSpec, EventEnvelope, ResourceKind, TaskSnapshot, TaskSpec,
};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

const HELP: &str = "HLSDownloaderCli [--json] COMMAND
  add URL [--dir PATH] [--name NAME] [--queue ID] [--paused] [--kind file|hls|dash]
          [--context FILE|-]  (JSON: url, headers, request_method, body_path)
  list | show TASK_ID
  pause|resume|retry TASK_ID
  remove TASK_ID [--delete-files]
  queue start|stop QUEUE_ID
  import FILE...
  export [--format json|csv|urls] [--output FILE] [TASK_ID...]
  zip SOURCE  (local path or HTTP(S) URL, directory preview only)
  crawl create --spec FILE|-  (CrawlSpec JSON; discovery only)
  crawl list | crawl get|pause|resume|sync PROJECT_ID
  crawl enqueue PROJECT_ID [URL...]  (empty list selects all discovered resources)
Exit codes: 0 success, 2 usage, 3 Core connection, 4 rejected/failed, 5 local I/O.
Default output excludes URLs and request headers. Use context stdin for secrets.
--paused creates a paused task atomically. Without it Core schedules normally.
";

struct Failure(i32, String);
type Result<T> = std::result::Result<T, Failure>;
fn usage(message: impl Into<String>) -> Failure {
    Failure(2, message.into())
}
fn rejected(message: impl Into<String>) -> Failure {
    Failure(4, message.into())
}
fn io_error(_: std::io::Error) -> Failure {
    Failure(5, "无法读写输入或输出文件".into())
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let as_json = args.iter().any(|arg| arg == "--json");
    args.retain(|arg| arg != "--json");
    if args.is_empty() || args[0] == "--help" || args[0] == "help" {
        print!("{HELP}");
        return;
    }
    let result = run(&args).and_then(|value| {
        let output = if as_json {
            serde_json::to_string(&value).map_err(|_| rejected("输出序列化失败"))?
        } else if let Some(data) = value.get("data").and_then(Value::as_str) {
            data.to_string()
        } else {
            serde_json::to_string_pretty(&value).map_err(|_| rejected("输出序列化失败"))?
        };
        writeln!(std::io::stdout().lock(), "{output}").map_err(io_error)
    });
    if let Err(Failure(code, message)) = result {
        if as_json {
            eprintln!("{}", json!({"error": message, "exit_code": code}));
        } else {
            eprintln!("{message}");
        }
        std::process::exit(code);
    }
}

fn connect() -> Result<CoreIpcClient> {
    if let Ok(client) = CoreIpcClient::connect_existing(Duration::from_millis(150)) {
        return Ok(client);
    }
    let root = install_root().ok_or_else(|| Failure(3, "无法定位安装目录".into()))?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{
            SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        };
        // A script captures these handles until every writer closes. The
        // resident Core must not keep the CLI's original capture handles alive.
        // Command creates its own null standard handles for the Core.
        for handle in [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ] {
            if !handle.is_null()
                && handle != INVALID_HANDLE_VALUE
                && unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0
            {
                return Err(Failure(3, "无法隔离 Core 的标准输入输出句柄".into()));
            }
        }
    }
    spawn_core(&root).map_err(|_| Failure(3, "无法启动 Core，请检查安装目录".into()))?;
    CoreIpcClient::connect_existing(Duration::from_secs(15))
        .map_err(|_| Failure(3, "无法连接 Core".into()))
}

fn command(client: &mut CoreIpcClient, cmd: CoreCommand) -> Result<Vec<EventEnvelope>> {
    let events = client
        .command(cmd)
        .map_err(|_| rejected("Core 拒绝请求；请检查任务、设置及使用条款状态"))?;
    for envelope in &events {
        match &envelope.event {
            CoreEvent::Error { code, .. } => return Err(rejected(format!("Core 错误: {code}"))),
            CoreEvent::DuplicateOffered { .. } => {
                return Err(rejected("任务已存在，需要在工作台选择重复任务处理方式"))
            }
            _ => {}
        }
    }
    Ok(events)
}

fn summary(task: &TaskSnapshot) -> Value {
    json!({"task_id": task.task_id, "filename": task.filename, "status": task.status,
        "downloaded_bytes": task.downloaded_bytes, "total_bytes": task.total_bytes,
        "speed_bytes_per_sec": task.speed_bytes_per_sec, "queue_id": task.queue_id,
        "available_actions": task.available_actions, "error_code": task.error_code})
}

fn event_result(events: &[EventEnvelope]) -> Value {
    let tasks: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.event {
            CoreEvent::TaskCreated { snapshot }
            | CoreEvent::TaskUpdated { snapshot }
            | CoreEvent::TaskProgress { snapshot } => Some(summary(snapshot)),
            _ => None,
        })
        .collect();
    json!({"ok": true, "tasks": tasks})
}

fn value<'a>(args: &'a [String], i: &mut usize) -> Result<&'a str> {
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| usage("选项缺少参数"))
}

fn read_input(source: &str) -> Result<Vec<u8>> {
    // Bound even stdin before JSON parsing; never echo this data or parser diagnostics.
    let reader: Box<dyn Read> = if source == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(std::fs::File::open(source).map_err(io_error)?)
    };
    let mut bytes = Vec::new();
    reader
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > 1024 * 1024 {
        return Err(usage("请求上下文超过 1 MiB"));
    }
    Ok(bytes)
}

fn read_context(source: &str, spec: &mut TaskSpec) -> Result<()> {
    let bytes = read_input(source)?;
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Context {
        url: Option<String>,
        headers: Option<std::collections::BTreeMap<String, String>>,
        request_method: Option<String>,
        body_path: Option<String>,
    }
    let context: Context =
        serde_json::from_slice(&bytes).map_err(|_| usage("请求上下文 JSON 无效"))?;
    if let Some(v) = context.url {
        spec.url = v;
    }
    if let Some(v) = context.headers {
        spec.headers = v;
    }
    if let Some(v) = context.request_method {
        spec.request_method = v;
    }
    if let Some(v) = context.body_path {
        spec.body_path = absolute(&v)?;
    }
    Ok(())
}

fn absolute(path: &str) -> Result<String> {
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(io_error)?.join(path)
    };
    Ok(path.to_string_lossy().into_owned())
}

fn run(args: &[String]) -> Result<Value> {
    // Parse completely before starting Core, including unknown subcommands/options.
    let op = args[0].as_str();
    let mut spec = TaskSpec::default();
    let mut format = "json".to_string();
    let mut output = None;
    let mut ids = Vec::new();
    let mut delete_files = false;
    let mut crawl = None;
    match op {
        "add" => {
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--dir" => spec.download_dir = absolute(value(args, &mut i)?)?,
                    "--name" => spec.filename = value(args, &mut i)?.into(),
                    "--queue" => spec.queue_id = value(args, &mut i)?.into(),
                    "--paused" => spec.start_paused = true,
                    "--context" => read_context(value(args, &mut i)?, &mut spec)?,
                    "--kind" => {
                        spec.resource_kind = match value(args, &mut i)? {
                            "file" => ResourceKind::File,
                            "hls" => ResourceKind::Hls,
                            "dash" => ResourceKind::Dash,
                            _ => return Err(usage("不支持的资源类型")),
                        }
                    }
                    v if !v.starts_with('-') && spec.url.is_empty() => spec.url = v.into(),
                    _ => return Err(usage("add 参数无效")),
                }
                i += 1;
            }
            if spec.url.is_empty() {
                return Err(usage("add 需要 URL 或含 url 的 --context"));
            }
        }
        "list" if args.len() == 1 => {}
        "show" | "pause" | "resume" | "retry" if args.len() == 2 => ids.push(args[1].clone()),
        "remove" if args.len() == 2 || (args.len() == 3 && args[2] == "--delete-files") => {
            ids.push(args[1].clone());
            delete_files = args.len() == 3;
        }
        "queue" if args.len() == 3 && matches!(args[1].as_str(), "start" | "stop") => {}
        "zip" if args.len() == 2 => {}
        "crawl" => {
            crawl = Some(match args.get(1).map(String::as_str) {
                Some("create") if args.len() == 4 && args[2] == "--spec" => {
                    let spec: CrawlSpec = serde_json::from_slice(&read_input(&args[3])?)
                        .map_err(|_| usage("抓站项目 JSON 无效"))?;
                    CrawlRequest::Create { spec }
                }
                Some("list") if args.len() == 2 => CrawlRequest::List,
                Some(action)
                    if args.len() == 3 && matches!(action, "get" | "pause" | "resume" | "sync") =>
                {
                    let project_id = args[2].clone();
                    match action {
                        "get" => CrawlRequest::Get { project_id },
                        "pause" => CrawlRequest::Pause { project_id },
                        "resume" => CrawlRequest::Resume { project_id },
                        _ => CrawlRequest::Sync { project_id },
                    }
                }
                Some("enqueue") if args.len() >= 3 => CrawlRequest::Enqueue {
                    project_id: args[2].clone(),
                    urls: args[3..].to_vec(),
                },
                _ => return Err(usage("crawl 参数无效，请查看 --help")),
            });
        }
        "import" if args.len() > 1 => {
            for path in &args[1..] {
                ids.push(absolute(path)?);
            }
        }
        "export" => {
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--format" => format = value(args, &mut i)?.into(),
                    "--output" => output = Some(absolute(value(args, &mut i)?)?),
                    v if !v.starts_with('-') => ids.push(v.into()),
                    _ => return Err(usage("export 参数无效")),
                }
                i += 1;
            }
            if !matches!(format.as_str(), "json" | "csv" | "urls") {
                return Err(usage("导出格式无效"));
            }
        }
        _ => return Err(usage("命令或参数无效；使用 --help 查看用法")),
    }
    let mut client = connect()?;
    let capability = match op {
        "zip" => Some("preview_zip"),
        "crawl" => Some("crawl_projects"),
        "add" if spec.start_paused => Some("create_task_paused"),
        _ => None,
    };
    if let Some(capability) = capability {
        match client
            .request(&CorePipeRequest::Capabilities { request_id: 1 })
            .map_err(|_| rejected("无法查询 Core 能力"))?
        {
            CorePipeResponse::Capabilities { commands, .. }
                if commands.iter().any(|v| v == capability) => {}
            _ => return Err(rejected("当前 Core 版本不支持此功能，请升级并重启 Core")),
        }
    }
    match op {
        "crawl" => match client
            .request(&CorePipeRequest::Crawl {
                request_id: 1,
                request: crawl.ok_or_else(|| usage("抓站参数缺失"))?,
            })
            .map_err(|_| rejected("抓站请求失败"))?
        {
            CorePipeResponse::Crawl { projects, .. } => Ok(json!({"projects":projects})),
            CorePipeResponse::Error { message, .. } => Err(rejected(message)),
            _ => Err(rejected("抓站响应无效")),
        },
        "zip" => {
            let source = if args[1].starts_with("http://") || args[1].starts_with("https://") {
                args[1].clone()
            } else {
                absolute(&args[1])?
            };
            match client
                .request(&CorePipeRequest::PreviewZip {
                    request_id: 1,
                    source,
                })
                .map_err(|_| rejected("ZIP 预览请求失败"))?
            {
                CorePipeResponse::ZipPreview { preview, .. } => {
                    serde_json::to_value(preview).map_err(|_| rejected("ZIP 结果无效"))
                }
                CorePipeResponse::Error { message, .. } => Err(rejected(message)),
                _ => Err(rejected("ZIP 预览响应无效")),
            }
        }
        "add" => Ok(event_result(&command(
            &mut client,
            CoreCommand::CreateTask { spec },
        )?)),
        "list" => Ok(
            json!({"tasks": client.snapshot().map_err(|_| rejected("无法读取任务"))?
            .iter().map(summary).collect::<Vec<_>>()}),
        ),
        "show" => {
            let events = command(
                &mut client,
                CoreCommand::GetTask {
                    task_id: ids.remove(0),
                },
            )?;
            Ok(event_result(&events))
        }
        "pause" | "resume" | "retry" | "remove" => {
            let task_id = ids.remove(0);
            let action = match op {
                "remove" if delete_files => "delete_files",
                "remove" => "delete",
                v => v,
            };
            let events = command(
                &mut client,
                CoreCommand::TaskAction {
                    task_id: task_id.clone(),
                    action: action.into(),
                },
            )?;
            Ok(json!({"ok":true,"task_id":task_id,"result":event_result(&events)}))
        }
        "import" => Ok(event_result(&command(
            &mut client,
            CoreCommand::ImportPaths { paths: ids },
        )?)),
        "export" => {
            let events = command(
                &mut client,
                CoreCommand::ExportTasks {
                    task_ids: ids,
                    format,
                },
            )?;
            let (data, count) = events
                .iter()
                .find_map(|e| match &e.event {
                    CoreEvent::TaskExport {
                        data, task_count, ..
                    } => Some((data, task_count)),
                    _ => None,
                })
                .ok_or_else(|| rejected("Core 没有返回导出结果"))?;
            if let Some(path) = output {
                // Do not overwrite a script's existing export accidentally.
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map_err(io_error)?;
                file.write_all(data.as_bytes()).map_err(io_error)?;
                Ok(json!({"ok":true,"task_count":count}))
            } else {
                Ok(json!({"data":data,"task_count":count}))
            }
        }
        "queue" => {
            let mut profiles = match client
                .load_settings()
                .map_err(|_| rejected("无法读取队列"))?
            {
                CorePipeResponse::Settings { queue_profiles, .. } => queue_profiles,
                _ => return Err(rejected("Core 返回了无效队列设置")),
            };
            let profile = profiles
                .iter_mut()
                .find(|p| p.id == args[2])
                .ok_or_else(|| rejected("队列不存在，请使用队列 ID"))?;
            profile.enabled = args[1] == "start";
            client
                .store_setting("queue_profiles", serde_json::to_value(&profiles).unwrap())
                .map_err(|_| rejected("无法保存队列"))?;
            for task in client.snapshot().map_err(|_| rejected("无法读取任务"))? {
                if task.queue_id != args[2] {
                    continue;
                }
                let action = if args[1] == "stop" {
                    "pause"
                } else if task.status == "paused" {
                    "resume"
                } else {
                    "start"
                };
                if task.available_actions.iter().any(|a| a == action) {
                    command(
                        &mut client,
                        CoreCommand::TaskAction {
                            task_id: task.task_id,
                            action: action.into(),
                        },
                    )?;
                }
            }
            Ok(json!({"ok":true,"queue_id":args[2],"enabled":args[1]=="start"}))
        }
        _ => unreachable!(),
    }
}
