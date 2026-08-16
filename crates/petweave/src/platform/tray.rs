//! StatusNotifierItem system tray via ksni (blocking API).
//!
//! The tray shows the pet as its icon. Left click toggles pet visibility;
//! the menu offers show/hide and quit. Menu labels are read from shared
//! state at menu-open time, so they are always current.
//!
//! Spawning uses `assume_sni_available(true)`: if no StatusNotifierWatcher
//! exists yet (e.g. niri without a tray daemon), the service keeps waiting
//! instead of failing the host.

use std::sync::{Arc, Mutex};

use petweave_core::render::Frame;

use crate::app::HostCommand;
use crate::graphics::scale_frame;

/// State shared between the host and the tray thread.
#[derive(Debug, Default)]
pub struct TrayShared {
    /// Whether the pet is currently visible.
    pub visible: bool,
}

/// Tray icon target size (square).
pub const ICON_SIZE: u32 = 64;

pub struct PetTray {
    shared: Arc<Mutex<TrayShared>>,
    tx: calloop::channel::Sender<HostCommand>,
    icon: Vec<ksni::Icon>,
    title: String,
}

impl PetTray {
    /// Build a tray for the pet. `pet_frame` is rendered into the icon.
    pub fn new(
        shared: Arc<Mutex<TrayShared>>,
        tx: calloop::channel::Sender<HostCommand>,
        pet_frame: Option<&Frame>,
        title: String,
    ) -> Self {
        let icon = pet_frame.map(frame_to_icon).unwrap_or_default();
        Self {
            shared,
            tx,
            icon,
            title,
        }
    }
}

impl ksni::Tray for PetTray {
    fn id(&self) -> String {
        "petweave".into()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: String::new(),
            icon_pixmap: self.icon.clone(),
            title: self.title.clone(),
            description: "PetWeave desktop pet".into(),
        }
    }

    /// Left click toggles pet visibility.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(HostCommand::ToggleVisible);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        let visible = self.shared.lock().unwrap().visible;
        vec![
            StandardItem {
                label: if visible {
                    "隐藏宠物".into()
                } else {
                    "显示宠物".into()
                },
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(HostCommand::ToggleVisible);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "退出 PetWeave".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(HostCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Render a pet frame into a square ARGB32 tray icon (aspect-fit, centered).
fn frame_to_icon(frame: &Frame) -> Vec<ksni::Icon> {
    // Aspect-fit scale into ICON_SIZE canvas.
    let scale = (ICON_SIZE as f32 / frame.width as f32)
        .min(ICON_SIZE as f32 / frame.height as f32)
        .min(1.0);
    let w = (frame.width as f32 * scale).round().max(1.0) as u32;
    let h = (frame.height as f32 * scale).round().max(1.0) as u32;
    let scaled = if w == frame.width && h == frame.height {
        frame.clone()
    } else {
        scale_frame(frame, w, h)
    };

    let mut canvas = Frame::new(ICON_SIZE, ICON_SIZE);
    let x = (ICON_SIZE as i32 - w as i32) / 2;
    let y = (ICON_SIZE as i32 - h as i32) / 2;
    canvas.draw_image(x, y, &scaled);

    // RGBA -> ARGB32 (network byte order: A, R, G, B).
    let data: Vec<u8> = canvas
        .pixels
        .chunks_exact(4)
        .flat_map(|p| [p[3], p[0], p[1], p[2]])
        .collect();
    vec![ksni::Icon {
        width: ICON_SIZE as i32,
        height: ICON_SIZE as i32,
        data,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray;

    fn icon_data(frame: &Frame) -> Vec<u8> {
        let icons = frame_to_icon(frame);
        assert_eq!(icons.len(), 1);
        assert_eq!((icons[0].width, icons[0].height), (64, 64));
        icons[0].data.clone()
    }

    #[test]
    fn icon_converts_rgba_to_argb_and_pads() {
        let mut f = Frame::new(32, 32);
        f.fill([255, 0, 0, 255]); // opaque red
        let data = icon_data(&f);
        // No upscaling: the 32x32 sprite sits centered on the 64x64 canvas.
        assert_eq!(&data[0..4], &[0, 0, 0, 0], "canvas corner is padding");
        // Sprite top-left at (16,16): A=255 R=255 G=0 B=0.
        let tl = (16 * 64 + 16) * 4;
        assert_eq!(&data[tl..tl + 4], &[255, 255, 0, 0]);
        // Canvas center is inside the sprite, still red.
        let center = (32 * 64 + 32) * 4;
        assert_eq!(&data[center..center + 4], &[255, 255, 0, 0]);
    }

    #[test]
    fn icon_pads_wide_frames_with_transparency() {
        // 4:1 wide frame -> must be padded, not distorted.
        let mut f = Frame::new(64, 16);
        f.fill([0, 0, 255, 255]); // blue
        let data = icon_data(&f);
        // Top row of the 64x64 canvas is outside the aspect-fitted sprite.
        assert_eq!(&data[0..4], &[0, 0, 0, 0], "padding is transparent");
        // Vertical center should hit the sprite (scale 1.0, h=16, y=(64-16)/2=24).
        let row = 24 + 16 / 2;
        let px = (row * 64 + 32) * 4;
        assert_eq!(&data[px..px + 4], &[255, 0, 0, 255], "blue -> ARGB32");
    }

    #[test]
    fn menu_labels_follow_visibility() {
        let shared = Arc::new(Mutex::new(TrayShared { visible: false }));
        let (tx, _rx) = calloop::channel::channel::<HostCommand>();
        let tray = PetTray::new(shared.clone(), tx, None, "PetWeave".into());
        let items = tray.menu();
        let ksni::menu::MenuItem::Standard(first) = &items[0] else {
            panic!("first item should be standard");
        };
        assert_eq!(first.label, "显示宠物");
    }
}
