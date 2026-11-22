//! Offscreen software rendering for visual fixtures.
//!
//! `HLS_V7_RENDER_PNG=<out.png>` makes `--visual-fixture <kind>` render the
//! production components through Slint's software renderer into an image
//! instead of opening native windows. It exercises the same `.slint` layout,
//! theme tokens and fixture data as the Windows fixture, so screenshots taken
//! on a headless CI or Linux box are comparable before/after a UI change.
//! Optional: `HLS_V7_RENDER_SIZE=<w>x<h>` (logical px) and
//! `HLS_V7_RENDER_SCALE=<factor>` for high-DPI frames.
//! `HLS_V7_RENDER_INPUT=tab;tab;hover:x,y;press:x,y` replays input before the
//! frame so keyboard focus, hover and pressed states can be captured.
//!
//! Every window the fixture creates is rendered; the first is written to the
//! requested path and further windows to `<stem>-<n>.png`.
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::Rgb8Pixel;
use slint::{LogicalSize, PhysicalSize};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub struct OffscreenPlatform {
    windows: RefCell<Vec<Rc<MinimalSoftwareWindow>>>,
    output: PathBuf,
    size: Option<(f32, f32)>,
    scale: f32,
}

impl OffscreenPlatform {
    pub fn from_env() -> Option<Self> {
        let output = PathBuf::from(std::env::var_os("HLS_V7_RENDER_PNG")?);
        let size = std::env::var("HLS_V7_RENDER_SIZE").ok().and_then(|value| {
            let (width, height) = value.split_once('x')?;
            Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
        });
        let scale = std::env::var("HLS_V7_RENDER_SCALE")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|value: &f32| *value >= 0.5 && *value <= 4.0)
            .unwrap_or(1.0);
        Some(Self {
            windows: RefCell::new(Vec::new()),
            output,
            size,
            scale,
        })
    }

    fn path_for(&self, index: usize) -> PathBuf {
        if index == 0 {
            return self.output.clone();
        }
        let stem = self
            .output
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "render".into());
        self.output.with_file_name(format!("{stem}-{index}.png"))
    }
}

impl Platform for OffscreenPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.windows.borrow_mut().push(Rc::clone(&window));
        Ok(window)
    }

    fn run_event_loop(&self) -> Result<(), slint::PlatformError> {
        let windows = self.windows.borrow().clone();
        for (index, window) in windows.iter().enumerate() {
            window.dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: self.scale,
            });
            let logical = match self.size {
                Some((width, height)) => LogicalSize::new(width, height),
                None => {
                    let current = window.window().size().to_logical(self.scale);
                    if current.width >= 1.0 && current.height >= 1.0 {
                        current
                    } else {
                        LogicalSize::new(520.0, 420.0)
                    }
                }
            };
            window.window().set_size(logical);
            window.dispatch_event(WindowEvent::Resized { size: logical });
            // Settle layout, timers and (reduced) animations before the frame.
            for _ in 0..3 {
                slint::platform::update_timers_and_animations();
                window.request_redraw();
                let _ = render(window);
            }
            if let Ok(script) = std::env::var("HLS_V7_RENDER_INPUT") {
                replay_input(window, &script);
                for _ in 0..3 {
                    slint::platform::update_timers_and_animations();
                    window.request_redraw();
                    let _ = render(window);
                }
            }
            // 动效逐帧：HLS_V7_RENDER_FRAMES=张数，HLS_V7_RENDER_FRAME_MS=间隔（真实时间）。
            // 与 HLS_V7_RENDER_MOTION=1 一起用（fixture 默认开启减少动态效果）。
            if let Some(frames) = std::env::var("HLS_V7_RENDER_FRAMES")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
            {
                let step = std::env::var("HLS_V7_RENDER_FRAME_MS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(40);
                let stem = self.path_for(index).with_extension("");
                for frame in 0..frames.min(240) {
                    // HLS_V7_RENDER_FRAME_INPUT="0:press:x,y|6:release:x,y"：在指定帧回放输入。
                    if let Ok(script) = std::env::var("HLS_V7_RENDER_FRAME_INPUT") {
                        for entry in script.split('|') {
                            if let Some((at, step)) = entry.split_once(':') {
                                if at.trim().parse::<usize>().ok() == Some(frame) {
                                    replay_input(window, step);
                                }
                            }
                        }
                    }
                    slint::platform::update_timers_and_animations();
                    if let Some(pixels) = render(window) {
                        let path =
                            std::path::PathBuf::from(format!("{}-f{frame:03}.png", stem.display()));
                        save_png(&path, window.window().size(), &pixels)
                            .map_err(slint::PlatformError::Other)?;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(step));
                }
            }
            let physical = window.window().size();
            let pixels = render(window)
                .ok_or_else(|| slint::PlatformError::Other("nothing rendered".into()))?;
            save_png(&self.path_for(index), physical, &pixels)
                .map_err(slint::PlatformError::Other)?;
        }
        Ok(())
    }
}

fn render(window: &MinimalSoftwareWindow) -> Option<Vec<Rgb8Pixel>> {
    let size = window.window().size();
    let (width, height) = (size.width.max(1) as usize, size.height.max(1) as usize);
    let mut buffer = vec![Rgb8Pixel { r: 0, g: 0, b: 0 }; width * height];
    window.request_redraw();
    let drawn = window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, width);
    });
    drawn.then_some(buffer)
}

/// Writes a binary PPM next to the PNG path and converts it with any
/// available encoder; PPM keeps this crate free of an extra image codec.
fn save_png(
    path: &std::path::Path,
    size: PhysicalSize,
    pixels: &[Rgb8Pixel],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let ppm = path.with_extension("ppm");
    let mut data = format!("P6\n{} {}\n255\n", size.width, size.height).into_bytes();
    data.reserve(pixels.len() * 3);
    for pixel in pixels {
        data.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
    }
    std::fs::write(&ppm, data).map_err(|error| format!("write {}: {error}", ppm.display()))?;
    println!("rendered {}", ppm.display());
    Ok(())
}

fn replay_input(window: &MinimalSoftwareWindow, script: &str) {
    use slint::platform::{Key, PointerEventButton};
    use slint::LogicalPosition;
    let point = |value: &str| -> Option<LogicalPosition> {
        let (x, y) = value.split_once(',')?;
        Some(LogicalPosition::new(
            x.trim().parse().ok()?,
            y.trim().parse().ok()?,
        ))
    };
    for step in script
        .split(';')
        .map(str::trim)
        .filter(|step| !step.is_empty())
    {
        if step == "tab" {
            let text: slint::SharedString = Key::Tab.into();
            window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            window.dispatch_event(WindowEvent::KeyReleased { text });
        } else if let Some(position) = step.strip_prefix("hover:").and_then(point) {
            window.dispatch_event(WindowEvent::PointerMoved { position });
        } else if let Some(position) = step.strip_prefix("press:").and_then(point) {
            window.dispatch_event(WindowEvent::PointerMoved { position });
            window.dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
        } else if let Some(position) = step.strip_prefix("release:").and_then(point) {
            window.dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        }
        slint::platform::update_timers_and_animations();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{native_window, SettingsWindow, Tokens, WorkbenchWindow};
    use slint::ComponentHandle;

    #[test]
    fn workbench_dialog_inherits_changes_to_reduced_motion() {
        slint::platform::set_platform(Box::new(OffscreenPlatform {
            windows: RefCell::new(Vec::new()),
            output: PathBuf::new(),
            size: None,
            scale: 1.0,
        }))
        .unwrap();
        let workbench = WorkbenchWindow::new().unwrap();
        let dialog = SettingsWindow::new().unwrap();
        for reduce_motion in [true, false] {
            native_window::apply_appearance(&workbench, false, reduce_motion);
            native_window::inherit_appearance(&dialog, &workbench);
            assert_eq!(dialog.global::<Tokens>().get_reduce_motion(), reduce_motion);
        }
    }
}
