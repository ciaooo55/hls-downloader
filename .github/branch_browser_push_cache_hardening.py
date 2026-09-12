from pathlib import Path

path = Path("native_shell/src/cast.rs")
text = path.read_text(encoding="utf-8")

old = "use std::sync::{Arc, Mutex, OnceLock};"
new = "use std::sync::atomic::{AtomicU64, Ordering};\nuse std::sync::{Arc, Mutex, OnceLock};"
if old not in text:
    raise SystemExit("atomic import anchor missing")
text = text.replace(old, new, 1)

old = '''fn pushes() -> &'static Mutex<HashMap<String, BrowserPush>> {
    static MAP: OnceLock<Mutex<HashMap<String, BrowserPush>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}
'''
new = '''const MAX_BROWSER_PUSHES: usize = 64;

fn pushes() -> &'static Mutex<VecDeque<BrowserPush>> {
    static QUEUE: OnceLock<Mutex<VecDeque<BrowserPush>>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn next_browser_push_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{millis:x}-{sequence:x}")
}

fn remember_browser_push(queue: &mut VecDeque<BrowserPush>, push: BrowserPush) {
    if queue.len() >= MAX_BROWSER_PUSHES {
        queue.pop_front();
    }
    queue.push_back(push);
}
'''
if old not in text:
    raise SystemExit("push cache anchor missing")
text = text.replace(old, new, 1)

old = '''    let id = format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
'''
new = '''    let id = next_browser_push_id();
'''
if old not in text:
    raise SystemExit("push id anchor missing")
text = text.replace(old, new, 1)

old = '''    if let Ok(mut map) = pushes().lock() {
        map.insert(push.id.clone(), push.clone());
    }
'''
new = '''    if let Ok(mut queue) = pushes().lock() {
        remember_browser_push(&mut queue, push.clone());
    }
'''
if old not in text:
    raise SystemExit("push insert anchor missing")
text = text.replace(old, new, 1)

old = '''pub fn browser_push_status(id: &str) -> Option<BrowserPush> {
    pushes().lock().ok()?.get(id).cloned()
}
'''
new = '''pub fn browser_push_status(id: &str) -> Option<BrowserPush> {
    pushes()
        .lock()
        .ok()?
        .iter()
        .rev()
        .find(|push| push.id == id)
        .cloned()
}
'''
if old not in text:
    raise SystemExit("push status anchor missing")
text = text.replace(old, new, 1)

anchor = '''    #[test]
    fn tvbox_scan_targets_prioritize_local_24_on_broad_subnet() {
'''
test = '''    #[test]
    fn browser_push_ids_are_unique_and_cache_is_bounded() {
        let ids = (0..1024)
            .map(|_| next_browser_push_id())
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 1024);

        let mut queue = VecDeque::new();
        for index in 0..(MAX_BROWSER_PUSHES + 10) {
            remember_browser_push(
                &mut queue,
                BrowserPush {
                    id: format!("push-{index}"),
                    kind: "tvbox".into(),
                    status: "ready".into(),
                    message: String::new(),
                    location: "http://192.168.1.2/media/test".into(),
                },
            );
        }
        assert_eq!(queue.len(), MAX_BROWSER_PUSHES);
        assert_eq!(queue.front().map(|push| push.id.as_str()), Some("push-10"));
        assert_eq!(
            queue.back().map(|push| push.id.as_str()),
            Some(format!("push-{}", MAX_BROWSER_PUSHES + 9).as_str())
        );
    }

'''
if anchor not in text:
    raise SystemExit("test insertion anchor missing")
text = text.replace(anchor, test + anchor, 1)

path.write_text(text, encoding="utf-8")
