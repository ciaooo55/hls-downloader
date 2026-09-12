from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one anchor, found {count}")
    path.write_text(text.replace(old, new, 1), encoding="utf-8")


cast = Path("native_shell/src/cast.rs")
replace_once(
    cast,
    '''    let token = crate::playback::random_mount_token();
    server.mount_remote(&token, url.to_string());
    let host = preferred_lan_ipv4()
        .map(|ip| ip.to_string())
        .ok_or_else(|| "没有可用于投屏的局域网地址".to_string())?;
''',
    '''    let host = preferred_lan_ipv4()
        .map(|ip| ip.to_string())
        .ok_or_else(|| "没有可用于投屏的局域网地址".to_string())?;
    let token = crate::playback::random_mount_token();
    server.mount_remote(&token, url.to_string());
''',
    "start_browser_push ownership",
)

worker = Path("native_shell/src/download_worker.rs")
replace_once(
    worker,
    '''fn cast_task(coordinator: &CoreCoordinator, task_id: &str) -> Result<Vec<EventEnvelope>, String> {
    let loopback = mount_task_url(coordinator, task_id)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let server = shared_media()?;
    server.enable_lan();
    let host = cast_lan_host("")?;
''',
    '''fn cast_task(coordinator: &CoreCoordinator, task_id: &str) -> Result<Vec<EventEnvelope>, String> {
    let host = cast_lan_host("")?;
    let loopback = mount_task_url(coordinator, task_id)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let server = shared_media()?;
    server.enable_lan();
''',
    "cast_task ownership",
)

replace_once(
    worker,
    '''fn cast_to_device(
    coordinator: &CoreCoordinator,
    task_id: &str,
    device_id: &str,
) -> Result<Vec<EventEnvelope>, String> {
    let loopback = mount_task_url(coordinator, task_id)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let server = shared_media()?;
    server.enable_lan();
    let host = cast_lan_host(device_id)?;
''',
    '''fn cast_to_device(
    coordinator: &CoreCoordinator,
    task_id: &str,
    device_id: &str,
) -> Result<Vec<EventEnvelope>, String> {
    let host = cast_lan_host(device_id)?;
    let loopback = mount_task_url(coordinator, task_id)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let server = shared_media()?;
    server.enable_lan();
''',
    "cast_to_device ownership",
)

replace_once(
    worker,
    '''        server.mount(&token, source);
        let host = cast_lan_host(device_id)?;
        crate::cast::lan_media_url(server, &token, &host)?
''',
    '''        let host = cast_lan_host(device_id)?;
        server.mount(&token, source);
        crate::cast::lan_media_url(server, &token, &host)?
''',
    "share local media ownership",
)

replace_once(
    worker,
    '''        } else {
            server.mount_remote(&token, media_url.clone());
            let host = cast_lan_host("")?;
            let redirect = crate::cast::lan_media_url(server, &token, &host)?;
''',
    '''        } else {
            let host = cast_lan_host("")?;
            server.mount_remote(&token, media_url.clone());
            let redirect = crate::cast::lan_media_url(server, &token, &host)?;
''',
    "share remote redirect ownership",
)
