//! Demo pet: a colored square with two eyes that flashes white while any key
//! is held down.
//!
//! Proves the full pipeline end-to-end: evdev input -> event bus -> pet state
//! -> frame render -> layer-shell presentation.

use petweave_core::config::PetConfig;
use petweave_core::events::Event;
use petweave_core::pet::{Pet, PetId};
use petweave_core::render::Frame;

pub struct DemoPet {
    id: PetId,
    base: [u8; 4],
    color: [u8; 4],
}

impl DemoPet {
    pub fn new(cfg: &PetConfig) -> Self {
        let base = parse_hex_color(&cfg.color).unwrap_or([0xff, 0x66, 0x99, 0xff]);
        Self {
            id: PetId(format!("demo:{}", cfg.name)),
            base,
            color: base,
        }
    }
}

impl Pet for DemoPet {
    fn id(&self) -> &PetId {
        &self.id
    }

    fn name(&self) -> &str {
        "demo"
    }

    fn on_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Input(ev) if ev.pressed => {
                self.color = [0xff, 0xff, 0xff, 0xff]; // flash white
                true
            }
            Event::Input(_) => {
                self.color = self.base;
                true
            }
            _ => false,
        }
    }

    fn render(&self, frame: &mut Frame) {
        frame.fill(self.color);
        // Two eyes so orientation is visible.
        let (w, h) = (frame.width as i32, frame.height as i32);
        if w >= 8 && h >= 8 {
            let eye = (h / 8).max(2) as u32;
            let cx = w / 2;
            let cy = h / 4;
            frame.fill_rect(cx - eye as i32 - (h / 8) as i32, cy, eye, eye, [0, 0, 0, 255]);
            frame.fill_rect(cx + (h / 8) as i32, cy, eye, eye, [0, 0, 0, 255]);
        }
    }

    fn reload(&mut self, cfg: &PetConfig) -> Result<(), String> {
        self.base = parse_hex_color(&cfg.color).unwrap_or(self.base);
        self.color = self.base;
        Ok(())
    }
}

/// Parse `#rrggbb` or `#rrggbbaa` into RGBA. `None` on malformed input.
fn parse_hex_color(s: &str) -> Option<[u8; 4]> {
    let s = s.strip_prefix('#')?;
    let mut bytes = [0u8; 4];
    bytes[3] = 0xff;
    match s.len() {
        6 => {
            for (i, b) in bytes[..3].iter_mut().enumerate() {
                *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
            }
        }
        8 => {
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
            }
        }
        _ => return None,
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_and_rgba() {
        assert_eq!(parse_hex_color("#ff6699"), Some([0xff, 0x66, 0x99, 0xff]));
        assert_eq!(
            parse_hex_color("#11223344"),
            Some([0x11, 0x22, 0x33, 0x44])
        );
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(parse_hex_color("ff6699"), None);
        assert_eq!(parse_hex_color("#ff66"), None);
        assert_eq!(parse_hex_color("#gggggg"), None);
    }
}
