//! Config persistence for the settings GUI (`docs/GUI_ROADMAP.md` §5.5).
//!
//! The host is read-only (D10); this module is the only writer:
//!
//! 1. read the original text into a [`DocumentMut`] (`toml_edit`, comments and
//!    key order preserved),
//! 2. apply the GUI's [`Config`] onto the `[[pets]]` array (new/removed/
//!    reordered pets included; unchanged keys are not rewritten),
//! 3. parse + `Config::validate()` the resulting text (single source of truth,
//!    `AGENTS.md` §3) and refuse to write when it does not round-trip,
//! 4. write atomically: `.bak` copy, `tmp` file with `0600`, `fsync`, rename.
//!
//! Legacy `[pet]` configs are migrated **in memory** on load via
//! `Config::migrate_text` (comments preserved) and reported through
//! [`Store::migrated_legacy`], so the first save upgrades the file.
//!
//! External modifications are detected by comparing the file on disk with the
//! text seen at load/save time ([`SaveError::ExternalModified`]); the GUI then
//! offers a reload instead of silently overwriting someone else's edits.

use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use petweave_core::config::{
    unknown_keys, BongoConfig, Config, MoveMode, PetInstance, RenderConfig,
};
use petweave_core::placement::Anchor;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};
use serde_json::Value as JsonValue;

/// One saved config file.
pub struct Store {
    pub path: PathBuf,
    /// The `[[pets]]`/`[render]` document that is edited in place.
    doc: DocumentMut,
    /// Snapshot the GUI's draft is compared against (post-migration ids
    /// normalised). `draft != baseline` means "unsaved changes".
    pub baseline: Config,
    /// Unknown keys found on load (typos; shown as warnings).
    pub warnings: Vec<String>,
    /// A legacy `[pet]` config was upgraded in memory on load.
    pub migrated_legacy: bool,
    /// Validation error of the file as loaded (the GUI still opens so the
    /// user can fix it; saving is blocked until the draft validates).
    pub load_error: Option<String>,
    /// Exact text seen on disk at load/save (external-modification guard).
    last_seen: String,
}

#[derive(Debug, Clone)]
pub struct SaveReport {
    pub path: PathBuf,
    pub backup: Option<PathBuf>,
    pub bytes: usize,
}

#[derive(Debug)]
pub enum SaveError {
    /// `Config::validate` (or a parse round-trip) rejected the draft.
    Invalid(String),
    /// The file changed on disk since it was loaded/saved here.
    ExternalModified,
    /// Reading the old file, writing the backup or the atomic write failed.
    Io(String),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "配置校验失败：{message}"),
            Self::ExternalModified => write!(
                f,
                "配置文件已被其他程序修改；请先「重新加载」，避免覆盖别人的改动"
            ),
            Self::Io(message) => write!(f, "写入配置失败：{message}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl Store {
    /// Load `path` (missing file = pure defaults, created on first save).
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let (text, exists) = match std::fs::read_to_string(&path) {
            Ok(text) => (text, true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };

        let mut warnings = if exists {
            unknown_keys(&text)
        } else {
            Vec::new()
        };
        let mut config = if exists {
            Config::parse(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))?
        } else {
            Config::default()
        };

        // Legacy `[pet]` → `[[pets]]` in memory, keeping comments. This also
        // removes a stale `[pet]` when `[[pets]]` already exists.
        let mut doc: DocumentMut = if exists {
            text.parse()
                .map_err(|e| format!("cannot parse {} as TOML: {e}", path.display()))?
        } else {
            DocumentMut::new()
        };
        let mut migrated_legacy = false;
        if exists {
            let (migrated, changed) = Config::migrate_text(&text)
                .map_err(|e| format!("cannot migrate {}: {e}", path.display()))?;
            if changed {
                doc = migrated
                    .parse()
                    .map_err(|e| format!("cannot parse migrated config: {e}"))?;
                config = Config::parse(&migrated)
                    .map_err(|e| format!("cannot parse migrated config: {e}"))?;
                migrated_legacy = true;
                warnings = unknown_keys(&migrated);
            }
        }

        let mut baseline = config;
        normalize(&mut baseline);
        let load_error = baseline.validate().err().map(|e| e.to_string());

        Ok(Self {
            path,
            doc,
            baseline,
            warnings,
            migrated_legacy,
            load_error,
            last_seen: text,
        })
    }

    /// Resolve the standard config path (`$XDG_CONFIG_HOME/...`) for a GUI
    /// started without `--config`. Falls back to `./petweave.toml`.
    pub fn default_path() -> PathBuf {
        petweave_core::config::config_file_path().unwrap_or_else(|| PathBuf::from("petweave.toml"))
    }

    /// Whether `draft` differs from the loaded/saved snapshot.
    pub fn is_dirty(&self, draft: &Config) -> bool {
        let mut normalized = draft.clone();
        normalize(&mut normalized);
        normalized != self.baseline
    }

    /// Persist `draft`, preserving comments/unknown keys and formatting of
    /// unchanged entries. On success the store's baseline becomes `draft`.
    pub fn save(&mut self, draft: &Config) -> Result<SaveReport, SaveError> {
        // Refuse to overwrite edits made after this store loaded the file.
        let current = std::fs::read_to_string(&self.path).unwrap_or_default();
        if current != self.last_seen {
            return Err(SaveError::ExternalModified);
        }

        let mut draft = draft.clone();
        normalize(&mut draft);
        if let Err(e) = draft.validate() {
            return Err(SaveError::Invalid(e.to_string()));
        }

        // Apply the draft to a copy of the document, then verify the result
        // parses back to exactly the intended config before touching disk.
        let mut doc = self.doc.clone();
        sync_document(&mut doc, &self.baseline, &draft)
            .map_err(|e| SaveError::Invalid(format!("cannot sync [[pets]]: {e}")))?;
        let text = doc.to_string();
        let mut check = Config::parse(&text)
            .map_err(|e| SaveError::Invalid(format!("generated config does not parse: {e}")))?;
        normalize(&mut check);
        if let Err(e) = check.validate() {
            return Err(SaveError::Invalid(e.to_string()));
        }
        if check != draft {
            return Err(SaveError::Invalid(
                "内部错误：生成的配置与界面状态不一致，已拒绝写盘（请报告此问题）".to_string(),
            ));
        }

        let backup = write_atomic(&self.path, &text).map_err(SaveError::Io)?;
        self.doc = doc;
        self.baseline = draft;
        self.last_seen = text.clone();
        self.migrated_legacy = false;
        self.load_error = None;
        self.warnings.clear();
        Ok(SaveReport {
            path: self.path.clone(),
            backup,
            bytes: text.len(),
        })
    }

    /// Re-read the file, discarding in-memory edits (GUI "重新加载").
    pub fn reload(&mut self) -> Result<(), String> {
        let path = self.path.clone();
        *self = Self::load(path)?;
        Ok(())
    }
}

/// Canonicalise in-memory values the TOML form may spell differently
/// (`width = 0`, empty `anchor`/`output`, `mode` vs `move_mode` alias).
///
/// This keeps `draft != baseline` meaningful without rewriting untouched keys
/// in the document (the sync only writes values that actually changed).
pub fn normalize(config: &mut Config) {
    for (index, pet) in config.pets.iter_mut().enumerate() {
        pet.id = Some(pet.resolved_id(index));
        pet.width = pet.width.filter(|v| *v > 0);
        pet.height = pet.height.filter(|v| *v > 0);
        let clean = |value: &mut Option<String>| {
            if value.as_deref().is_some_and(|s| s.trim().is_empty()) {
                *value = None;
            }
        };
        clean(&mut pet.layer);
        clean(&mut pet.anchor);
        clean(&mut pet.output);
        clean(&mut pet.mode);
        if pet.mode.is_some() {
            pet.move_mode = None;
        }
        pet.move_mode = pet
            .move_mode
            .take()
            .filter(|m| MoveMode::parse(m).is_some() && !m.trim().is_empty());
    }
}

// --- document sync ---------------------------------------------------------

/// Global `[general]` values the GUI exposes (F12: frame rate). Unchanged
/// values are left exactly as written; the table is created only when needed.
fn sync_general(
    doc: &mut DocumentMut,
    before: &petweave_core::config::General,
    after: &petweave_core::config::General,
) {
    if before == after {
        return;
    }
    let Some(table) = doc
        .entry("general")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
    else {
        return;
    };
    if before.fps != after.fps {
        replace_item(table, "fps", toml_edit::value(after.fps as i64));
    }
    if before.log_level != after.log_level {
        set_string(table, "log_level", &after.log_level);
    }
    if before.sysinfo_interval_secs != after.sysinfo_interval_secs {
        replace_item(
            table,
            "sysinfo_interval_secs",
            toml_edit::value(after.sysinfo_interval_secs as i64),
        );
    }
    if before.tray_enabled != after.tray_enabled {
        replace_item(table, "tray_enabled", toml_edit::value(after.tray_enabled));
    }
    if table.is_empty() {
        doc.remove("general");
    }
}

/// Drop source positions recursively so the document writer emits tables in
/// traversal (i.e. array) order instead of their original file positions.
fn clear_positions(table: &mut Table) {
    table.set_position(None);
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(nested) => clear_positions(nested),
            Item::ArrayOfTables(array) => {
                for nested in array.iter_mut() {
                    clear_positions(nested);
                }
            }
            _ => {}
        }
    }
}

/// Rewrite the `[[pets]]` array from `draft`, keeping each existing pet table
/// (and therefore its comments) and moving it when the order changed.
fn sync_document(doc: &mut DocumentMut, baseline: &Config, draft: &Config) -> Result<(), String> {
    sync_general(doc, &baseline.general, &draft.general);
    let baseline_by_id: HashMap<String, usize> = baseline
        .resolve_pets()
        .into_iter()
        .map(|rp| (rp.id, rp.index))
        .collect();

    // Detach all old tables first so they can be re-ordered / dropped.
    let mut old_tables: Vec<Table> = Vec::new();
    if let Some(array) = doc.get("pets").and_then(Item::as_array_of_tables).cloned() {
        for table in array.iter() {
            old_tables.push(table.clone());
        }
    }
    let baseline_ids: Vec<String> = baseline
        .resolve_pets()
        .into_iter()
        .map(|rp| rp.id)
        .collect();
    let mut pool: HashMap<String, Table> = HashMap::new();
    for (index, table) in old_tables.into_iter().enumerate() {
        let id = baseline_ids
            .get(index)
            .cloned()
            .unwrap_or_else(|| format!("__old-{index}"));
        pool.insert(id, table);
    }

    let mut pets = ArrayOfTables::new();
    for (index, after) in draft.pets.iter().enumerate() {
        let id = after.id.clone().unwrap_or_else(|| after.resolved_id(index));
        let before = baseline_by_id
            .get(&id)
            .and_then(|index| baseline.pets.get(*index));
        let mut table = pool.remove(&id).unwrap_or_default();
        update_pet_table(&mut table, before, after)?;
        // A table extracted from the old array is no longer implicit; new
        // tables are explicit by construction.
        table.set_implicit(false);
        // `DocumentMut`'s writer orders tables by their original source
        // position. Moved/reordered pets must not keep theirs, otherwise the
        // file order would ignore the GUI's (and config's) stacking order.
        clear_positions(&mut table);
        pets.push(table);
    }

    doc.as_table_mut().insert("pets", Item::ArrayOfTables(pets));
    Ok(())
}

/// Apply one pet to its document table, writing only values that changed
/// relative to `before` (new pets compare against `PetInstance::default()`).
fn update_pet_table(
    table: &mut Table,
    before: Option<&PetInstance>,
    after: &PetInstance,
) -> Result<(), String> {
    let default = PetInstance::default();

    // Identity + plain fields.
    if after.id.is_some() {
        set_string(table, "id", after.id.as_deref().unwrap_or_default());
    }
    if before.is_none_or(|b| b.name != after.name) {
        set_string(table, "name", &after.name);
    }
    if before.is_none_or(|b| b.enabled != after.enabled) {
        replace_item(table, "enabled", toml_edit::value(after.enabled));
    }
    if before.is_none_or(|b| b.kind != after.kind) {
        set_string(table, "kind", &after.kind);
    }
    let package_changed = before.map_or(!after.package.is_empty(), |b| b.package != after.package);
    if package_changed {
        if after.package.is_empty() {
            table.remove("package");
        } else {
            set_string(table, "package", &after.package);
        }
    }
    let color_changed = before.map_or(after.color != default.color, |b| b.color != after.color);
    if color_changed {
        set_string(table, "color", &after.color);
    }

    // Placement group (inherits from `[render]` when absent).
    let b = |f: fn(&PetInstance) -> Option<u32>| before.and_then(f);
    sync_opt_u32(table, "width", b(|p| p.width), after.width);
    sync_opt_u32(table, "height", b(|p| p.height), after.height);
    let b = |f: fn(&PetInstance) -> Option<String>| before.and_then(f);
    sync_opt_string(
        table,
        "layer",
        b(|p| p.layer.clone()),
        after.layer.as_deref(),
    );
    sync_opt_string(
        table,
        "anchor",
        b(|p| p.anchor.clone()),
        after.anchor.as_deref(),
    );
    sync_opt_i32(
        table,
        "margin_top",
        before.and_then(|p| p.margin_top),
        after.margin_top,
    );
    sync_opt_i32(
        table,
        "margin_right",
        before.and_then(|p| p.margin_right),
        after.margin_right,
    );
    sync_opt_i32(
        table,
        "margin_bottom",
        before.and_then(|p| p.margin_bottom),
        after.margin_bottom,
    );
    sync_opt_i32(
        table,
        "margin_left",
        before.and_then(|p| p.margin_left),
        after.margin_left,
    );
    sync_opt_string(
        table,
        "output",
        b(|p| p.output.clone()),
        after.output.as_deref(),
    );
    sync_opt_bool(
        table,
        "disable_fullscreen_hide",
        before.and_then(|p| p.disable_fullscreen_hide),
        after.disable_fullscreen_hide,
    );
    sync_opt_bool(
        table,
        "click_through",
        before.and_then(|p| p.click_through),
        after.click_through,
    );
    // `mode` is the canonical key; drop the legacy alias when we write it.
    let mode_before = before.and_then(|p| p.mode.clone());
    if mode_before != after.mode {
        match after.mode.as_deref().filter(|m| !m.is_empty()) {
            Some(mode) => {
                set_string(table, "mode", mode);
                table.remove("move_mode");
            }
            None => {
                table.remove("mode");
            }
        }
    }

    sync_bongo(table, before.map(|p| &p.bongo), &after.bongo);
    sync_options(table, before.map(|p| &p.options), &after.options);
    Ok(())
}

fn sync_options(table: &mut Table, before: Option<&std::collections::BTreeMap<String, JsonValue>>, after: &std::collections::BTreeMap<String, JsonValue>) {
    if before.is_some_and(|old| old == after) || before.is_none() && after.is_empty() { return; }
    if after.is_empty() { table.remove("options"); return; }
    let mut options = Table::new();
    for (key, value) in after {
        if let Some(item) = json_item(value) { options.insert(key, item); }
    }
    table.insert("options", Item::Table(options));
}

fn json_item(value: &JsonValue) -> Option<Item> {
    match value {
        JsonValue::Null => None,
        JsonValue::Bool(v) => Some(toml_edit::value(*v)),
        JsonValue::String(v) => Some(toml_edit::value(v.clone())),
        JsonValue::Number(v) if v.as_i64().is_some() => Some(toml_edit::value(v.as_i64().unwrap())),
        JsonValue::Number(v) => Some(toml_edit::value(v.as_f64()?)),
        JsonValue::Array(values) => {
            let mut array = toml_edit::Array::new();
            for value in values { match json_item(value)? { Item::Value(v) => array.push(v), _ => return None } }
            Some(Item::Value(toml_edit::Value::Array(array)))
        }
        JsonValue::Object(_) => None,
    }
}

fn sync_bongo(table: &mut Table, before: Option<&BongoConfig>, after: &BongoConfig) {
    let default = BongoConfig::default();
    let differs = match before {
        Some(before) => before != after,
        // A new pet does not need a `[pets.bongo]` table for pure defaults.
        None => after != &default,
    };
    if !differs {
        return;
    }
    let Some(bongo) = table
        .entry("bongo")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
    else {
        return;
    };
    let b = |f: fn(&BongoConfig) -> u32| before.map(f);
    sync_bongo_u32(
        bongo,
        "cat_height",
        b(|c| c.cat_height),
        after.cat_height,
        default.cat_height,
    );
    let b = |f: fn(&BongoConfig) -> u64| before.map(f);
    sync_bongo_u64(
        bongo,
        "keypress_duration_ms",
        b(|c| c.keypress_duration_ms),
        after.keypress_duration_ms,
        default.keypress_duration_ms,
    );
    sync_bongo_u64(
        bongo,
        "idle_sleep_timeout_secs",
        b(|c| c.idle_sleep_timeout_secs),
        after.idle_sleep_timeout_secs,
        default.idle_sleep_timeout_secs,
    );
    sync_bongo_bool(
        bongo,
        "hand_mapping",
        before.map(|c| c.hand_mapping),
        after.hand_mapping,
        default.hand_mapping,
    );
    sync_bongo_bool(
        bongo,
        "mirror_x",
        before.map(|c| c.mirror_x),
        after.mirror_x,
        default.mirror_x,
    );
    sync_bongo_bool(
        bongo,
        "enable_scheduled_sleep",
        before.map(|c| c.enable_scheduled_sleep),
        after.enable_scheduled_sleep,
        default.enable_scheduled_sleep,
    );
    sync_bongo_string(
        bongo,
        "assets_dir",
        before.map(|c| c.assets_dir.clone()),
        &after.assets_dir,
        &default.assets_dir,
    );
    sync_bongo_string(
        bongo,
        "sleep_begin",
        before.map(|c| c.sleep_begin.clone()),
        &after.sleep_begin,
        &default.sleep_begin,
    );
    sync_bongo_string(
        bongo,
        "sleep_end",
        before.map(|c| c.sleep_end.clone()),
        &after.sleep_end,
        &default.sleep_end,
    );
    if bongo.is_empty() {
        table.remove("bongo");
    }
}

/// Replace an existing key's value **in place**, keeping the key's leading
/// comment/decor and the value's trailing comment. New keys are appended.
fn replace_item(table: &mut Table, key: &str, mut item: Item) {
    if let Some((_key, old)) = table.get_key_value_mut(key) {
        if let (Some(old_value), Some(new_value)) = (old.as_value(), item.as_value_mut()) {
            *new_value.decor_mut() = old_value.decor().clone();
        }
        *old = item;
    } else {
        table.insert(key, item);
    }
}

fn set_string(table: &mut Table, key: &str, value: &str) {
    replace_item(table, key, toml_edit::value(value));
}

fn sync_opt_u32(table: &mut Table, key: &str, before: Option<u32>, after: Option<u32>) {
    match after.filter(|v| *v > 0) {
        Some(value) => {
            if before != Some(value) {
                replace_item(table, key, toml_edit::value(value as i64));
            }
        }
        None => {
            if before.is_some() {
                table.remove(key);
            }
        }
    }
}

fn sync_opt_i32(table: &mut Table, key: &str, before: Option<i32>, after: Option<i32>) {
    match after {
        Some(value) => {
            if before != Some(value) {
                replace_item(table, key, toml_edit::value(value as i64));
            }
        }
        None => {
            if before.is_some() {
                table.remove(key);
            }
        }
    }
}

fn sync_opt_bool(table: &mut Table, key: &str, before: Option<bool>, after: Option<bool>) {
    match after {
        Some(value) => {
            if before != Some(value) {
                replace_item(table, key, toml_edit::value(value));
            }
        }
        None => {
            if before.is_some() {
                table.remove(key);
            }
        }
    }
}

fn sync_opt_string(table: &mut Table, key: &str, before: Option<String>, after: Option<&str>) {
    match after.filter(|v| !v.trim().is_empty()) {
        Some(value) => {
            if before.as_deref() != Some(value) {
                replace_item(table, key, toml_edit::value(value));
            }
        }
        None => {
            if before.is_some() {
                table.remove(key);
            }
        }
    }
}

fn sync_bongo_u32(table: &mut Table, key: &str, before: Option<u32>, after: u32, default: u32) {
    if before == Some(after) {
        return;
    }
    if after == default {
        table.remove(key);
    } else {
        replace_item(table, key, toml_edit::value(after as i64));
    }
}

fn sync_bongo_u64(table: &mut Table, key: &str, before: Option<u64>, after: u64, default: u64) {
    if before == Some(after) {
        return;
    }
    if after == default {
        table.remove(key);
    } else {
        replace_item(table, key, toml_edit::value(after as i64));
    }
}

fn sync_bongo_bool(table: &mut Table, key: &str, before: Option<bool>, after: bool, default: bool) {
    if before == Some(after) {
        return;
    }
    if after == default {
        table.remove(key);
    } else {
        replace_item(table, key, toml_edit::value(after));
    }
}

fn sync_bongo_string(
    table: &mut Table,
    key: &str,
    before: Option<String>,
    after: &str,
    default: &str,
) {
    if before.as_deref() == Some(after) {
        return;
    }
    if after == default {
        table.remove(key);
    } else {
        replace_item(table, key, toml_edit::value(after));
    }
}

// --- atomic write ----------------------------------------------------------

/// Write `text` to `path` atomically, keeping one `.bak` copy of the previous
/// file. Returns the backup path when one was made.
fn write_atomic(path: &Path, text: &str) -> Result<Option<PathBuf>, String> {
    use std::os::unix::fs::PermissionsExt;

    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        // Configuration may contain secrets in the future; keep it private.
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }

    let backup = if path.exists() {
        let backup = path.with_extension("toml.bak");
        std::fs::copy(path, &backup)
            .map_err(|e| format!("cannot write backup {}: {e}", backup.display()))?;
        let _ = std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o600));
        Some(backup)
    } else {
        None
    };

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("petweave.toml");
    let tmp = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot write {}: {e}", tmp.display()));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot replace {}: {e}", path.display()));
    }
    // Best effort: make the rename durable.
    if let Some(parent) = parent {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    // `rename` keeps the tmp's 0600, but be explicit for exotic umasks.
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    Ok(backup)
}

/// Validate an anchor string for the params form (F14 inline errors).
pub fn anchor_is_valid(value: &str) -> bool {
    !value.trim().is_empty() && Anchor::parse(value).is_ok()
}

/// Effective `[render]` defaults for "继承自 [render]" hints.
pub fn render_defaults(config: &Config) -> RenderConfig {
    config.render.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_config(dir: &tempfile::TempDir, text: &str) -> PathBuf {
        let path = dir.path().join("petweave.toml");
        std::fs::write(&path, text).expect("write config");
        path
    }

    const COMMENTS: &str = r##"# 全局注释：不要删我
[general]
fps = 60

# 第一只宠物的注释
[[pets]]
id = "cat"
# 名字注释
name = "Cat"
kind = "bongo"
width = 200

[pets.bongo]
# 猫高注释
cat_height = 120
"##;

    #[test]
    fn save_preserves_comments_and_updates_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, COMMENTS);
        let mut store = Store::load(path.clone()).expect("load");
        assert!(!store.is_dirty(&store.baseline.clone()));

        let mut draft = store.baseline.clone();
        draft.pets[0].name = "Kitty".into();
        draft.pets[0].width = None; // 恢复继承
        draft.pets[0].bongo.cat_height = 140;
        assert!(store.is_dirty(&draft));

        let report = store.save(&draft).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        for comment in [
            "# 全局注释：不要删我",
            "# 第一只宠物的注释",
            "# 名字注释",
            "# 猫高注释",
        ] {
            assert!(text.contains(comment), "comment lost: {comment}\n{text}");
        }
        assert!(text.contains(r#"name = "Kitty""#), "{text}");
        assert!(text.contains("cat_height = 140"), "{text}");
        assert!(!text.contains("width = 200"), "reset key removed:\n{text}");

        let reloaded = Config::parse(&text).expect("parse");
        reloaded.validate().expect("valid");
        assert_eq!(reloaded.pets[0].name, "Kitty");
        assert_eq!(reloaded.pets[0].width, None);
        assert_eq!(reloaded.pets[0].bongo.cat_height, 140);

        assert!(!store.is_dirty(&draft), "baseline updated");
        assert_eq!(report.path, path);
        let backup = report.backup.expect("backup");
        let backup_text = std::fs::read_to_string(backup).unwrap();
        assert!(backup_text.contains(r#"name = "Cat""#));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config is private");
    }

    #[test]
    fn save_adds_removes_and_reorders_pets() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            r##"# a 的注释
[[pets]]
id = "a"
name = "A"
kind = "demo"

# b 的注释
[[pets]]
id = "b"
name = "B"
kind = "bongo"
"##,
        );
        let mut store = Store::load(path.clone()).expect("load");

        let mut draft = store.baseline.clone();
        draft.pets.swap(0, 1);
        let c = PetInstance {
            id: Some("c".into()),
            name: "C".into(),
            kind: "sprite".into(),
            package: "blinky".into(),
            ..PetInstance::default()
        };
        draft.pets.push(c);
        store.save(&draft).expect("save reordered");

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# a 的注释") && text.contains("# b 的注释"));
        let parsed = Config::parse(&text).unwrap();
        let ids: Vec<_> = parsed.resolve_pets().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["b", "a", "c"], "order follows the GUI\n{text}");
        assert_eq!(parsed.pets[2].package, "blinky");

        // Remove "a" and save again: its comment goes away with the table.
        let mut draft = store.baseline.clone();
        draft.pets.retain(|p| p.id.as_deref() != Some("a"));
        store.save(&draft).expect("save removed");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("# a 的注释"));
        assert!(text.contains("# b 的注释"));
        assert_eq!(Config::parse(&text).unwrap().pets.len(), 2);
    }

    #[test]
    fn invalid_draft_is_rejected_and_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, COMMENTS);
        let before = std::fs::read_to_string(&path).unwrap();
        let mut store = Store::load(path.clone()).expect("load");

        let mut draft = store.baseline.clone();
        draft.pets[0].width = Some(99_999);
        let err = store.save(&draft).expect_err("must reject");
        assert!(matches!(err, SaveError::Invalid(_)), "{err:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(store.is_dirty(&draft), "failed save keeps dirty state");
    }

    #[test]
    fn external_modification_blocks_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, COMMENTS);
        let mut store = Store::load(path.clone()).expect("load");

        // Someone else edits the file after we loaded it.
        std::fs::write(&path, format!("{COMMENTS}\n# appended\n")).unwrap();
        let mut draft = store.baseline.clone();
        draft.pets[0].name = "Changed".into();
        let err = store.save(&draft).expect_err("must not overwrite");
        assert!(matches!(err, SaveError::ExternalModified), "{err:?}");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("# appended"));
    }

    #[test]
    fn legacy_pet_is_migrated_on_load_and_saved_as_pets() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            r##"# 旧配置注释
[pet]
name = "Old"
kind = "demo"
width = 128

[pet.bongo]
cat_height = 90
"##,
        );
        let mut store = Store::load(path.clone()).expect("load");
        assert!(store.migrated_legacy);
        assert_eq!(store.baseline.pets.len(), 1);
        assert_eq!(store.baseline.pets[0].id.as_deref(), Some("default"));

        let report = store.save(&store.baseline.clone()).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# 旧配置注释"));
        assert!(text.contains("[[pets]]"), "{text}");
        assert!(text.contains(r#"id = "default""#), "{text}");
        assert!(!text.contains("\n[pet]"), "legacy table gone:\n{text}");
        assert!(text.contains("[pets.bongo]"), "{text}");
        assert!(report.backup.is_some());
        assert!(!store.migrated_legacy, "flag clears after save");
    }

    #[test]
    fn missing_file_is_created_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/petweave.toml");
        let store = Store::load(path.clone()).expect("load defaults");
        assert!(store.baseline.pets.len() == 1);
        let mut store = store;
        store.save(&store.baseline.clone()).expect("save");

        let text = std::fs::read_to_string(&path).unwrap();
        let parsed = Config::parse(&text).unwrap();
        parsed.validate().expect("valid default config");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let parent_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(parent_mode, 0o700);
    }

    #[test]
    fn inheriting_removes_the_override_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            r#"[[pets]]
id = "cat"
name = "Cat"
kind = "demo"
width = 200
height = 100
anchor = "top|left"
mode = "physics"
click_through = false
"#,
        );
        let mut store = Store::load(path.clone()).unwrap();
        let mut draft = store.baseline.clone();
        draft.pets[0].width = None;
        draft.pets[0].height = None;
        draft.pets[0].anchor = None;
        draft.pets[0].mode = None;
        draft.pets[0].click_through = None;
        store.save(&draft).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        for key in [
            "width =",
            "height =",
            "anchor =",
            "mode =",
            "click_through =",
        ] {
            assert!(!text.contains(key), "{key} should be inherited:\n{text}");
        }
        let parsed = Config::parse(&text).unwrap();
        assert_eq!(parsed.pets[0].width, None);
        assert_eq!(parsed.pets[0].mode, None);
    }

    #[test]
    fn bongo_default_removes_nested_key_and_empty_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            r#"[[pets]]
id = "cat"
name = "Cat"
kind = "bongo"
[pets.bongo]
cat_height = 120
"#,
        );
        let mut store = Store::load(path.clone()).unwrap();
        let mut draft = store.baseline.clone();
        draft.pets[0].bongo.cat_height = 110; // default
        store.save(&draft).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("cat_height"), "{text}");
        assert!(
            !text.contains("[pets.bongo]"),
            "empty table removed:\n{text}"
        );
        let parsed = Config::parse(&text).unwrap();
        assert_eq!(parsed.pets[0].bongo.cat_height, 110);
    }

    #[test]
    fn global_fps_is_saved_and_comments_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            r#"# 全局注释
[general]
# 帧率注释
fps = 60

[[pets]]
id = "cat"
name = "Cat"
kind = "demo"
"#,
        );
        let mut store = Store::load(path.clone()).unwrap();
        let mut draft = store.baseline.clone();
        draft.general.fps = 30;
        store.save(&draft).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# 全局注释") && text.contains("# 帧率注释"),
            "{text}"
        );
        assert!(text.contains("fps = 30"), "{text}");
        let parsed = Config::parse(&text).unwrap();
        assert_eq!(parsed.general.fps, 30);
        assert!(!store.is_dirty(&draft));
    }

    #[test]
    fn mode_alias_is_canonicalized_on_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            r#"[[pets]]
id = "cat"
name = "Cat"
kind = "demo"
move_mode = "physics"
"#,
        );
        let mut store = Store::load(path.clone()).unwrap();
        assert_eq!(store.baseline.pets[0].move_mode.as_deref(), Some("physics"));
        assert!(
            !store.is_dirty(&store.baseline.clone()),
            "alias is not dirty"
        );

        let mut draft = store.baseline.clone();
        draft.pets[0].mode = Some("drag".into());
        draft.pets[0].move_mode = None;
        store.save(&draft).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"mode = "drag""#), "{text}");
        assert!(!text.contains("move_mode"), "legacy alias removed:\n{text}");
    }

    #[test]
    fn normalize_treats_zero_and_empty_as_inherited() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            r#"[[pets]]
id = "cat"
name = "Cat"
kind = "demo"
width = 0
anchor = ""
output = ""
mode = ""
"#,
        );
        let store = Store::load(path).unwrap();
        let pet = &store.baseline.pets[0];
        assert_eq!(pet.width, None);
        assert_eq!(pet.anchor, None);
        assert_eq!(pet.output, None);
        assert_eq!(pet.mode, None);
        assert!(!store.is_dirty(&store.baseline.clone()));
    }
}
