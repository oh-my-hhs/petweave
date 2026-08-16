//! Role-package manifest (`pet.toml` inside a `.petweave` package).

use std::collections::HashMap;

use serde::Deserialize;

use crate::error::Error;

/// Top-level manifest of a `.petweave` package.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub meta: Meta,
    pub pet: PetDecl,
    /// Named animations; the keys are referenced by `reactions`.
    pub animations: HashMap<String, Animation>,
    pub reactions: Reactions,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Meta {
    /// Package name — filesystem-safe: `[a-zA-Z0-9._-]+`.
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub license: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PetDecl {
    /// Pet runtime kind: "sprite" | "lua" (Live2D later).
    pub kind: String,
    /// Optional surface size override (default: animation cell size).
    pub surface_width: Option<u32>,
    pub surface_height: Option<u32>,
    /// Lua entry script (kind = "lua"); default "main.lua".
    pub script: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Animation {
    /// Path (inside the package) to the sprite sheet PNG.
    pub sheet: String,
    /// Size of one frame cell in the sheet (grid, row-major).
    pub cell_width: u32,
    pub cell_height: u32,
    /// Cell indices to play in order (row-major); default `[0]`.
    pub frames: Vec<u32>,
    /// Playback rate in frames per second; default 1.
    pub fps: u32,
    /// Loop the animation; default false.
    #[serde(rename = "loop")]
    pub loop_: bool,
}

/// Event -> animation wiring (declarative behavior, zero code).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
pub struct Reactions {
    /// Animation id played whenever no reaction is active.
    pub idle: Option<String>,
    /// Animation played when a left-hand key is pressed.
    pub key_left: Option<String>,
    /// Animation played when a right-hand key is pressed.
    pub key_right: Option<String>,
    /// Animation played when both hands are pressed at once.
    pub key_both: Option<String>,
    /// Animation played once when the pet is clicked with the left button.
    pub click: Option<String>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            meta: Meta::default(),
            pet: PetDecl::default(),
            animations: HashMap::new(),
            reactions: Reactions::default(),
        }
    }
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: "1.0.0".to_string(),
            author: None,
            license: None,
            description: None,
        }
    }
}

impl Default for PetDecl {
    fn default() -> Self {
        Self {
            kind: "sprite".to_string(),
            surface_width: None,
            surface_height: None,
            script: "main.lua".to_string(),
        }
    }
}

impl Default for Animation {
    fn default() -> Self {
        Self {
            sheet: String::new(),
            cell_width: 0,
            cell_height: 0,
            frames: vec![0],
            fps: 1,
            loop_: false,
        }
    }
}

impl Manifest {
    /// Parse from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, Error> {
        let m: Manifest =
            toml::from_str(text).map_err(|e| Error::Config(format!("invalid pet.toml: {e}")))?;
        m.validate()?;
        Ok(m)
    }

    /// Validate names, sizes and reaction references.
    pub fn validate(&self) -> Result<(), Error> {
        let bad = |c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
        if self.meta.name.is_empty() || self.meta.name.chars().any(bad) {
            return Err(Error::Config(format!(
                "meta.name must be a filesystem-safe name ([a-zA-Z0-9._-]), got {:?}",
                self.meta.name
            )));
        }
        if !matches!(self.pet.kind.as_str(), "sprite" | "lua") {
            return Err(Error::Config(format!(
                "pet.kind must be \"sprite\" or \"lua\" for now, got {:?}",
                self.pet.kind
            )));
        }
        if self.pet.kind == "lua" && self.pet.script.is_empty() {
            return Err(Error::Config("pet.script is required for kind = \"lua\"".into()));
        }
        for (id, anim) in &self.animations {
            if anim.sheet.is_empty() {
                return Err(Error::Config(format!("animations.{id}.sheet is required")));
            }
            if anim.cell_width == 0 || anim.cell_height == 0 {
                return Err(Error::Config(format!(
                    "animations.{id}.cell_width/cell_height must be > 0"
                )));
            }
            if anim.frames.is_empty() {
                return Err(Error::Config(format!("animations.{id}.frames must not be empty")));
            }
        }
        let has = |id: &str| self.animations.contains_key(id);
        for (what, id) in [
            ("reactions.idle", &self.reactions.idle),
            ("reactions.key_left", &self.reactions.key_left),
            ("reactions.key_right", &self.reactions.key_right),
            ("reactions.key_both", &self.reactions.key_both),
            ("reactions.click", &self.reactions.click),
        ] {
            if let Some(id) = id {
                if !has(id) {
                    return Err(Error::Config(format!(
                        "{what} references unknown animation {id:?}"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Animation frame rate clamped to a sane range.
pub fn clamp_fps(fps: u32) -> u32 {
    fps.clamp(1, 120)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        [meta]
        name = "bongo-sprite"
        version = "1.0.0"
        author = "PetWeave"
        license = "MIT"

        [pet]
        kind = "sprite"

        [animations.idle]
        sheet = "sprites/both-up.png"
        cell_width = 264
        cell_height = 110
        frames = [0]
        fps = 4
        loop = true

        [animations.paw-left]
        sheet = "sprites/left-down.png"
        cell_width = 264
        cell_height = 110

        [reactions]
        idle = "idle"
        key-left = "paw-left"
    "#;

    #[test]
    fn parses_sample_manifest() {
        let m = Manifest::from_toml(SAMPLE).expect("parse");
        assert_eq!(m.meta.name, "bongo-sprite");
        assert_eq!(m.pet.kind, "sprite");
        assert_eq!(m.animations.len(), 2);
        assert_eq!(m.animations["idle"].loop_, true);
        assert_eq!(m.animations["idle"].frames, vec![0]);
        assert_eq!(m.reactions.idle.as_deref(), Some("idle"));
        assert_eq!(m.reactions.key_left.as_deref(), Some("paw-left"));
    }

    #[test]
    fn rejects_unknown_reaction_target() {
        let text = SAMPLE.replace("key-left = \"paw-left\"", "key-left = \"nope\"");
        assert!(Manifest::from_toml(&text).is_err());
    }

    #[test]
    fn rejects_bad_name() {
        let text = SAMPLE.replace("name = \"bongo-sprite\"", "name = \"bad/name\"");
        assert!(Manifest::from_toml(&text).is_err());
    }

    #[test]
    fn rejects_zero_cell() {
        let text = SAMPLE.replace("cell_width = 264", "cell_width = 0");
        assert!(Manifest::from_toml(&text).is_err());
    }

    #[test]
    fn defaults_apply() {
        let m = Manifest::default();
        assert_eq!(m.pet.kind, "sprite");
        assert!(m.animations.is_empty());
        assert_eq!(Animation::default().frames, vec![0]);
        assert_eq!(Animation::default().fps, 1);
        assert!(!Animation::default().loop_);
    }
}
