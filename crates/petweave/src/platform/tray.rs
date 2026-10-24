//! StatusNotifierItem system tray via ksni (blocking API).
//!
//! The tray shows the first pet as its icon. Left click toggles the
//! visibility of every pet; the menu offers show/hide-all, one submenu per
//! pet (show/hide + its movement mode), the global movement-mode radio group
//! (drag / physics / fixed), the render layer (top / overlay — overlay keeps
//! the pet above fullscreen windows) and quit. Labels and state are read from
//! shared state at menu-open time, so they are always current.
//!
//! Spawning uses `assume_sni_available(true)`: if no StatusNotifierWatcher
//! exists yet (e.g. niri without a tray daemon), the service keeps waiting
//! instead of failing the host.

use std::sync::{Arc, Mutex};

use petweave_core::config::MoveMode;
use petweave_core::render::Frame;

use crate::app::HostCommand;
use crate::graphics::scale_frame;

/// One pet as the tray sees it (menu entries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayPet {
    pub id: String,
    pub name: String,
    pub visible: bool,
    pub mode: MoveMode,
}

/// Layers the tray lets the user pick (Q: keep pets above fullscreen).
pub const TRAY_LAYERS: &[(&str, &str)] = &[
    ("top", "Top（常规，全屏时隐藏）"),
    ("overlay", "Overlay（全屏时仍显示）"),
];

/// State shared between the host and the tray thread.
#[derive(Debug, Default)]
pub struct TrayShared {
    /// Whether any pet is currently visible (drives the show/hide label).
    pub visible: bool,
    /// Current movement mode of the first pet (global radio selection).
    pub mode: MoveMode,
    /// Current layer-shell layer of the first pet ("" or unknown = top).
    pub layer: String,
    /// Tray title/tooltip (pet name or "N 只宠物").
    pub title: String,
    /// Running pets, in config order.
    pub pets: Vec<TrayPet>,
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
        // Prefer the live title so pet renames/reloads show up without
        // re-registering the tray.
        let live = self.shared.lock().ok().map(|s| s.title.clone()).unwrap_or_default();
        if live.is_empty() {
            self.title.clone()
        } else {
            live
        }
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
        use ksni::menu::{MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
        let (visible, mode, layer, pets) = {
            let shared = self.shared.lock().unwrap();
            (shared.visible, shared.mode, shared.layer.clone(), shared.pets.clone())
        };
        let mut items: Vec<MenuItem<Self>> = vec![StandardItem {
            label: if visible {
                "隐藏全部宠物".into()
            } else {
                "显示全部宠物".into()
            },
            activate: Box::new(|this: &mut Self| {
                let _ = this.tx.send(HostCommand::ToggleVisible);
            }),
            ..Default::default()
        }
        .into()];
        if !pets.is_empty() {
            // One submenu per pet: show/hide + its own movement mode.
            let mut entries: Vec<MenuItem<Self>> = Vec::new();
            for (idx, pet) in pets.iter().enumerate() {
                let label = if pet.visible {
                    format!("隐藏 {}", pet.name)
                } else {
                    format!("显示 {}", pet.name)
                };
                let id = pet.id.clone();
                entries.push(
                    StandardItem {
                        label,
                        activate: Box::new(move |this: &mut Self| {
                            let _ = this.tx.send(HostCommand::TogglePetVisible(id.clone()));
                        }),
                        ..Default::default()
                    }
                    .into(),
                );
                let pet_mode = pet.mode;
                let pet_id = pet.id.clone();
                entries.push(
                    SubMenu {
                        label: format!("{}：{}", pet.name, pet_mode.label()),
                        submenu: vec![RadioGroup {
                            selected: MoveMode::ALL
                                .iter()
                                .position(|m| *m == pet_mode)
                                .unwrap_or(1),
                            select: Box::new(move |this: &mut Self, index: usize| {
                                if let Some(mode) = MoveMode::ALL.get(index) {
                                    let _ = this.tx.send(HostCommand::SetPetMoveMode {
                                        id: pet_id.clone(),
                                        mode: *mode,
                                    });
                                }
                            }),
                            options: MoveMode::ALL
                                .iter()
                                .map(|m| RadioItem {
                                    label: m.label().into(),
                                    ..Default::default()
                                })
                                .collect(),
                        }
                        .into()],
                        ..Default::default()
                    }
                    .into(),
                );
                if idx + 1 < pets.len() {
                    entries.push(MenuItem::Separator);
                }
            }
            if pets.len() > 1 {
                items.push(MenuItem::Separator);
                items.push(
                    SubMenu {
                        label: format!("宠物（{}）", pets.len()),
                        submenu: entries,
                        ..Default::default()
                    }
                    .into(),
                );
            } else {
                items.push(MenuItem::Separator);
                items.extend(entries);
            }
        }
        items.push(MenuItem::Separator);
        items.push(
            SubMenu {
                label: "移动模式（全部）".into(),
                submenu: vec![RadioGroup {
                    selected: MoveMode::ALL
                        .iter()
                        .position(|m| *m == mode)
                        .unwrap_or(1),
                    select: Box::new(|this: &mut Self, index: usize| {
                        if let Some(mode) = MoveMode::ALL.get(index) {
                            let _ = this.tx.send(HostCommand::SetMoveMode(*mode));
                        }
                    }),
                    options: MoveMode::ALL
                        .iter()
                        .map(|m| RadioItem {
                            label: m.label().into(),
                            ..Default::default()
                        })
                        .collect(),
                }
                .into()],
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        // Render layer: `overlay` stays above fullscreen windows (top is
        // hidden by the fullscreen auto-hide, the layer-shell default).
        let selected_layer = TRAY_LAYERS
            .iter()
            .position(|(id, _)| *id == layer)
            .unwrap_or(0);
        items.push(
            SubMenu {
                label: format!(
                    "渲染层级：{}",
                    TRAY_LAYERS[selected_layer].1.split('（').next().unwrap_or("Top")
                ),
                submenu: vec![RadioGroup {
                    selected: selected_layer,
                    select: Box::new(|this: &mut Self, index: usize| {
                        if let Some((id, _)) = TRAY_LAYERS.get(index) {
                            let _ = this.tx.send(HostCommand::SetLayer((*id).to_string()));
                        }
                    }),
                    options: TRAY_LAYERS
                        .iter()
                        .map(|(_, label)| RadioItem {
                            label: (*label).to_string(),
                            ..Default::default()
                        })
                        .collect(),
                }
                .into()],
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "打开设置…".into(),
                activate: Box::new(|_this: &mut Self| {
                    let mut candidates = Vec::new();
                    if let Ok(exe) = std::env::current_exe() {
                        if let Some(dir) = exe.parent() {
                            candidates.push(dir.join("petweave-settings"));
                        }
                    }
                    if let Some(path) = std::env::var_os("PATH") {
                        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("petweave-settings")));
                    }
                    if let Some(binary) = candidates.into_iter().find(|path| {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::metadata(path)
                            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                            .unwrap_or(false)
                    }) {
                        if let Err(error) = std::process::Command::new(binary).spawn() {
                            tracing::warn!(%error, "failed to launch petweave-settings from tray");
                        }
                    } else {
                        tracing::warn!("petweave-settings not found; build it with cargo build -p petweave-settings");
                    }
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "退出 PetWeave".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(HostCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );
        items
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
    fn title_follows_shared_state() {
        let shared = shared_with_pets(vec![pet("a", "A", true), pet("b", "B", true)]);
        let (tx, _rx) = calloop::channel::channel::<HostCommand>();
        let tray = PetTray::new(shared.clone(), tx, None, "stale".into());
        assert_eq!(tray.title(), "PetWeave", "live shared title wins");
        shared.lock().unwrap().title = "PetWeave · 3 只宠物".into();
        assert_eq!(tray.title(), "PetWeave · 3 只宠物");
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

    fn shared_with_pets(pets: Vec<TrayPet>) -> Arc<Mutex<TrayShared>> {
        Arc::new(Mutex::new(TrayShared {
            visible: true,
            mode: MoveMode::Physics,
            layer: "top".into(),
            title: "PetWeave".into(),
            pets,
        }))
    }

    fn pet(id: &str, name: &str, visible: bool) -> TrayPet {
        TrayPet {
            id: id.into(),
            name: name.into(),
            visible,
            mode: MoveMode::Physics,
        }
    }

    #[test]
    fn menu_labels_follow_visibility() {
        let shared = Arc::new(Mutex::new(TrayShared {
            visible: false,
            ..Default::default()
        }));
        let (tx, _rx) = calloop::channel::channel::<HostCommand>();
        let tray = PetTray::new(shared.clone(), tx, None, "PetWeave".into());
        let items = tray.menu();
        let ksni::menu::MenuItem::Standard(first) = &items[0] else {
            panic!("first item should be standard");
        };
        assert_eq!(first.label, "显示全部宠物");
    }

    #[test]
    fn menu_has_one_entry_per_pet() {
        let shared = shared_with_pets(vec![
            pet("bongo-main", "BongoCat", true),
            pet("blinky-1", "Blinky", false),
        ]);
        let (tx, rx) = calloop::channel::channel::<HostCommand>();
        let mut tray = PetTray::new(shared, tx, None, "PetWeave".into());
        let items = tray.menu();
        // [show/hide all, sep, 宠物(2) submenu, sep, mode, sep, quit]
        let ksni::menu::MenuItem::SubMenu(pets_menu) = &items[2] else {
            panic!("third item should be the per-pet submenu");
        };
        assert_eq!(pets_menu.label, "宠物（2）");
        let labels: Vec<String> = pets_menu
            .submenu
            .iter()
            .filter_map(|m| match m {
                ksni::menu::MenuItem::Standard(s) => Some(s.label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["隐藏 BongoCat", "显示 Blinky"]);

        // Toggling one pet sends the per-pet command with its id.
        let ksni::menu::MenuItem::Standard(first) = &pets_menu.submenu[0] else {
            panic!("first pet entry should be standard");
        };
        (first.activate)(&mut tray);
        match rx.recv() {
            Ok(HostCommand::TogglePetVisible(id)) => assert_eq!(id, "bongo-main"),
            other => panic!("expected TogglePetVisible, got {other:?}"),
        }
    }

    #[test]
    fn single_pet_menu_is_inlined() {
        let shared = shared_with_pets(vec![pet("only", "Only", true)]);
        let (tx, _rx) = calloop::channel::channel::<HostCommand>();
        let tray = PetTray::new(shared, tx, None, "PetWeave".into());
        let items = tray.menu();
        // [show/hide all, sep, pet toggle, pet mode submenu, sep, mode, sep, quit]
        let ksni::menu::MenuItem::Standard(toggle) = &items[2] else {
            panic!("third item should be the pet toggle");
        };
        assert_eq!(toggle.label, "隐藏 Only");
        let ksni::menu::MenuItem::SubMenu(mode) = &items[3] else {
            panic!("fourth item should be the pet mode submenu");
        };
        assert_eq!(mode.label, "Only：物理模式");
    }

    #[test]
    fn menu_lists_move_modes_with_selection() {
        let shared = Arc::new(Mutex::new(TrayShared {
            mode: MoveMode::Fixed,
            ..Default::default()
        }));
        let (tx, rx) = calloop::channel::channel::<HostCommand>();
        let mut tray = PetTray::new(shared, tx, None, "PetWeave".into());

        let items = tray.menu();
        // [show/hide, sep, mode submenu, sep, layer submenu, sep, settings, sep, quit]
        assert_eq!(items.len(), 9, "menu shape changed");
        let ksni::menu::MenuItem::SubMenu(sub) = &items[2] else {
            panic!("third item should be the mode submenu");
        };
        assert_eq!(sub.label, "移动模式（全部）");
        let ksni::menu::MenuItem::RadioGroup(group) = &sub.submenu[0] else {
            panic!("submenu should hold a radio group");
        };
        let labels: Vec<_> = group.options.iter().map(|o| o.label.clone()).collect();
        assert_eq!(labels, ["拖动模式", "物理模式", "固定模式"]);
        assert_eq!(group.selected, 2, "fixed mode preselected");

        // Selecting an option commands the host loop.
        (group.select)(&mut tray, 0);
        match rx.recv() {
            Ok(HostCommand::SetMoveMode(MoveMode::Drag)) => {}
            other => panic!("expected SetMoveMode(Drag), got {other:?}"),
        }
    }

    #[test]
    fn menu_lists_render_layers_with_selection() {
        let shared = Arc::new(Mutex::new(TrayShared {
            layer: "overlay".into(),
            ..Default::default()
        }));
        let (tx, rx) = calloop::channel::channel::<HostCommand>();
        let mut tray = PetTray::new(shared, tx, None, "PetWeave".into());

        let items = tray.menu();
        let ksni::menu::MenuItem::SubMenu(sub) = &items[4] else {
            panic!("fifth item should be the layer submenu");
        };
        assert!(sub.label.starts_with("渲染层级"), "{}", sub.label);
        let ksni::menu::MenuItem::RadioGroup(group) = &sub.submenu[0] else {
            panic!("submenu should hold a radio group");
        };
        let labels: Vec<_> = group.options.iter().map(|o| o.label.clone()).collect();
        assert_eq!(labels, ["Top（常规，全屏时隐藏）", "Overlay（全屏时仍显示）"]);
        assert_eq!(group.selected, 1, "overlay preselected from shared state");

        // Choosing Top sends the command with the layer id.
        (group.select)(&mut tray, 0);
        match rx.recv() {
            Ok(HostCommand::SetLayer(layer)) => assert_eq!(layer, "top"),
            other => panic!("expected SetLayer(top), got {other:?}"),
        }
        (group.select)(&mut tray, 1);
        match rx.recv() {
            Ok(HostCommand::SetLayer(layer)) => assert_eq!(layer, "overlay"),
            other => panic!("expected SetLayer(overlay), got {other:?}"),
        }
    }
}
