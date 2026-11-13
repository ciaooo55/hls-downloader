use crate::{file_dialogs, native_window, ResourceRow, ResourceToolsWindow};
use hls_client::{
    CoreCommand, CoreEvent, CorePipeRequest, CorePipeResponse, CrawlProject, CrawlRequest,
    CrawlSpec, HarvestCandidate, ResourceKind, TaskSpec, TorrentFileEntry, TorrentFileSelection,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    time::{Duration, Instant},
};

pub struct ResourceTools {
    pub window: ResourceToolsWindow,
    rows: Rc<VecModel<ResourceRow>>,
    projects: RefCell<Vec<CrawlProject>>,
    harvest: RefCell<Vec<HarvestCandidate>>,
    torrent: RefCell<Vec<TorrentFileEntry>>,
    task_id: String,
    pending: Cell<Option<u64>>,
    waiting_torrent: Cell<bool>,
    creating_project: Cell<bool>,
    listing_projects: Cell<bool>,
    refresh_pending: Cell<Option<u64>>,
    next_refresh: Cell<Instant>,
    rendered_project_id: RefCell<String>,
    request: Rc<dyn Fn(CorePipeRequest) -> bool>,
    batch: Rc<dyn Fn(Vec<CoreCommand>) -> bool>,
    id: Rc<dyn Fn() -> u64>,
}

impl ResourceTools {
    pub fn keeps_ui_alive(&self) -> bool {
        self.window.window().is_visible()
            || self.pending.get().is_some()
            || self.waiting_torrent.get()
    }

    pub fn new(
        mode: &str,
        task_id: String,
        directory: &str,
        dark: bool,
        request: Rc<dyn Fn(CorePipeRequest) -> bool>,
        batch: Rc<dyn Fn(Vec<CoreCommand>) -> bool>,
        id: Rc<dyn Fn() -> u64>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let window = ResourceToolsWindow::new()?;
        window.set_mode(mode.into());
        window.set_output_dir(directory.into());
        crate::native_window::apply_theme(&window, dark);
        let rows = Rc::new(VecModel::default());
        window.set_rows(ModelRc::from(rows.clone()));
        let this = Rc::new(Self {
            window,
            rows,
            projects: RefCell::new(Vec::new()),
            harvest: RefCell::new(Vec::new()),
            torrent: RefCell::new(Vec::new()),
            task_id,
            pending: Cell::new(None),
            waiting_torrent: Cell::new(false),
            creating_project: Cell::new(false),
            listing_projects: Cell::new(false),
            refresh_pending: Cell::new(None),
            next_refresh: Cell::new(Instant::now()),
            rendered_project_id: RefCell::new(String::new()),
            request,
            batch,
            id,
        });
        let weak = Rc::downgrade(&this);
        this.window.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                if let Err(error) = this.action(&action) {
                    this.window.set_status(error.into());
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.on_toggle(move |id, selected| {
            if let Some(this) = weak.upgrade() {
                if let Some(index) = this.rows.iter().position(|row| row.id == id) {
                    if let Some(mut row) = this.rows.row_data(index) {
                        row.selected = selected;
                        this.rows.set_row_data(index, row);
                    }
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.on_select_project(move |index| {
            if let Some(this) = weak.upgrade() {
                this.window.set_project_index(index);
                this.render_project();
                this.next_refresh.set(Instant::now());
            }
        });
        if mode == "crawl" {
            let _ = this.action("list");
        }
        if mode == "torrent" && !this.task_id.is_empty() {
            this.command(CoreCommand::GetTaskTorrentFiles {
                task_id: this.task_id.clone(),
            });
        }
        Ok(this)
    }

    fn send(&self, request: CorePipeRequest, id: u64) -> bool {
        if (self.request)(request) {
            self.pending.set(Some(id));
            self.window.set_busy(true);
            self.window.set_status("".into());
            true
        } else {
            self.window.set_status("命令队列已满，请稍后重试".into());
            false
        }
    }
    fn command(&self, command: CoreCommand) {
        let id = (self.id)();
        let probe = matches!(command, CoreCommand::ProbeTorrent { .. });
        if self.send(
            CorePipeRequest::Command {
                request_id: id,
                command,
            },
            id,
        ) {
            self.waiting_torrent.set(probe);
        }
    }
    fn chosen(&self) -> HashSet<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| row.selected.then_some(index))
            .collect()
    }

    pub fn poll(&self) {
        if self.window.get_mode() != "crawl"
            || !self.window.window().is_visible()
            || self.window.get_creating()
            || self.pending.get().is_some()
            || self.refresh_pending.get().is_some()
            || Instant::now() < self.next_refresh.get()
        {
            return;
        }
        let Some(project_id) = self
            .projects
            .borrow()
            .get(self.window.get_project_index().max(0) as usize)
            .map(|p| p.id.clone())
        else {
            return;
        };
        let id = (self.id)();
        if (self.request)(CorePipeRequest::Crawl {
            request_id: id,
            request: CrawlRequest::Get { project_id },
        }) {
            self.refresh_pending.set(Some(id));
        }
        self.next_refresh
            .set(Instant::now() + Duration::from_secs(1));
    }

    fn action(&self, action: &str) -> Result<(), String> {
        if action == "close" {
            let _ = self.window.hide();
            return Ok(());
        }
        if self.window.get_busy() {
            return Ok(());
        }
        let mode = self.window.get_mode();
        let source = self.window.get_source().trim().to_owned();
        match action {
            "file" => {
                if let Some(path) =
                    file_dialogs::files(native_window::hwnd(self.window.window()), false)?.first()
                {
                    self.window.set_source(path.clone().into());
                }
            }
            "folder" => {
                if let Some(path) = file_dialogs::folder(native_window::hwnd(self.window.window()))?
                {
                    self.window.set_output_dir(path.into());
                }
            }
            "new_project" => {
                self.window.set_creating(true);
                self.rows.set_vec(Vec::new());
            }
            "select_all" | "select_none" => {
                for index in 0..self.rows.row_count() {
                    if let Some(mut row) = self.rows.row_data(index) {
                        row.selected = action == "select_all";
                        self.rows.set_row_data(index, row);
                    }
                }
            }
            "inspect" | "list" | "pause" | "resume" | "sync" | "probe" | "download" => {
                let id = (self.id)();
                if mode == "crawl" {
                    let selected = self
                        .projects
                        .borrow()
                        .get(self.window.get_project_index().max(0) as usize)
                        .map(|p| p.id.clone());
                    let request = match action {
                        "list" => selected.map_or(CrawlRequest::List, |project_id| {
                            CrawlRequest::Get { project_id }
                        }),
                        "inspect" => CrawlRequest::Create {
                            spec: self.crawl_spec()?,
                        },
                        "pause" => CrawlRequest::Pause {
                            project_id: selected.ok_or("请选择项目")?,
                        },
                        "resume" => CrawlRequest::Resume {
                            project_id: selected.ok_or("请选择项目")?,
                        },
                        "sync" => CrawlRequest::Sync {
                            project_id: selected.ok_or("请选择项目")?,
                        },
                        "download" => {
                            let chosen = self.chosen();
                            if chosen.is_empty() {
                                return Err("请选择资源".into());
                            }
                            let projects = self.projects.borrow();
                            let project = projects
                                .get(self.window.get_project_index().max(0) as usize)
                                .ok_or("请选择项目")?;
                            CrawlRequest::Enqueue {
                                project_id: project.id.clone(),
                                urls: project
                                    .resources
                                    .iter()
                                    .enumerate()
                                    .filter(|(index, _)| chosen.contains(index))
                                    .map(|(_, r)| r.url.clone())
                                    .collect(),
                            }
                        }
                        _ => return Ok(()),
                    };
                    let listing = matches!(request, CrawlRequest::List);
                    if self.send(
                        CorePipeRequest::Crawl {
                            request_id: id,
                            request,
                        },
                        id,
                    ) {
                        self.creating_project.set(action == "inspect");
                        self.listing_projects.set(listing);
                    }
                } else if mode == "zip" {
                    if source.is_empty() {
                        return Err("请输入 ZIP 路径或地址".into());
                    }
                    self.send(
                        CorePipeRequest::PreviewZip {
                            request_id: id,
                            source,
                        },
                        id,
                    );
                } else if mode == "torrent" {
                    if action == "download" {
                        let chosen = self.chosen();
                        if chosen.is_empty() {
                            return Err("至少选择一个文件".into());
                        }
                        let selections = self
                            .torrent
                            .borrow()
                            .iter()
                            .enumerate()
                            .map(|(index, file)| TorrentFileSelection {
                                index: file.index,
                                path: file.path.clone(),
                                selected: chosen.contains(&index),
                            })
                            .collect();
                        let command = if self.task_id.is_empty() {
                            CoreCommand::SelectTorrentFiles { source, selections }
                        } else {
                            CoreCommand::SetTaskTorrentFiles {
                                task_id: self.task_id.clone(),
                                selections,
                            }
                        };
                        self.command(command);
                    } else {
                        self.command(CoreCommand::ProbeTorrent { source });
                    }
                } else if action == "download" {
                    let chosen = self.chosen();
                    if chosen.is_empty() {
                        return Err("请选择资源".into());
                    }
                    let commands = self
                        .harvest
                        .borrow()
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| chosen.contains(index))
                        .map(|(_, link)| CoreCommand::CreateTask {
                            spec: TaskSpec {
                                url: link.url.clone(),
                                filename: link.filename.clone(),
                                resource_kind: match link.extension.as_str() {
                                    "m3u8" => ResourceKind::Hls,
                                    "mpd" => ResourceKind::Dash,
                                    _ => ResourceKind::File,
                                },
                                download_dir: self.window.get_output_dir().into(),
                                headers: std::collections::BTreeMap::from([(
                                    "Referer".into(),
                                    source.clone(),
                                )]),
                                ..Default::default()
                            },
                        })
                        .collect();
                    if (self.batch)(commands) {
                        self.window
                            .set_status("所选资源已提交，结果见任务列表".into());
                    }
                } else {
                    let probe_urls = if action == "probe" {
                        let chosen = self.chosen();
                        self.harvest
                            .borrow()
                            .iter()
                            .enumerate()
                            .filter(|(index, _)| chosen.contains(index))
                            .map(|(_, link)| link.url.clone())
                            .collect()
                    } else {
                        Vec::new()
                    };
                    self.command(CoreCommand::HarvestPage {
                        url: source,
                        referer: String::new(),
                        probe_urls,
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn crawl_spec(&self) -> Result<CrawlSpec, String> {
        let w = &self.window;
        let max_depth = w
            .get_max_depth()
            .parse::<u32>()
            .ok()
            .filter(|n| *n <= 10)
            .ok_or("最大深度应为 0 至 10")?;
        let max_pages = w
            .get_max_pages()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=500).contains(n))
            .ok_or("页面上限应为 1 至 500")?;
        let page_concurrency = w
            .get_concurrency()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=2).contains(n))
            .ok_or("页面并发应为 1 或 2")?;
        let sync_interval_secs = w
            .get_sync_interval()
            .parse::<u64>()
            .ok()
            .filter(|n| *n == 0 || *n >= 60)
            .ok_or("同步间隔应为 0 或至少 60 秒")?;
        let split = |text: slint::SharedString| {
            text.split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect()
        };
        Ok(CrawlSpec {
            entry_url: w.get_source().trim().into(),
            output_dir: w.get_output_dir().trim().into(),
            queue_id: w.get_queue_id().trim().into(),
            same_origin: w.get_same_origin(),
            offline: w.get_offline(),
            max_depth,
            max_pages,
            page_concurrency,
            sync_interval_secs,
            extensions: split(w.get_extensions()),
            include_paths: split(w.get_includes()),
            exclude_paths: split(w.get_excludes()),
        })
    }

    pub fn response(&self, response: &CorePipeResponse) {
        let id = serde_json::to_value(response)
            .ok()
            .and_then(|v| v.get("request_id").and_then(serde_json::Value::as_u64));
        if id.is_some() && self.refresh_pending.get() == id {
            self.refresh_pending.set(None);
            // A user command owns the next visible state. A background response
            // must not overwrite it or restore a project while creating another.
            if self.pending.get().is_none() && !self.window.get_creating() {
                if let CorePipeResponse::Crawl { projects, .. } = response {
                    let mut current = self.projects.borrow_mut();
                    for project in projects {
                        if let Some(item) = current.iter_mut().find(|item| item.id == project.id) {
                            *item = project.clone();
                        }
                    }
                    drop(current);
                    self.render_project();
                }
            }
            return;
        }
        if self.pending.get() != id || id.is_none() {
            return;
        }
        self.pending.set(None);
        // ProbeTorrent is acknowledged before metadata discovery finishes.
        // Only the matching result/error may enable another probe.
        self.window.set_busy(self.waiting_torrent.get());
        match response {
            CorePipeResponse::ZipPreview { preview, .. } => {
                self.window.set_summary(
                    format!(
                        "{} 项 · ZIP64：{} · 读取元数据 {}",
                        preview.entries.len(),
                        preview.zip64,
                        crate::format_bytes(preview.metadata_bytes_read)
                    )
                    .into(),
                );
                self.rows.set_vec(
                    preview
                        .entries
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| ResourceRow {
                            id: index.to_string().into(),
                            label: entry.path.clone().into(),
                            detail: format!(
                                "{} · 压缩后 {} · 方法 {}{}",
                                crate::format_bytes(entry.uncompressed_bytes),
                                crate::format_bytes(entry.compressed_bytes),
                                entry.method,
                                if entry.encrypted { " · 加密" } else { "" }
                            )
                            .into(),
                            selected: false,
                        })
                        .collect::<Vec<_>>(),
                );
            }
            CorePipeResponse::Crawl { projects, .. } => {
                let selected = if self.creating_project.replace(false) {
                    projects.first().map(|p| p.id.clone())
                } else {
                    self.projects
                        .borrow()
                        .get(self.window.get_project_index().max(0) as usize)
                        .map(|p| p.id.clone())
                };
                let mut current = self.projects.borrow_mut();
                if self.listing_projects.replace(false) {
                    current.clear();
                }
                for project in projects {
                    if let Some(item) = current.iter_mut().find(|item| item.id == project.id) {
                        *item = project.clone();
                    } else {
                        current.push(project.clone());
                    }
                }
                self.window.set_projects(ModelRc::new(VecModel::from(
                    current
                        .iter()
                        .map(|p| {
                            format!(
                                "{} · {}",
                                crate::safe_display_url(&p.spec.entry_url),
                                p.status
                            )
                            .into()
                        })
                        .collect::<Vec<_>>(),
                )));
                let index = selected
                    .and_then(|id| current.iter().position(|p| p.id == id))
                    .unwrap_or(current.len().saturating_sub(1));
                self.window.set_project_index(index as i32);
                drop(current);
                self.render_project();
            }
            // Workbench applies these events once through event().
            CorePipeResponse::Events { .. } => {}
            CorePipeResponse::Error { message, .. } => {
                self.waiting_torrent.set(false);
                self.creating_project.set(false);
                self.listing_projects.set(false);
                self.window.set_busy(false);
                self.window.set_status(message.clone().into());
            }
            _ => {}
        }
    }

    pub fn event(&self, event: &CoreEvent) {
        match event {
            CoreEvent::HarvestResult { url, links }
            | CoreEvent::HarvestProbeResult { url, links }
                if self.window.get_mode() == "harvest"
                    && self.window.get_source().trim() == url =>
            {
                *self.harvest.borrow_mut() = links.clone();
                self.window.set_busy(false);
                self.rows.set_vec(
                    links
                        .iter()
                        .enumerate()
                        .map(|(index, link)| ResourceRow {
                            id: index.to_string().into(),
                            label: if link.filename.is_empty() {
                                crate::safe_display_url(&link.url)
                            } else {
                                link.filename.clone()
                            }
                            .into(),
                            detail: format!(
                                "{} · {} · {}",
                                link.category,
                                if link.size > 0 {
                                    crate::format_bytes(link.size)
                                } else {
                                    "大小未知".into()
                                },
                                link.extension
                            )
                            .into(),
                            selected: false,
                        })
                        .collect::<Vec<_>>(),
                );
            }
            CoreEvent::TaskTorrentFiles {
                task_id,
                files,
                selections,
                ..
            } if self.window.get_mode() == "torrent" && *task_id == self.task_id => {
                self.render_torrent(files, selections)
            }
            CoreEvent::TorrentProbeResult { source, files, .. }
                if self.window.get_mode() == "torrent"
                    && self.window.get_source().trim() == source =>
            {
                self.render_torrent(files, &[])
            }
            CoreEvent::TorrentSelectionResult { source, .. }
                if self.window.get_mode() == "torrent"
                    && self.window.get_source().trim() == source =>
            {
                self.window
                    .set_status("文件选择已保存，添加该种子任务后生效".into())
            }
            CoreEvent::Error { code, message }
                if self.waiting_torrent.get()
                    && matches!(code.as_str(), "torrent_probe_failed" | "torrent_probe_busy") =>
            {
                self.waiting_torrent.set(false);
                self.window.set_busy(false);
                self.window.set_status(message.clone().into());
            }
            _ => {}
        }
    }

    fn render_torrent(&self, files: &[TorrentFileEntry], selections: &[TorrentFileSelection]) {
        *self.torrent.borrow_mut() = files.to_vec();
        self.waiting_torrent.set(false);
        self.window.set_busy(false);
        self.rows.set_vec(
            files
                .iter()
                .enumerate()
                .map(|(index, file)| ResourceRow {
                    id: index.to_string().into(),
                    label: file.path.clone().into(),
                    detail: crate::format_bytes(file.size).into(),
                    selected: selections
                        .iter()
                        .find(|s| s.index == file.index)
                        .is_none_or(|s| s.selected),
                })
                .collect::<Vec<_>>(),
        );
        self.window.set_summary(
            format!(
                "{} 个文件 · {}",
                files.len(),
                crate::format_bytes(files.iter().map(|f| f.size).sum())
            )
            .into(),
        );
    }

    fn render_project(&self) {
        let projects = self.projects.borrow();
        let Some(project) = projects.get(self.window.get_project_index().max(0) as usize) else {
            self.window.set_creating(true);
            self.rows.set_vec(Vec::new());
            self.window.set_summary("尚无项目，可创建抓站项目".into());
            return;
        };
        let selected: HashSet<_> = if *self.rendered_project_id.borrow() == project.id {
            self.rows
                .iter()
                .filter(|r| r.selected)
                .map(|r| r.id.to_string())
                .collect()
        } else {
            HashSet::new()
        };
        *self.rendered_project_id.borrow_mut() = project.id.clone();
        self.window.set_creating(false);
        self.window.set_summary(
            format!(
                "{} · 已访问 {} 页 · {} 个资源 · {}",
                project.status,
                project.visited_pages,
                project.resources.len(),
                project.reason
            )
            .into(),
        );
        let rows: Vec<_> = project
            .resources
            .iter()
            .map(|r| ResourceRow {
                id: r.url.clone().into(),
                label: crate::safe_display_url(&r.url).into(),
                detail: format!(
                    "{} · {} {}",
                    r.kind,
                    if r.saved {
                        "已保存"
                    } else if r.selected {
                        "已确认下载"
                    } else {
                        "待确认"
                    },
                    r.error
                )
                .into(),
                selected: selected.contains(&r.url),
            })
            .collect();
        if self.rows.row_count() == rows.len()
            && self
                .rows
                .iter()
                .zip(&rows)
                .all(|(old, new)| old.id == new.id)
        {
            for (index, row) in rows.into_iter().enumerate() {
                if self.rows.row_data(index).as_ref() != Some(&row) {
                    self.rows.set_row_data(index, row);
                }
            }
        } else {
            self.rows.set_vec(rows);
        }
    }
}
