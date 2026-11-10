use crate::{native_window, MediaView, PlayerWindow, WorkbenchWindow};
use base64::Engine;
use hls_client::{
    CoreCommand, CoreEvent, CoreIpcClient, CorePipeRequest, CorePipeResponse, EventEnvelope,
    PlayerEmbed,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    io::Cursor,
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct VideoHost {
    parent: Option<i64>,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl VideoHost {
    fn detached() -> Self {
        Self {
            parent: None,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }
}

pub struct MediaController {
    main: WorkbenchWindow,
    pip: RefCell<Option<PlayerWindow>>,
    detached: Cell<bool>,
    active: Cell<bool>,
    session: Cell<u64>,
    layout: Cell<u64>,
    sequence: Cell<u64>,
    task: RefCell<String>,
    audio_ids: RefCell<Vec<String>>,
    subtitle_ids: RefCell<Vec<String>>,
    supported: Cell<bool>,
    controls_supported: Cell<bool>,
    confirmed_host: RefCell<Option<VideoHost>>,
    pending_embed: RefCell<Option<(u64, u64, VideoHost)>>,
    pending_status: Cell<Option<u64>>,
    pending_thumbnail: Cell<Option<(u64, u64, f64, bool)>>,
    hover_preview: Cell<Option<(f64, Instant)>>,
    shown_preview: Cell<Option<f64>>,
    automatic_preview: Cell<bool>,
    next_embed_attempt: Cell<Instant>,
    next_status: Cell<Instant>,
    seek: Cell<Option<f32>>,
    volume: Cell<Option<f32>>,
    request: Rc<dyn Fn(CorePipeRequest) -> bool>,
    id: Rc<dyn Fn() -> u64>,
    thumbnails: mpsc::SyncSender<(u64, String, f64)>,
    subtitles: RefCell<crate::player_subtitles::PlayerSubtitles>,
    position_updated: Cell<Instant>,
    pending_subtitle_reset: Cell<Option<u64>>,
    timer: slint::Timer,
}

impl MediaController {
    pub fn apply_theme(&self, dark: bool) {
        if let Some(window) = self.pip.borrow().as_ref() {
            crate::native_window::apply_theme(window, dark);
        }
    }

    pub fn keeps_ui_alive(&self) -> bool {
        self.active.get() || self.pending_thumbnail.get().is_some()
    }

    pub fn main_closed(self: &Rc<Self>) {
        if self.active.get() && !self.detached.get() {
            self.show_pip();
        }
    }

    pub fn new(
        main: WorkbenchWindow,
        request: Rc<dyn Fn(CorePipeRequest) -> bool>,
        id: Rc<dyn Fn() -> u64>,
    ) -> Rc<Self> {
        // Thumbnail decoding can wait for FFmpeg. It has a separate connection
        // and a one-item queue so it cannot block pause/seek/download commands.
        let (thumbnails, jobs) = mpsc::sync_channel::<(u64, String, f64)>(1);
        let weak = main.as_weak();
        std::thread::spawn(move || {
            let mut client = None;
            while let Ok((request_id, task_id, seconds)) = jobs.recv() {
                let result = (|| {
                    if client.is_none() {
                        client = Some(CoreIpcClient::connect()?);
                    }
                    client
                        .as_mut()
                        .unwrap()
                        .request(&CorePipeRequest::PlayerThumbnail {
                            request_id,
                            task_id,
                            seconds: Some(seconds),
                        })
                })();
                if result.is_err() {
                    client = None;
                }
                let response = result.unwrap_or_else(|message| CorePipeResponse::Error {
                    request_id: Some(request_id),
                    code: "thumbnail_connection".into(),
                    message,
                });
                if let Ok(json) = serde_json::to_string(&response) {
                    if weak
                        .upgrade_in_event_loop(move |ui| ui.invoke_response_json(json.into()))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        let now = Instant::now();
        let this = Rc::new(Self {
            main,
            pip: RefCell::new(None),
            detached: Cell::new(false),
            active: Cell::new(false),
            session: Cell::new(0),
            layout: Cell::new(0),
            sequence: Cell::new(0),
            task: RefCell::new(String::new()),
            audio_ids: RefCell::new(Vec::new()),
            subtitle_ids: RefCell::new(Vec::new()),
            supported: Cell::new(false),
            controls_supported: Cell::new(false),
            confirmed_host: RefCell::new(None),
            pending_embed: RefCell::new(None),
            pending_status: Cell::new(None),
            pending_thumbnail: Cell::new(None),
            hover_preview: Cell::new(None),
            shown_preview: Cell::new(None),
            automatic_preview: Cell::new(false),
            next_embed_attempt: Cell::new(now),
            next_status: Cell::new(now),
            seek: Cell::new(None),
            volume: Cell::new(None),
            request,
            id,
            thumbnails,
            subtitles: RefCell::new(Default::default()),
            position_updated: Cell::new(now),
            pending_subtitle_reset: Cell::new(None),
            timer: slint::Timer::default(),
        });
        let weak = Rc::downgrade(&this);
        this.main.on_media_action(move |action| {
            if let Some(this) = weak.upgrade() {
                this.action(&action)
            }
        });
        let weak = Rc::downgrade(&this);
        this.main.on_media_seek(move |value| {
            if let Some(this) = weak.upgrade() {
                this.seek.set(Some(value));
            }
        });
        let weak = Rc::downgrade(&this);
        this.main.on_media_preview(move |value| {
            if let Some(this) = weak.upgrade() {
                this.hover(value);
            }
        });
        let weak = Rc::downgrade(&this);
        this.main.on_media_volume(move |value| {
            if let Some(this) = weak.upgrade() {
                this.volume.set(Some(value));
            }
        });
        let weak = Rc::downgrade(&this);
        this.main.on_media_audio(move |index| {
            if let Some(this) = weak.upgrade() {
                this.track(false, index)
            }
        });
        let weak = Rc::downgrade(&this);
        this.main.on_media_subtitle(move |index| {
            if let Some(this) = weak.upgrade() {
                this.track(true, index)
            }
        });
        let weak = Rc::downgrade(&this);
        this.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(150),
            move || {
                if let Some(this) = weak.upgrade() {
                    this.tick()
                }
            },
        );
        (this.request)(CorePipeRequest::Capabilities {
            request_id: (this.id)(),
        });
        this.status();
        this
    }

    fn command(&self, action: String) -> bool {
        if !self.controls_supported.get() {
            self.notice("Core 不支持此版本的播放控制，请使用同版本组件");
            return false;
        }
        let reset = action.starts_with("seek")
            || action.starts_with("speed:")
            || action.starts_with("audio:");
        let id = (self.id)();
        let sent = (self.request)(CorePipeRequest::PlayerControl {
            request_id: id,
            session_generation: self.session.get(),
            action,
        });
        if sent && reset {
            self.pending_subtitle_reset.set(Some(id));
            self.subtitles.borrow().suspend();
        }
        sent
    }

    fn status(&self) {
        if self.pending_status.get().is_some() {
            return;
        }
        let id = (self.id)();
        if (self.request)(CorePipeRequest::Command {
            request_id: id,
            command: CoreCommand::PlayerControl {
                action: "status".into(),
            },
        }) {
            self.pending_status.set(Some(id));
            self.next_status
                .set(Instant::now() + Duration::from_millis(600));
        }
    }

    fn notice(&self, text: &str) {
        self.main.set_media_notice(text.into());
        if let Some(pip) = self.pip.borrow().as_ref() {
            pip.set_notice(text.into());
        }
    }

    fn action(self: &Rc<Self>, action: &str) {
        match action {
            "translate" => {
                self.subtitles.borrow_mut().toggle();
            }
            "pip" => self.show_pip(),
            "dock" => {
                self.detached.set(false);
                self.main.set_media_detached(false);
                if let Some(pip) = self.pip.borrow().as_ref() {
                    pip.window().set_fullscreen(false);
                    let _ = pip.hide();
                }
                let _ = self.main.show();
                native_window::activate(self.main.window());
            }
            "fullscreen" => {
                if self.detached.get() {
                    if let Some(pip) = self.pip.borrow().as_ref() {
                        let full = !pip.window().is_fullscreen();
                        pip.set_fullscreen(full);
                        pip.window().set_fullscreen(full);
                    }
                } else {
                    let full = !self.main.window().is_fullscreen();
                    self.main.set_media_fullscreen(full);
                    self.main.window().set_fullscreen(full);
                }
            }
            "escape" => {
                if self.detached.get() {
                    if let Some(pip) = self.pip.borrow().as_ref() {
                        pip.set_fullscreen(false);
                        pip.window().set_fullscreen(false);
                    }
                } else {
                    self.main.set_media_fullscreen(false);
                    self.main.window().set_fullscreen(false);
                }
            }
            "clear_preview" => {
                self.hover_preview.set(None);
                self.shown_preview.set(None);
                self.automatic_preview.set(false);
                self.set_thumbnail(slint::Image::default());
            }
            "preview" => {
                self.hover_preview.set(None);
                self.automatic_preview.set(false);
                self.preview(self.main.get_media_state().position as f64, false);
            }
            value => {
                if self.active.get() {
                    self.subtitles.borrow().user_control(value);
                    self.command(value.into());
                }
            }
        }
    }

    fn show_pip(self: &Rc<Self>) {
        if self.pip.borrow().is_none() {
            let Ok(pip) = PlayerWindow::new() else {
                self.notice("无法创建画中画窗口");
                return;
            };
            let weak = Rc::downgrade(self);
            pip.on_action(move |action| {
                if let Some(this) = weak.upgrade() {
                    this.action(&action)
                }
            });
            let weak = Rc::downgrade(self);
            pip.on_seek(move |value| {
                if let Some(this) = weak.upgrade() {
                    this.seek.set(Some(value));
                }
            });
            let weak = Rc::downgrade(self);
            pip.on_preview(move |value| {
                if let Some(this) = weak.upgrade() {
                    this.hover(value);
                }
            });
            let weak = Rc::downgrade(self);
            pip.on_volume(move |value| {
                if let Some(this) = weak.upgrade() {
                    this.volume.set(Some(value));
                }
            });
            let weak = Rc::downgrade(self);
            pip.on_audio(move |index| {
                if let Some(this) = weak.upgrade() {
                    this.track(false, index)
                }
            });
            let weak = Rc::downgrade(self);
            pip.on_subtitle(move |index| {
                if let Some(this) = weak.upgrade() {
                    this.track(true, index)
                }
            });
            let weak = Rc::downgrade(self);
            pip.window().on_close_requested(move || {
                if let Some(this) = weak.upgrade() {
                    this.action("dock");
                }
                slint::CloseRequestResponse::KeepWindowShown
            });
            *self.pip.borrow_mut() = Some(pip);
        }
        self.main.set_media_fullscreen(false);
        self.main.window().set_fullscreen(false);
        self.detached.set(true);
        self.main.set_media_detached(true);
        if let Some(pip) = self.pip.borrow().as_ref() {
            pip.set_state(self.main.get_media_state());
            pip.set_thumbnail(self.main.get_media_thumbnail());
            native_window::apply_theme(pip, self.main.global::<crate::Tokens>().get_dark());
            pip.set_notice(self.main.get_media_notice());
            let _ = pip.show();
            native_window::prepare_dialog(pip, true, true, None);
        }
    }

    fn track(&self, subtitle: bool, index: i32) {
        if !self.active.get() {
            return;
        }
        let ids = if subtitle {
            self.subtitle_ids.borrow()
        } else {
            self.audio_ids.borrow()
        };
        if let Some(id) = ids.get(index.max(0) as usize) {
            self.command(format!(
                "{}:{id}",
                if subtitle { "subtitle" } else { "audio" }
            ));
        }
    }

    fn hover(&self, seconds: f32) {
        if seconds < 0.0 || !seconds.is_finite() {
            self.hover_preview.set(None);
            if self.automatic_preview.replace(false) {
                self.shown_preview.set(None);
                self.set_thumbnail(slint::Image::default());
            }
            return;
        }
        // One-second buckets and a short quiet period avoid spawning FFmpeg
        // for every mouse pixel. Late frames only apply to the hovered bucket.
        let seconds = (seconds as f64).floor();
        if self
            .hover_preview
            .get()
            .is_none_or(|(target, _)| target != seconds)
        {
            self.hover_preview
                .set(Some((seconds, Instant::now() + Duration::from_millis(300))));
        }
    }

    fn preview(&self, seconds: f64, automatic: bool) {
        if !self.active.get() || self.pending_thumbnail.get().is_some() {
            return;
        }
        let id = (self.id)();
        if self
            .thumbnails
            .try_send((id, self.task.borrow().clone(), seconds))
            .is_ok()
        {
            self.pending_thumbnail
                .set(Some((id, self.session.get(), seconds, automatic)));
            if !automatic {
                self.notice("正在读取预览帧…");
            }
        }
    }

    pub fn event(&self, envelope: &EventEnvelope) {
        let CoreEvent::PlayerSession {
            active,
            title,
            session_generation,
            layout_generation,
            task_id,
            paused,
            speed,
            position_seconds,
            duration_seconds,
            volume,
            muted,
            buffering,
            tracks,
            ..
        } = &envelope.event
        else {
            return;
        };
        if envelope.sequence != 0 && envelope.sequence <= self.sequence.get() {
            return;
        }
        self.sequence.set(envelope.sequence);
        if self.session.replace(*session_generation) != *session_generation {
            self.layout.set(0);
            self.confirmed_host.borrow_mut().take();
            self.pending_embed.borrow_mut().take();
            self.pending_thumbnail.set(None);
            self.set_thumbnail(slint::Image::default());
            self.seek.set(None);
            self.volume.set(None);
            self.hover_preview.set(None);
            self.shown_preview.set(None);
            self.automatic_preview.set(false);
        }
        self.layout.set(self.layout.get().max(*layout_generation));
        self.active.set(*active);
        *self.task.borrow_mut() = task_id.clone();
        let previous = self.main.get_media_state();
        let expected = previous.position as f64
            + if !previous.paused && !previous.buffering {
                self.position_updated.get().elapsed().as_secs_f64() * previous.speed as f64
            } else {
                0.0
            };
        self.position_updated.set(Instant::now());
        self.subtitles
            .borrow()
            .update(crate::player_subtitles::Playback {
                generation: if *active { *session_generation } else { 0 },
                position: *position_seconds,
                duration: *duration_seconds,
                paused: *paused,
                buffering: *buffering || self.pending_subtitle_reset.get().is_some(),
                speed: *speed,
            });
        let audio_changed = tracks
            .iter()
            .filter(|track| track.kind == "audio")
            .position(|track| track.selected)
            .unwrap_or(0) as i32
            != previous.audio_index;
        if (expected - *position_seconds).abs() > 1.0
            || previous.speed != *speed as f32
            || audio_changed
        {
            self.subtitles.borrow().reset();
        }
        self.main.set_media_visible(*active);
        if !active {
            self.main.set_media_fullscreen(false);
            self.main.window().set_fullscreen(false);
            if let Some(pip) = self.pip.borrow().as_ref() {
                pip.window().set_fullscreen(false);
                let _ = pip.hide();
            }
            self.detached.set(false);
            self.main.set_media_detached(false);
            return;
        }
        let mut audio = Vec::new();
        let mut subtitles = vec!["关闭字幕".into()];
        let mut audio_ids = Vec::new();
        let mut subtitle_ids = vec!["no".to_owned()];
        let mut audio_index = 0;
        let mut subtitle_index = 0;
        for track in tracks {
            let label = if track.title.is_empty() {
                format!("{} {}", track.id, track.language)
            } else {
                format!("{} {}", track.title, track.language)
            };
            if track.kind == "audio" {
                if track.selected {
                    audio_index = audio.len() as i32;
                }
                audio.push(label.into());
                audio_ids.push(track.id.clone());
            } else if track.kind == "sub" {
                if track.selected {
                    subtitle_index = subtitles.len() as i32;
                }
                subtitles.push(label.into());
                subtitle_ids.push(track.id.clone());
            }
        }
        *self.audio_ids.borrow_mut() = audio_ids;
        *self.subtitle_ids.borrow_mut() = subtitle_ids;
        let (caption, translation_notice) = self.subtitles.borrow().display(*position_seconds);
        let state = MediaView {
            title: title.clone().into(),
            paused: *paused,
            speed: *speed as f32,
            position: *position_seconds as f32,
            duration: *duration_seconds as f32,
            time: format!(
                "{} / {}",
                clock(*position_seconds),
                clock(*duration_seconds)
            )
            .into(),
            volume: *volume as f32,
            muted: *muted,
            buffering: *buffering,
            audio: ModelRc::new(VecModel::from(audio)),
            subtitles: ModelRc::new(VecModel::from(subtitles)),
            audio_index,
            subtitle_index,
            translating: self.subtitles.borrow().enabled(),
            caption: caption.into(),
            translation_notice: translation_notice.into(),
        };
        self.main.set_media_state(state.clone());
        if let Some(pip) = self.pip.borrow().as_ref() {
            pip.set_state(state);
        }
    }

    pub fn response(&self, response: &CorePipeResponse) {
        let id = match response {
            CorePipeResponse::Events { request_id, .. }
            | CorePipeResponse::Capabilities { request_id, .. }
            | CorePipeResponse::PlayerThumbnail { request_id, .. } => Some(*request_id),
            CorePipeResponse::Error { request_id, .. } => *request_id,
            _ => None,
        };
        if let CorePipeResponse::Capabilities { commands, .. } = response {
            self.supported
                .set(commands.iter().any(|c| c == "player_embed_generation"));
            self.controls_supported
                .set(commands.iter().any(|c| c == "player_control_generation"));
        }
        if let Some(id) = id {
            let success = matches!(response, CorePipeResponse::Events { events, .. }
                if !events.iter().any(|event| matches!(event.event, CoreEvent::Error { .. })));
            if self.subtitles.borrow().playback_response(id, success) && !success {
                self.notice("字幕播放控制失败，请手动控制播放");
            }
        }
        if self.pending_subtitle_reset.get().is_some() && self.pending_subtitle_reset.get() == id {
            self.pending_subtitle_reset.set(None);
            self.subtitles.borrow().suspend();
            if matches!(response, CorePipeResponse::Error { .. }) {
                self.status();
            }
        }
        if self.pending_status.get().is_some() && self.pending_status.get() == id {
            self.pending_status.set(None);
        }
        let pending = self
            .pending_embed
            .borrow()
            .as_ref()
            .filter(|(request_id, _, _)| Some(*request_id) == id)
            .cloned();
        let had_pending = pending.is_some();
        if let Some((_, session, host)) = pending {
            self.pending_embed.borrow_mut().take();
            let success = matches!(response,CorePipeResponse::Events {events,..} if !events.iter().any(|e|matches!(e.event,CoreEvent::Error {..})));
            if success && session == self.session.get() {
                *self.confirmed_host.borrow_mut() = Some(host);
            } else {
                self.next_embed_attempt
                    .set(Instant::now() + Duration::from_secs(2));
                self.status();
            }
        }
        if let Some((request_id, session, seconds, automatic)) = self
            .pending_thumbnail
            .get()
            .filter(|(request_id, _, _, _)| Some(*request_id) == id)
        {
            let _ = request_id;
            self.pending_thumbnail.set(None);
            if session != self.session.get() || !self.active.get() {
                return;
            }
            if automatic
                && self
                    .hover_preview
                    .get()
                    .is_none_or(|(target, _)| target != seconds)
            {
                return;
            }
            if let CorePipeResponse::PlayerThumbnail { jpeg_base64, .. } = response {
                match thumbnail(jpeg_base64) {
                    Ok(image) => {
                        self.set_thumbnail(image);
                        self.shown_preview.set(Some(seconds));
                        self.automatic_preview.set(automatic);
                        if !automatic {
                            self.notice("");
                        }
                    }
                    Err(message) => {
                        self.shown_preview.set(Some(seconds));
                        if !automatic {
                            self.notice(&message);
                        }
                    }
                }
            } else if let CorePipeResponse::Error { message, .. } = response {
                self.shown_preview.set(Some(seconds));
                if !automatic {
                    self.notice(message);
                }
            }
        }
        if let CorePipeResponse::Error { message, .. } = response {
            if had_pending {
                self.notice(message);
            }
        }
    }

    pub fn resynchronize(&self) {
        // Core restart resets event numbering. The new snapshot establishes a
        // fresh stream; playback state is queried independently of task rows.
        self.sequence.set(0);
        self.layout.set(0);
        self.confirmed_host.borrow_mut().take();
        self.pending_embed.borrow_mut().take();
        self.pending_status.set(None);
        self.pending_thumbnail.set(None);
        self.pending_subtitle_reset.set(None);
        self.seek.set(None);
        self.volume.set(None);
        self.set_thumbnail(slint::Image::default());
        self.hover_preview.set(None);
        self.shown_preview.set(None);
        self.automatic_preview.set(false);
        self.subtitles.borrow().update(Default::default());
        (self.request)(CorePipeRequest::Capabilities {
            request_id: (self.id)(),
        });
        self.status();
    }

    fn set_thumbnail(&self, image: slint::Image) {
        self.main.set_media_thumbnail(image.clone());
        if let Some(pip) = self.pip.borrow().as_ref() {
            pip.set_thumbnail(image);
        }
    }

    fn tick(&self) {
        if !self.active.get() {
            return;
        }
        if let Some(value) = self.seek.take() {
            self.command(format!("seek_to:{value:.3}"));
        }
        let subtitles = self.subtitles.borrow();
        let mut view = self.main.get_media_state();
        let position = view.position as f64
            + if !view.paused && !view.buffering {
                self.position_updated.get().elapsed().as_secs_f64().min(1.0) * view.speed as f64
            } else {
                0.0
            };
        let (caption, notice) = subtitles.display(position);
        if self.controls_supported.get() {
            if let Some(paused) = subtitles.playback_action(position) {
                let id = (self.id)();
                subtitles.playback_command_sent(id, paused);
                let sent = (self.request)(CorePipeRequest::PlayerControl {
                    request_id: id,
                    session_generation: self.session.get(),
                    action: if paused { "pause" } else { "resume" }.into(),
                });
                if !sent {
                    subtitles.playback_response(id, false);
                    self.notice("字幕播放控制未发送，请手动控制播放");
                }
            }
        }
        view.translating = subtitles.enabled();
        view.caption = caption.into();
        view.translation_notice = notice.into();
        self.main.set_media_state(view.clone());
        if let Some(pip) = self.pip.borrow().as_ref() {
            pip.set_state(view);
        }
        if let Some(value) = self.volume.take() {
            self.command(format!("volume:{value:.1}"));
        }
        if let Some((seconds, ready)) = self.hover_preview.get() {
            if Instant::now() >= ready && self.shown_preview.get() != Some(seconds) {
                self.preview(seconds, true);
            }
        }
        let host = self.host();
        if host.parent.is_some() && Instant::now() >= self.next_status.get() {
            self.status();
        }
        if !self.supported.get()
            || self.pending_embed.borrow().is_some()
            || Instant::now() < self.next_embed_attempt.get()
            || self.confirmed_host.borrow().as_ref() == Some(&host)
        {
            return;
        }
        let id = (self.id)();
        let generation = self.layout.get().saturating_add(1);
        let embed = PlayerEmbed {
            session_generation: self.session.get(),
            layout_generation: generation,
            parent_hwnd: host.parent,
            x: host.x,
            y: host.y,
            width: host.width,
            height: host.height,
        };
        if (self.request)(CorePipeRequest::PlayerEmbed {
            request_id: id,
            embed,
        }) {
            self.layout.set(generation);
            *self.pending_embed.borrow_mut() = Some((id, self.session.get(), host));
        }
    }

    fn host(&self) -> VideoHost {
        if !self.detached.get() && (self.main.get_context_open() || self.main.get_tools_open()) {
            return VideoHost::detached();
        }
        if self.detached.get() {
            if let Some(pip) = self.pip.borrow().as_ref() {
                return video_host(
                    pip.window(),
                    pip.get_video_x(),
                    pip.get_video_y(),
                    pip.get_video_width(),
                    pip.get_video_height(),
                );
            }
        }
        video_host(
            self.main.window(),
            self.main.get_video_x(),
            self.main.get_video_y(),
            self.main.get_video_width(),
            self.main.get_video_height(),
        )
    }
}

fn video_host(window: &slint::Window, x: f32, y: f32, width: f32, height: f32) -> VideoHost {
    if !window.is_visible() || width <= 0.0 || height <= 0.0 {
        return VideoHost::detached();
    }
    #[cfg(windows)]
    {
        use slint::winit_030::WinitWindowAccessor;
        if window
            .with_winit_window(|native| native.is_minimized().unwrap_or(false))
            .unwrap_or(false)
        {
            return VideoHost::detached();
        }
    }
    let Some(parent) = native_window::hwnd(window) else {
        return VideoHost::detached();
    };
    let scale = window.scale_factor();
    VideoHost {
        parent: Some(parent),
        x: (x * scale).round().max(0.0) as i32,
        y: (y * scale).round().max(0.0) as i32,
        width: (width * scale).round().clamp(1.0, 16384.0) as i32,
        height: (height * scale).round().clamp(1.0, 16384.0) as i32,
    }
}

fn clock(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0) as u64
    } else {
        0
    };
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn thumbnail(encoded: &str) -> Result<slint::Image, String> {
    if encoded.len() > 512 * 1024 {
        return Err("预览图超过大小限制".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "预览图编码无效")?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(512);
    limits.max_image_height = Some(512);
    limits.max_alloc = Some(2 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("预览图无法读取：{error}"))?
        .to_rgba8();
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        image.as_raw(),
        image.width(),
        image.height(),
    );
    Ok(slint::Image::from_rgba8(buffer))
}
