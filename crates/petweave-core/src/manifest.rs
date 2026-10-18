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
    #[serde(alias = "surface-width")]
    pub surface_width: Option<u32>,
    #[serde(alias = "surface-height")]
    pub surface_height: Option<u32>,
    /// Lua entry script (kind = "lua"); default "main.lua".
    pub script: String,

    // --- sprite key handling (runtime packages) --------------------------
    // Every key below is accepted in TOML with underscores or dashes
    // (`key_mode` / `key-mode`): the field names are the documented spelling,
    // the aliases keep the dashed form working.
    /// Key event semantics: `paw` (default, BongoCat-style one-shot
    /// reactions to the left/right keyboard halves) or `press` (taps
    /// alternate between the two reactions and stay while held).
    #[serde(alias = "key-mode")]
    pub key_mode: String,
    /// Press mode: alternate `key-left` / `key-right` on every tap.
    pub alternate: bool,
    /// Press mode: alternate the two artworks regardless of the key's half
    /// of the keyboard (both reactions are "hit" poses).
    #[serde(alias = "force-alternation")]
    pub force_alternation: bool,
    /// Blank rows reserved *around* the pose for script-driven movement
    /// (`pet.offset`): the surface is that much bigger than the artwork in
    /// both directions, so a small hop never clips. 0 = no headroom.
    pub padding: u32,
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
///
/// Two spellings are accepted (see the `Deserialize` impl; TOML shares one
/// key so both cannot appear in the same file):
///
/// * a table with named keys — `[reactions] idle = "idle" …`;
/// * an array of per-key entries — `[[reactions]] keys = [2] animation = "…"`.
///
/// Array entries are collected into [`Reactions::extra`].
#[derive(Debug, Clone, Default)]
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
    /// Per-key reactions: pressing any of `keys` (Linux EV_KEY codes) shows
    /// `animation`. Both `[[reactions]]` entries and the equivalent
    /// `[[reactions.extra]]` spelling land here; entries keep file order, so
    /// a later entry for the same key wins.
    pub extra: Vec<KeyReaction>,
}

impl<'de> Deserialize<'de> for Reactions {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};

        /// One key inside `reactions`: a named reaction (a string value) or
        /// the `keys` / `animation` pair of a per-key entry.
        #[derive(Clone, Copy)]
        enum Field {
            Idle,
            KeyLeft,
            KeyRight,
            KeyBoth,
            Click,
            EntryKeys,
            EntryAnimation,
            /// `[[reactions.extra]]` (older docs spelling of the array).
            Extra,
            Other,
        }

        impl<'de> Deserialize<'de> for Field {
            fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                struct FieldVisitor;
                impl Visitor<'_> for FieldVisitor {
                    type Value = Field;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str("a reaction key name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        Ok(match v {
                            "idle" => Field::Idle,
                            "key-left" => Field::KeyLeft,
                            "key-right" => Field::KeyRight,
                            "key-both" => Field::KeyBoth,
                            "click" => Field::Click,
                            "keys" => Field::EntryKeys,
                            "animation" => Field::EntryAnimation,
                            "extra" => Field::Extra,
                            _ => Field::Other,
                        })
                    }
                }
                de.deserialize_identifier(FieldVisitor)
            }
        }

        /// A map is a `[reactions]` table (named keys and/or one entry);
        /// a sequence is the `[[reactions]]` array-of-tables form.
        struct ReactionsVisitor;

        impl<'de> Visitor<'de> for ReactionsVisitor {
            type Value = Reactions;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a [reactions] table or a [[reactions]] array")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Reactions, A::Error> {
                let mut out = Reactions::default();
                let mut entry: Option<KeyReaction> = None;
                while let Some(field) = map.next_key::<Field>()? {
                    match field {
                        Field::Idle => out.idle = map.next_value()?,
                        Field::KeyLeft => out.key_left = map.next_value()?,
                        Field::KeyRight => out.key_right = map.next_value()?,
                        Field::KeyBoth => out.key_both = map.next_value()?,
                        Field::Click => out.click = map.next_value()?,
                        Field::EntryKeys => {
                            let e = entry.get_or_insert_with(KeyReaction::default);
                            e.keys = map.next_value()?;
                            e.is_key_entry = true;
                        }
                        Field::EntryAnimation => {
                            entry.get_or_insert_with(KeyReaction::default).animation =
                                map.next_value()?;
                        }
                        Field::Extra => {
                            out.extra.append(&mut map.next_value::<Vec<KeyReaction>>()?);
                        }
                        Field::Other => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                if let Some(e) = entry {
                    out.extra.push(e);
                }
                Ok(out)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Reactions, A::Error> {
                let mut out = Reactions::default();
                while let Some(e) = seq.next_element::<KeyReaction>()? {
                    // `is_key_entry` is decided per element by
                    // `KeyReaction::deserialize` (an element that only sets
                    // named fields, e.g. `idle = "…"`, is not a key entry).
                    let named = e.named.clone();
                    out.idle = named.idle.or(out.idle);
                    out.key_left = named.key_left.or(out.key_left);
                    out.key_right = named.key_right.or(out.key_right);
                    out.key_both = named.key_both.or(out.key_both);
                    out.click = named.click.or(out.click);
                    if e.is_key_entry {
                        out.extra.push(e);
                    }
                }
                Ok(out)
            }
        }

        de.deserialize_struct("Reactions", &[], ReactionsVisitor)
    }
}

/// One `[[reactions]]` element: named fields (e.g. `idle`) and/or a
/// per-key reaction — pressing any of `keys` (Linux EV_KEY codes) shows
/// `animation`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyReaction {
    /// Keys this entry reacts to; an entry in the array form must name at
    /// least one.
    pub keys: Vec<u32>,
    pub animation: Option<String>,
    /// Set in memory (not from TOML): this entry is a per-key reaction, so
    /// `keys` is required. A named-only `[[reactions]]` element (e.g.
    /// `idle = "…"`) leaves it false.
    pub is_key_entry: bool,
    /// Named reactions carried by an array element (`[[reactions]]` and
    /// `[reactions]` are the same TOML key, so a package that lists key
    /// bindings can still name `idle` in an element instead of a table).
    pub named: NamedReactions,
}

/// The named (non per-key) reactions of `[reactions]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamedReactions {
    pub idle: Option<String>,
    pub key_left: Option<String>,
    pub key_right: Option<String>,
    pub key_both: Option<String>,
    pub click: Option<String>,
}

impl<'de> Deserialize<'de> for KeyReaction {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        use serde::de::{IgnoredAny, MapAccess, Visitor};

        struct KeyReactionVisitor;

        impl<'de> Visitor<'de> for KeyReactionVisitor {
            type Value = KeyReaction;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a reactions entry")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<KeyReaction, A::Error> {
                let mut out = KeyReaction::default();
                // Unknown keys are present for named entries (`idle`, …);
                // those entries are not per-key reactions.
                let mut named = false;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "keys" => {
                            let keys: Vec<u32> = map.next_value()?;
                            out.is_key_entry = true;
                            out.keys = keys;
                        }
                        "animation" => out.animation = map.next_value()?,
                        "idle" => out.named.idle = map.next_value()?,
                        "key-left" => out.named.key_left = map.next_value()?,
                        "key-right" => out.named.key_right = map.next_value()?,
                        "key-both" => out.named.key_both = map.next_value()?,
                        "click" => out.named.click = map.next_value()?,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                            named = true;
                        }
                    }
                }
                // An entry that only carries named fields is not a key
                // entry; the sentinel value of an empty `keys = []` is still
                // a (rejected) per-key entry.
                // (`named` is informational: `keys` wins.)
                let _ = named;
                Ok(out)
            }
        }

        de.deserialize_map(KeyReactionVisitor)
    }
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
            key_mode: String::new(),
            alternate: false,
            force_alternation: false,
            padding: 0,
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
        if !self.pet.key_mode.is_empty() && !matches!(self.pet.key_mode.as_str(), "paw" | "press")
        {
            return Err(Error::Config(format!(
                "pet.key-mode must be \"paw\" or \"press\", got {:?}",
                self.pet.key_mode
            )));
        }
        for (i, r) in self.reactions.extra.iter().enumerate() {
            if !r.is_key_entry {
                continue; // a named-only entry (e.g. `[[reactions]] idle = …`)
            }
            if r.keys.is_empty() {
                return Err(Error::Config(format!(
                    "reactions[{i}]: keys must list at least one EV_KEY code"
                )));
            }
            match r.animation.as_deref() {
                Some(id) if !has(id) => {
                    return Err(Error::Config(format!(
                        "reactions[{i}] references unknown animation {id:?}"
                    )));
                }
                Some(_) => {}
                None => {
                    return Err(Error::Config(format!(
                        "reactions[{i}]: animation is required"
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

    /// `[[reactions]]` (the documented spelling) and `[[reactions.extra]]`
    /// both feed `Reactions::extra`, next to the named keys.
    #[test]
    fn array_of_tables_reactions_land_in_extra() {
        let base = SAMPLE.replace(
            "        [reactions]\n        idle = \"idle\"\n        key-left = \"paw-left\"\n",
            "",
        );
        let array = format!(
            "{base}[[reactions]]\nkeys = [2]\nanimation = \"paw-left\"\n\n\
             [[reactions]]\nkeys = [3, 4]\nanimation = \"idle\"\n"
        );
        let m = Manifest::from_toml(&array).expect("parse [[reactions]]");
        assert!(m.reactions.idle.is_none());
        assert!(m.reactions.key_left.is_none());
        assert_eq!(m.reactions.extra.len(), 2);
        assert_eq!(m.reactions.extra[0].keys, vec![2]);
        assert_eq!(m.reactions.extra[0].animation.as_deref(), Some("paw-left"));
        assert_eq!(m.reactions.extra[1].keys, vec![3, 4]);
        assert_eq!(m.reactions.extra[1].animation.as_deref(), Some("idle"));

        // The older `[[reactions.extra]]` spelling still works.
        let nested = format!(
            "{base}[[reactions.extra]]\nkeys = [16, 17]\nanimation = \"paw-left\"\n"
        );
        let m = Manifest::from_toml(&nested).expect("parse [[reactions.extra]]");
        assert_eq!(m.reactions.extra.len(), 1);
        assert_eq!(m.reactions.extra[0].keys, vec![16, 17]);

        // An array element can name a named reaction instead (`[reactions]`
        // and `[[reactions]]` are the same TOML key, so this is how a package
        // sets `idle` next to key bindings): it lands in the named slot and
        // is not a key entry.
        let named = format!("{base}[[reactions]]\nidle = \"idle\"\n");
        let m = Manifest::from_toml(&named).expect("named element");
        assert_eq!(m.reactions.idle.as_deref(), Some("idle"));
        assert!(m.reactions.extra.is_empty());

        // An explicit empty `keys` list is still a (rejected) key entry.
        let bad = format!("{base}[[reactions]]\nkeys = []\nanimation = \"idle\"\n");
        let err = Manifest::from_toml(&bad).expect_err("empty keys entry");
        assert!(err.to_string().contains("reactions[0]"), "{err}");
        // An entry naming a missing animation is rejected too.
        let bad = format!("{base}[[reactions]]\nkeys = [2]\nanimation = \"zzz\"\n");
        let err = Manifest::from_toml(&bad).expect_err("unknown animation");
        assert!(err.to_string().contains("zzz"), "{err}");

        // A package that only uses the array form has no named keys.
        let array_only = r#"
            [meta]
            name = "expr"
            [pet]
            kind = "sprite"
            [animations.basic]
            sheet = "s.png"
            cell_width = 10
            cell_height = 10
            [[reactions]]
            keys = [2]
            animation = "basic"
        "#;
        let m = Manifest::from_toml(array_only).expect("parse");
        assert_eq!(m.reactions.extra.len(), 1);
        assert!(m.reactions.idle.is_none());
    }

    /// The kebab-case spelling of the `[pet]` knobs is the documented one and
    /// must parse; the underscored names keep working too.
    #[test]
    fn pet_knobs_accept_both_spellings() {
        let dashed = r#"
            [meta]
            name = "p"
            [pet]
            kind = "sprite"
            key-mode = "press"
            force-alternation = true
            padding = 6
            surface-width = 100
            surface-height = 60
            [animations.idle]
            sheet = "s.png"
            cell_width = 10
            cell_height = 10
        "#;
        let m = Manifest::from_toml(dashed).expect("kebab-case");
        assert_eq!(m.pet.key_mode, "press");
        assert!(m.pet.force_alternation);
        assert_eq!(m.pet.padding, 6);
        assert_eq!(m.pet.surface_width, Some(100));

        let underscored = dashed
            .replace("key-mode", "key_mode")
            .replace("force-alternation", "force_alternation")
            .replace("surface-width", "surface_width")
            .replace("surface-height", "surface_height");
        let m = Manifest::from_toml(&underscored).expect("snake_case");
        assert_eq!(m.pet.key_mode, "press");
        assert!(m.pet.force_alternation);
        assert_eq!(m.pet.surface_height, Some(60));
    }

    /// The removed bob/anchor knobs are simply unknown TOML keys now: they do
    /// not fail loading (older packages keep working), they are ignored.
    #[test]
    fn removed_bob_knobs_are_ignored() {
        let text = SAMPLE.replace(
            "kind = \"sprite\"",
            "kind = \"sprite\"\n        bob_pixels = 6\n        anchor_padding = 4\n        anchor_mode = \"top\"",
        );
        let m = Manifest::from_toml(&text).expect("old packages still load");
        assert_eq!(m.pet.kind, "sprite");
        assert_eq!(m.pet.padding, 0);
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
