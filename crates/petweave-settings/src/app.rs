//! Settings GUI state and eframe entry (`docs/GUI_ROADMAP.md` G2).
//!
//! The app is deliberately thin: [`Store`] owns persistence, [`Client`] owns
//! the control socket, and the pages live in [`crate::ui`]. State that the UI
//! edits is a plain [`Config`] draft; "dirty" is `draft != store.baseline`
//! (structural comparison, §7 "脏状态"), so there are no per-field dirty
//! flags to get out of sync.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use petweave::package::InstalledPkg;
use petweave::platform::control;
use petweave_core::config::{BongoConfig, Config, PetInstance, RenderConfig, MAX_PETS};
use petweave_core::placement::Rect;

use crate::canvas::{self, CanvasPet, CanvasState, OutputPage};
use crate::client::Client;
use crate::i18n::Tr;
use crate::preview::{PreviewCache, PREVIEW_INTERVAL};
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Pets,
    Params,
    Layout,
    Doctor,
    About,
}

impl Page {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pets => Tr::SIDEBAR_PETS,
            Self::Params => Tr::SIDEBAR_PARAMS,
            Self::Layout => Tr::SIDEBAR_LAYOUT,
            Self::Doctor => Tr::SIDEBAR_DOCTOR,
            Self::About => Tr::SIDEBAR_ABOUT,
        }
    }

    /// Pages that show pet previews (F30) and therefore drive the animation
    /// clock.
    pub fn shows_previews(self) -> bool {
        matches!(self, Self::Pets | Self::Params | Self::Layout)
    }
}

/// GUI state. Tests construct it directly (no window/compositor needed).
pub struct SettingsApp {
    pub store: Store,
    /// Config being edited; equals `store.baseline` after a successful save.
    pub draft: Config,
    pub selected: Option<usize>,
    pub page: Page,
    pub client: Client,
    pub installed: Vec<InstalledPkg>,
    /// Last successful action ("已保存…"), shown in green.
    pub status: Option<String>,
    /// Last failure shown in the banner (never modal; F14 wants inline).
    pub error: Option<String>,
    /// CJK font family installed at startup, or why it failed.
    pub font_note: String,
    /// The close-confirmation modal is open.
    pub close_confirm: bool,
    /// Set when the user chose "save and close" (or had no changes).
    pub allow_close: bool,
    /// A save happened while the control socket was not connected; send
    /// `Reload` as soon as the handshake completes (inotify is a fallback).
    pub pending_reload: bool,
    /// Preview runtime + texture cache (F30). Rebuilt only when the rendered
    /// inputs change; ticking it is what animates thumbnails.
    pub preview: PreviewCache,
    /// Layout canvas state (selected output page, snapping, in-flight drag).
    pub canvas: CanvasState,
    /// When the layout page last asked the host for `ListOutputs`/`ListPets`
    /// (monitor hotplug + pets dragged on the desktop, F21).
    pub last_control_refresh: Instant,
}

impl SettingsApp {
    pub fn new(config_path: PathBuf, socket: Option<PathBuf>) -> Result<Self, String> {
        let socket_path = socket.or_else(control::socket_path).unwrap_or_default();
        Self::new_at(config_path, socket_path)
    }

    /// Like [`SettingsApp::new`] with an explicit control socket path; an
    /// empty path disables the control client (tests, pure config editing).
    pub fn new_at(config_path: PathBuf, socket_path: PathBuf) -> Result<Self, String> {
        let store = Store::load(config_path)?;
        let draft = store.baseline.clone();
        let error = store.load_error.clone();
        let mut client = Client::new(socket_path);
        if !client.path().as_os_str().is_empty() {
            // Failure is normal: the host may not be running (F33).
            let _ = client.connect();
        }
        let installed = petweave::package::list();
        let selected = (!draft.pets.is_empty()).then_some(0);
        Ok(Self {
            store,
            draft,
            selected,
            page: Page::Pets,
            client,
            installed,
            status: None,
            error,
            font_note: String::new(),
            close_confirm: false,
            allow_close: false,
            pending_reload: false,
            preview: PreviewCache::new(),
            canvas: CanvasState::default(),
            last_control_refresh: Instant::now(),
        })
    }

    // --- derived state -----------------------------------------------------

    pub fn dirty(&self) -> bool {
        self.store.is_dirty(&self.draft)
    }

    pub fn validation_error(&self) -> Option<String> {
        self.draft.validate().err().map(|e| e.to_string())
    }

    pub fn render_defaults(&self) -> RenderConfig {
        self.draft.render.clone()
    }

    pub fn selected_pet(&self) -> Option<&PetInstance> {
        self.selected.and_then(|index| self.draft.pets.get(index))
    }

    pub fn selected_pet_mut(&mut self) -> Option<&mut PetInstance> {
        self.selected
            .and_then(|index| self.draft.pets.get_mut(index))
    }

    pub fn output_names(&self) -> Vec<String> {
        let configured = self
            .selected_pet()
            .and_then(|p| p.output.clone())
            .unwrap_or_default();
        self.client.output_names(&configured)
    }

    pub fn packages_of_kind(&self, kind: &str) -> Vec<&InstalledPkg> {
        self.installed
            .iter()
            .filter(|pkg| pkg.kind == kind)
            .collect()
    }

    // --- layout / preview (G3) --------------------------------------------

    /// Canvas pages: one per `ListOutputs` entry, else a synthetic stand-in so
    /// the layout page keeps working with the host offline (F33).
    pub fn output_pages(&self) -> Vec<OutputPage> {
        canvas::output_pages(&self.client.outputs)
    }

    /// Surface size to draw for `index`: the loaded pet's own size when the
    /// preview runtime has it (script-owned sizes included), else the config
    /// precedence without the pet's preferred size (disabled/offline pets).
    pub fn pet_surface_size(&self, index: usize) -> (u32, u32) {
        let Some(pet) = self.draft.pets.get(index) else {
            return (self.draft.render.width, self.draft.render.height);
        };
        let id = pet.resolved_id(index);
        self.preview.surface_size(&id).unwrap_or_else(|| {
            let render = &self.draft.render;
            (
                pet.width.unwrap_or(render.width),
                pet.height.unwrap_or(render.height),
            )
        })
    }

    /// Which page a pet is drawn on: its `output`, else the first page.
    pub fn pet_page_name(&self, index: usize, pages: &[OutputPage]) -> String {
        let configured = self
            .draft
            .pets
            .get(index)
            .and_then(|pet| pet.output.clone())
            .unwrap_or_default();
        canvas::page_for_pet(pages, &configured).name.clone()
    }

    /// Every pet that belongs on page `page_index`, in config order
    /// (= stacking order).
    ///
    /// The rectangle comes from the running host when it reports one
    /// (`PetInfo.rect`, which also reflects dragging on the desktop), and from
    /// the placement mirror + preview size otherwise.
    pub fn canvas_pets(&self, pages: &[OutputPage], page_index: usize) -> Vec<CanvasPet> {
        let Some(page) = pages.get(page_index) else {
            return Vec::new();
        };
        let render = &self.draft.render;
        let mut pets = Vec::new();
        for (index, pet) in self.draft.pets.iter().enumerate() {
            if self.pet_page_name(index, pages) != page.name {
                continue;
            }
            let id = pet.resolved_id(index);
            let size = self.pet_surface_size(index);
            let (rect, live) = match self.client.pets.iter().find(|info| info.id == id) {
                Some(info) => (
                    Rect {
                        x: info.rect[0] as f64,
                        y: info.rect[1] as f64,
                        width: info.rect[2].max(1) as f64,
                        height: info.rect[3].max(1) as f64,
                    },
                    true,
                ),
                None => (canvas::pet_rect(pet, render, size, page.rect), false),
            };
            pets.push(CanvasPet {
                index,
                id,
                name: pet.display_name(index),
                rect,
                size,
                enabled: pet.enabled,
                live,
            });
        }
        pets
    }

    /// Write an absolute position for one pet (F22) and return the placement
    /// in control-channel shape so it can be pushed live.
    pub fn set_pet_position(
        &mut self,
        index: usize,
        x: f64,
        y: f64,
        page: &OutputPage,
    ) -> Option<(String, [i32; 4])> {
        let size = self.pet_surface_size(index);
        let render = self.draft.render.clone();
        let pet = self.draft.pets.get_mut(index)?;
        Some(canvas::apply_position(pet, &render, size, x, y, page.rect))
    }

    /// Push a placement to the running host (no-op when it is offline). The
    /// canvas throttles these; the config file is still written on save (D10).
    pub fn push_placement(&mut self, id: &str, placement: &(String, [i32; 4])) -> bool {
        if !self.canvas.should_send(id, placement) {
            return false;
        }
        self.client
            .set_placement(id, &placement.0, placement.1)
    }

    /// F24 preset: bottom centre, then push it live.
    pub fn preset_bottom_center(&mut self, index: usize) {
        let render = self.draft.render.clone();
        if let Some(pet) = self.draft.pets.get_mut(index) {
            canvas::apply_bottom_center(pet, &render);
        }
        self.resend_placement(index);
    }

    /// F24 preset: drop all placement overrides (back to `[render]`).
    pub fn preset_clear_placement(&mut self, index: usize) {
        if let Some(pet) = self.draft.pets.get_mut(index) {
            canvas::clear_placement(pet);
        }
        self.resend_placement(index);
    }

    /// Recompute a pet's placement from the draft and push it to the host.
    ///
    /// Used after edits that do not go through a drag (anchor checkboxes,
    /// margin fields, presets) so the desktop follows the form immediately.
    pub fn resend_placement(&mut self, index: usize) {
        let render = self.draft.render.clone();
        let Some(pet) = self.draft.pets.get(index) else {
            return;
        };
        let (anchor, margins) = canvas::resolved_placement(pet, &render);
        let placement = (
            canvas::anchor_string(anchor),
            [margins.0, margins.1, margins.2, margins.3],
        );
        let id = pet.resolved_id(index);
        self.push_placement(&id, &placement);
    }

    /// Ask the host for fresh pets/outputs (monitor hotplug, F21).
    pub fn refresh_control(&mut self) {
        self.last_control_refresh = Instant::now();
        if !self.client.can_control() {
            return;
        }
        self.client.list_pets();
        self.client.list_outputs();
    }

    /// Stable `{kind}-{n}` id that is not used by the draft yet (D11).
    pub fn unique_id(&self, kind: &str) -> String {
        let used: Vec<String> = self
            .draft
            .resolve_pets()
            .into_iter()
            .map(|pet| pet.id)
            .collect();
        let base = if kind.trim().is_empty() {
            "pet"
        } else {
            kind.trim()
        };
        for n in 1..=(MAX_PETS * 4) {
            let candidate = format!("{base}-{n}");
            if !used.iter().any(|id| id == &candidate) {
                return candidate;
            }
        }
        format!("{base}-{}", used.len() + 1)
    }

    // --- actions -----------------------------------------------------------

    pub fn save(&mut self) {
        match self.store.save(&self.draft) {
            Ok(report) => {
                self.draft = self.store.baseline.clone();
                self.error = None;
                let backup = report
                    .backup
                    .as_ref()
                    .map(|p| format!("（备份：{}）", p.display()))
                    .unwrap_or_default();
                // Do not wait for the 300ms inotify debounce (F31). When the
                // host is offline, say so instead of pretending it applied.
                let notified = self.client.reload();
                self.pending_reload = !notified;
                let outcome = if notified {
                    Tr::SAVED
                } else {
                    "已保存（宿主未连接：启动后或走热重载生效）"
                };
                self.status = Some(format!("{outcome}：{}{}", report.path.display(), backup));
            }
            Err(e) => {
                self.error = Some(e.to_string());
                self.status = None;
            }
        }
    }

    pub fn reload_from_disk(&mut self) {
        match self.store.reload() {
            Ok(()) => {
                self.draft = self.store.baseline.clone();
                self.selected = (!self.draft.pets.is_empty()).then_some(0);
                self.error = self.store.load_error.clone();
                self.status = Some(format!("已重新读取 {}", self.store.path.display()));
                self.client.reload();
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn add_pet(&mut self, kind: &str, package: &str) {
        if self.draft.pets.len() >= MAX_PETS {
            self.error = Some(format!("宠物数量上限为 {MAX_PETS} 只"));
            return;
        }
        let id = self.unique_id(kind);
        let default = PetInstance::default();
        let pet = PetInstance {
            id: Some(id),
            kind: kind.to_string(),
            package: package.to_string(),
            name: default_name(kind, package),
            color: default.color,
            ..default
        };
        self.draft.pets.push(pet);
        self.selected = Some(self.draft.pets.len() - 1);
        self.page = Page::Params;
        self.status = Some(format!("已添加 {}", default_name(kind, package)));
    }

    pub fn copy_selected(&mut self) {
        let Some(index) = self.selected else { return };
        let Some(source) = self.draft.pets.get(index).cloned() else {
            return;
        };
        if self.draft.pets.len() >= MAX_PETS {
            self.error = Some(format!("宠物数量上限为 {MAX_PETS} 只"));
            return;
        }
        let mut copy = source;
        copy.id = Some(self.unique_id(&copy.kind));
        copy.name = format!("{} 副本", copy.name);
        self.draft.pets.insert(index + 1, copy);
        self.selected = Some(index + 1);
        self.page = Page::Params;
    }

    pub fn delete_selected(&mut self) {
        let Some(index) = self.selected else { return };
        if index >= self.draft.pets.len() {
            return;
        }
        self.draft.pets.remove(index);
        self.selected = if self.draft.pets.is_empty() {
            None
        } else {
            Some(index.min(self.draft.pets.len() - 1))
        };
    }

    pub fn move_selected(&mut self, delta: isize) {
        let Some(index) = self.selected else { return };
        let target = index as isize + delta;
        if target < 0 || target as usize >= self.draft.pets.len() {
            return;
        }
        self.draft.pets.swap(index, target as usize);
        self.selected = Some(target as usize);
    }

    /// Called by the params page when the kind changes: keep the package only
    /// when it belongs to the new kind.
    pub fn on_kind_changed(&mut self, before: &str) {
        let Some(kind) = self.selected_pet().map(|pet| pet.kind.clone()) else {
            return;
        };
        // Snapshot the fallback package before borrowing the pet mutably.
        let fallback = (matches!(kind.as_str(), "sprite" | "lua"))
            .then(|| {
                self.installed
                    .iter()
                    .find(|pkg| pkg.kind == kind)
                    .map(|pkg| pkg.name.clone())
            })
            .flatten();
        let Some(pet) = self.selected_pet_mut() else {
            return;
        };
        if pet.kind != before && !matches!(pet.kind.as_str(), "sprite" | "lua") {
            pet.package.clear();
        }
        if matches!(pet.kind.as_str(), "sprite" | "lua") && pet.package.is_empty() {
            if let Some(package) = fallback {
                pet.package = package;
            }
        }
    }

    // --- eframe integration ------------------------------------------------

    pub fn update_frame(&mut self, ctx: &egui::Context) {
        self.client.tick();
        if self.pending_reload && self.client.can_control() && self.client.reload() {
            self.pending_reload = false;
        }
        // Poll while connecting/connected so events show up promptly; stay
        // asleep otherwise (no busy repaint loop).
        if matches!(
            self.client.state(),
            crate::client::ConnectionState::Connecting | crate::client::ConnectionState::Connected
        ) {
            ctx.request_repaint_after(Duration::from_millis(300));
        } else {
            ctx.request_repaint_after(Duration::from_secs(2));
        }

        // Previews (F30) only run while a page that shows them is visible, at
        // ~10 fps; the About page costs nothing.
        if self.page.shows_previews() {
            self.preview.sync(&self.draft);
            if self.preview.tick() {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(PREVIEW_INTERVAL);
            }
        }

        // Monitor hotplug and pets dragged on the desktop (F21): the layout
        // page refreshes its snapshot twice a second instead of trusting a
        // stale `ListOutputs` answer from handshake time.
        if self.page == Page::Layout
            && self.client.can_control()
            && self.last_control_refresh.elapsed() >= Duration::from_millis(2000)
        {
            self.refresh_control();
        }

        // Close confirmation when there are unsaved edits.
        if ctx.input(|input| input.viewport().close_requested())
            && self.dirty()
            && !self.allow_close
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_confirm = true;
        }
        if self.close_confirm {
            let mut close = false;
            egui::Window::new(Tr::UNSAVED)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(Tr::CLOSE_CONFIRM);
                    ui.horizontal(|ui| {
                        if ui.button(Tr::SAVE_AND_CLOSE).clicked() {
                            self.save();
                            if self.error.is_none() {
                                close = true;
                            }
                        }
                        if ui.button(Tr::DISCARD).clicked() {
                            close = true;
                        }
                        if ui.button(Tr::CANCEL).clicked() {
                            self.close_confirm = false;
                        }
                    });
                });
            if close {
                self.close_confirm = false;
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// Top bar: title, host status, dirty state and the save/reload actions.
    pub fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(Tr::APP_TITLE);
            ui.separator();
            let (color, text) = match self.client.state() {
                crate::client::ConnectionState::Connected => (
                    egui::Color32::from_rgb(46, 160, 67),
                    self.client.state_label(),
                ),
                crate::client::ConnectionState::VersionMismatch(_) => {
                    (egui::Color32::RED, self.client.state_label())
                }
                _ => (egui::Color32::GRAY, self.client.state_label()),
            };
            ui.colored_label(color, text);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let error = self.validation_error();
                let can_save = self.dirty() && error.is_none();
                let disabled_hint = match (&error, self.dirty()) {
                    (Some(error), _) => error.clone(),
                    (None, false) => "没有未保存的修改".to_string(),
                    (None, true) => Tr::UNSAVED.to_string(),
                };
                let response = ui
                    .add_enabled(can_save, egui::Button::new(Tr::SAVE))
                    .on_disabled_hover_text(disabled_hint);
                if response.clicked() {
                    self.save();
                }
                if ui.button(Tr::RELOAD).clicked() {
                    self.reload_from_disk();
                }
                if self.dirty() {
                    ui.colored_label(egui::Color32::from_rgb(200, 130, 0), Tr::UNSAVED);
                }
            });
        });
        ui.separator();
    }

    /// Sidebar navigation.
    pub fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        for page in [Page::Pets, Page::Params, Page::Layout, Page::Doctor, Page::About] {
            let selected = self.page == page;
            if ui.selectable_label(selected, page.label()).clicked() {
                self.page = page;
            }
        }
        ui.separator();
        if self.dirty() {
            ui.colored_label(egui::Color32::from_rgb(200, 130, 0), Tr::UNSAVED);
        }
        ui.add_space(4.0);
        if let Some(status) = &self.status {
            ui.label(
                egui::RichText::new(status)
                    .small()
                    .color(egui::Color32::from_rgb(46, 160, 67)),
            );
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        if !self.client.can_control()
            && self.client.state() != &crate::client::ConnectionState::Connected
        {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(Tr::HOST_OFFLINE).small());
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            if !self.font_note.is_empty() {
                ui.label(egui::RichText::new(&self.font_note).small().weak());
            }
        });
    }

    /// Central page + banners (also used by the headless UI test).
    pub fn central(&mut self, ui: &mut egui::Ui) {
        if self.store.migrated_legacy {
            ui.colored_label(egui::Color32::from_rgb(30, 120, 200), Tr::MIGRATED);
        }
        if !self.store.warnings.is_empty() {
            ui.collapsing(Tr::UNKNOWN_KEYS, |ui| {
                for warning in &self.store.warnings {
                    ui.label(warning);
                }
            });
        }
        if let Some(error) = &self.error {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("{}: {error}", Tr::INVALID_CONFIG),
            );
        }
        ui.add_space(4.0);
        match self.page {
            Page::Pets => crate::ui::pets::show(self, ui),
            Page::Params => crate::ui::params::show(self, ui),
            Page::Layout => crate::ui::layout::show(self, ui),
            Page::Doctor => crate::ui::doctor::show(self, ui),
            Page::About => crate::ui::about::show(self, ui),
        }
    }

    /// Combined frame for the `egui_kittest` harness: the same per-frame work
    /// the real app does ([`Self::update_frame`]) plus top bar and page, so
    /// tests exercise preview syncing and control polling too.
    pub fn ui_test(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.update_frame(&ctx);
        self.top_bar(ui);
        self.central(ui);
    }

    pub fn selected_name(&self) -> String {
        self.selected_pet()
            .map(|pet| pet.name.clone())
            .unwrap_or_else(|| "-".to_string())
    }
}

fn default_name(kind: &str, package: &str) -> String {
    match kind {
        "bongo" => "BongoCat".to_string(),
        "demo" => "Demo".to_string(),
        _ if !package.is_empty() => package.to_string(),
        _ => "Pet".to_string(),
    }
}

/// Defaults used by the params page "恢复默认" buttons.
pub fn bongo_defaults() -> BongoConfig {
    BongoConfig::default()
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // egui 0.37 hands the app a root `Ui`; panels are created inside it.
        let ctx = ui.ctx().clone();
        self.update_frame(&ctx);
        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("nav")
            .resizable(false)
            .default_size(170.0)
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default().show(ui, |ui| self.central(ui));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_app(text: &str) -> (tempfile::TempDir, SettingsApp) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("petweave.toml");
        std::fs::write(&path, text).unwrap();
        let app = SettingsApp::new_at(path, PathBuf::new()).expect("app");
        (dir, app)
    }

    const ONE_PET: &str = r##"# keep me
[[pets]]
id = "cat"
name = "Cat"
kind = "bongo"
"##;

    #[test]
    fn add_pet_uses_unique_ids_and_selection() {
        let (_dir, mut app) = temp_app(ONE_PET);
        app.add_pet("demo", "");
        assert_eq!(app.draft.pets.last().unwrap().id.as_deref(), Some("demo-1"));
        app.add_pet("demo", "");
        assert_eq!(app.draft.pets.last().unwrap().id.as_deref(), Some("demo-2"));
        app.add_pet("bongo", "");
        assert_eq!(
            app.draft.pets.last().unwrap().id.as_deref(),
            Some("bongo-1")
        );
        assert_eq!(app.selected, Some(3));
        assert_eq!(app.page, Page::Params);
        assert!(app.dirty());
    }

    #[test]
    fn copy_selected_clears_id_and_inserts_after() {
        let (_dir, mut app) = temp_app(ONE_PET);
        app.selected = Some(0);
        app.copy_selected();
        let copy = &app.draft.pets[1];
        assert_eq!(copy.id.as_deref(), Some("bongo-1"));
        assert!(copy.name.contains("副本"));
        assert_eq!(app.selected, Some(1));
    }

    #[test]
    fn delete_and_move_keep_selection_valid() {
        let (_dir, mut app) = temp_app(
            r#"[[pets]]
id = "a"
name = "A"
kind = "demo"
[[pets]]
id = "b"
name = "B"
kind = "demo"
[[pets]]
id = "c"
name = "C"
kind = "demo"
"#,
        );
        app.selected = Some(1);
        app.move_selected(1);
        let ids: Vec<_> = app
            .draft
            .pets
            .iter()
            .map(|p| p.id.clone().unwrap())
            .collect();
        assert_eq!(ids, ["a", "c", "b"]);
        assert_eq!(app.selected, Some(2));

        app.delete_selected();
        assert_eq!(app.draft.pets.len(), 2);
        assert_eq!(app.selected, Some(1));

        app.delete_selected();
        app.delete_selected();
        assert!(app.draft.pets.is_empty());
        assert_eq!(app.selected, None);
        // Adding after deleting everything still works.
        app.add_pet("demo", "");
        assert_eq!(app.selected, Some(0));
    }

    #[test]
    fn dirty_tracks_structural_changes_and_save_clears_it() {
        let (_dir, mut app) = temp_app(ONE_PET);
        assert!(!app.dirty());
        app.draft.pets[0].name = "Renamed".into();
        assert!(app.dirty());
        app.save();
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(!app.dirty());
        assert_eq!(app.store.baseline.pets[0].name, "Renamed");
    }

    #[test]
    fn invalid_draft_is_reported_and_not_saved() {
        let (_dir, mut app) = temp_app(ONE_PET);
        app.draft.pets[0].name.clear(); // validation: name must not be empty
        assert!(app.validation_error().is_some());
        app.save();
        assert!(
            app.error.as_deref().unwrap_or("").contains("校验失败"),
            "{:?}",
            app.error
        );
    }

    #[test]
    fn legacy_config_is_flagged_for_migration() {
        let (_dir, app) = temp_app(
            r#"[pet]
name = "Old"
kind = "demo"
"#,
        );
        assert!(app.store.migrated_legacy);
        assert_eq!(app.draft.pets[0].id.as_deref(), Some("default"));
        assert!(!app.dirty());
    }
}
