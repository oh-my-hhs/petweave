//! Configuration: TOML file with defaults, CLI overrides on top.

use std::path::Path;

use serde::Deserialize;

use crate::error::Error;

/// Top-level configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub input: InputConfig,
    pub render: RenderConfig,
    pub pet: PetConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct General {
    /// Animation/render loop target FPS cap.
    pub fps: u32,
    /// Log level: trace|debug|info|warn|error.
    pub log_level: String,
    /// How often to sample CPU/memory and emit `Event::System` (0 = off).
    pub sysinfo_interval_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// Master switch for global keyboard capture.
    pub enabled: bool,
    /// Auto-detect keyboards under /dev/input/event*.
    pub auto_detect: bool,
    /// Explicit device paths (e.g. "/dev/input/event4"). Empty = none.
    pub devices: Vec<String>,
    /// Hotplug rescan interval in seconds (reserved; M1).
    pub scan_interval_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RenderConfig {
    /// Pet surface width in pixels.
    pub width: u32,
    /// Pet surface height in pixels.
    pub height: u32,
    /// wlr-layer-shell layer: background|bottom|top|overlay.
    pub layer: String,
    /// Anchor: one or more of top|bottom|left|right (| separated).
    pub anchor: String,
    pub margin_top: i32,
    pub margin_right: i32,
    pub margin_bottom: i32,
    pub margin_left: i32,
    /// Output to bind the surface to, by name (xdg-output). Empty = auto.
    pub output: String,
    /// Never auto-hide the pet for fullscreen windows.
    pub disable_fullscreen_hide: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PetConfig {
    /// Pet instance name (used in ids and logs).
    pub name: String,
    /// Whether the pet is loaded.
    pub enabled: bool,
    /// Pet kind: "demo" | "bongo" (role packages arrive with M2).
    pub kind: String,
    /// Demo pet base color as #rrggbb[aa].
    pub color: String,
    /// BongoCat-specific options.
    pub bongo: BongoConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BongoConfig {
    /// Directory holding the four bongo cat PNG frames.
    pub assets_dir: String,
    /// Target cat height in pixels; width follows the asset aspect ratio.
    pub cat_height: u32,
    /// How long a paw stays down after a key press, in ms.
    pub keypress_duration_ms: u64,
    /// Map keys to left/right paws by physical position.
    pub hand_mapping: bool,
    /// Flip the cat horizontally (and swap the paw mapping).
    pub mirror_x: bool,
    /// Sleep after this many seconds without a key press (0 = disabled).
    pub idle_sleep_timeout_secs: u64,
    /// Enable the scheduled sleep window (wall-clock).
    pub enable_scheduled_sleep: bool,
    /// Sleep window start, "HH:MM" (24h).
    pub sleep_begin: String,
    /// Sleep window end, "HH:MM" (24h).
    pub sleep_end: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            input: InputConfig::default(),
            render: RenderConfig::default(),
            pet: PetConfig::default(),
        }
    }
}

impl Default for General {
    fn default() -> Self {
        Self {
            fps: 60,
            log_level: "info".to_string(),
            sysinfo_interval_secs: 5,
        }
    }
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_detect: true,
            devices: Vec::new(),
            scan_interval_secs: 30,
        }
    }
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
            layer: "top".to_string(),
            anchor: "bottom".to_string(),
            margin_top: 16,
            margin_right: 0,
            margin_bottom: 16,
            margin_left: 0,
            output: String::new(),
            disable_fullscreen_hide: false,
        }
    }
}

impl Default for PetConfig {
    fn default() -> Self {
        Self {
            name: "demo".to_string(),
            enabled: true,
            kind: "demo".to_string(),
            color: "#ff6699".to_string(),
            bongo: BongoConfig::default(),
        }
    }
}

impl Default for BongoConfig {
    fn default() -> Self {
        Self {
            assets_dir: "assets/bongocat".to_string(),
            cat_height: 110,
            keypress_duration_ms: 100,
            hand_mapping: true,
            mirror_x: false,
            idle_sleep_timeout_secs: 0,
            enable_scheduled_sleep: false,
            sleep_begin: "22:00".to_string(),
            sleep_end: "06:00".to_string(),
        }
    }
}

impl Config {
    /// Load config from `path` (if given and readable), otherwise defaults.
    pub fn load(path: Option<&Path>) -> Result<Self, Error> {
        let mut cfg = Config::default();
        if let Some(p) = path {
            let text = std::fs::read_to_string(p)
                .map_err(|e| Error::Config(format!("cannot read {}: {e}", p.display())))?;
            let parsed: Config = toml::from_str(&text)
                .map_err(|e| Error::Config(format!("invalid TOML in {}: {e}", p.display())))?;
            cfg = parsed;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate ranges and enum-like values.
    pub fn validate(&self) -> Result<(), Error> {
        if !(1..=240).contains(&self.general.fps) {
            return Err(Error::Config("general.fps must be in 1..=240".into()));
        }
        if self.render.width == 0
            || self.render.height == 0
            || self.render.width > 4096
            || self.render.height > 4096
        {
            return Err(Error::Config(
                "render.width/height must be in 1..=4096".into(),
            ));
        }
        match self.render.layer.as_str() {
            "background" | "bottom" | "top" | "overlay" => {}
            other => {
                return Err(Error::Config(format!(
                    "render.layer must be one of background|bottom|top|overlay, got {other:?}"
                )));
            }
        }
        match self.pet.kind.as_str() {
            "demo" | "bongo" => {}
            other => {
                return Err(Error::Config(format!(
                    "pet.kind must be one of demo|bongo, got {other:?}"
                )));
            }
        }
        if !(10..=500).contains(&self.pet.bongo.cat_height) {
            return Err(Error::Config("pet.bongo.cat_height must be in 10..=500".into()));
        }
        for (name, t) in [
            ("pet.bongo.sleep_begin", &self.pet.bongo.sleep_begin),
            ("pet.bongo.sleep_end", &self.pet.bongo.sleep_end),
        ] {
            if !is_hhmm(t) {
                return Err(Error::Config(format!("{name} must be \"HH:MM\" (24h), got {t:?}")));
            }
        }
        Ok(())
    }
}

/// Validate a "HH:MM" clock string.
pub fn is_hhmm(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
        && (b[0] - b'0') * 10 + (b[1] - b'0') < 24
        && (b[3] - b'0') * 10 + (b[4] - b'0') < 60
}

/// Parse "HH:MM" into minutes since midnight.
pub fn hhmm_to_minutes(s: &str) -> Option<u32> {
    if !is_hhmm(s) {
        return None;
    }
    let b = s.as_bytes();
    Some(((b[0] - b'0') as u32) * 600 + ((b[1] - b'0') as u32) * 60 + (b[3] - b'0') as u32 * 10 + (b[4] - b'0') as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.general.fps, 60);
        assert_eq!(cfg.render.width, 256);
        assert!(cfg.input.enabled);
        assert!(cfg.input.auto_detect);
    }

    #[test]
    fn parses_toml_with_partial_sections() {
        let text = r#"
            [render]
            width = 128
            height = 96
            layer = "overlay"
        "#;
        let cfg: Config = toml::from_str(text).expect("parse");
        assert_eq!(cfg.render.width, 128);
        assert_eq!(cfg.render.height, 96);
        assert_eq!(cfg.render.layer, "overlay");
        // Missing sections fall back to defaults.
        assert_eq!(cfg.general.fps, 60);
        assert_eq!(cfg.pet.name, "demo");
    }

    #[test]
    fn rejects_bad_layer() {
        let cfg = Config {
            render: RenderConfig {
                layer: "mid-air".into(),
                ..RenderConfig::default()
            },
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_out_of_range_fps() {
        let cfg = Config {
            general: General { fps: 0, ..General::default() },
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn bongo_defaults_and_kind_validation() {
        let cfg = Config::default();
        assert_eq!(cfg.pet.kind, "demo");
        assert_eq!(cfg.pet.bongo.cat_height, 110);
        assert!(cfg.validate().is_ok());
        let cfg = Config {
            pet: PetConfig {
                kind: "shimeji".into(),
                ..PetConfig::default()
            },
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn sleep_clock_parsing() {
        assert!(is_hhmm("22:00"));
        assert!(is_hhmm("06:30"));
        assert!(!is_hhmm("24:00"));
        assert!(!is_hhmm("6:00"));
        assert!(!is_hhmm("12:60"));
        assert!(!is_hhmm("1200"));
        assert_eq!(hhmm_to_minutes("22:00"), Some(22 * 60));
        assert_eq!(hhmm_to_minutes("00:05"), Some(5));
        assert_eq!(hhmm_to_minutes("bad"), None);
    }

    #[test]
    fn rejects_bad_sleep_window() {
        let cfg = Config {
            pet: PetConfig {
                bongo: BongoConfig {
                    sleep_begin: "25:00".into(),
                    ..BongoConfig::default()
                },
                ..PetConfig::default()
            },
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }
}
