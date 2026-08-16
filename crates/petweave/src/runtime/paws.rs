//! Keycode -> paw mapping, shared by keyboard-reacting pets.
//!
//! Same physical-position table as wayland-bongocat's `paw_for_keycode`.

/// Which paw a key maps to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Paw {
    Left,
    Right,
}

impl Paw {
    pub fn swapped(self) -> Paw {
        match self {
            Paw::Left => Paw::Right,
            Paw::Right => Paw::Left,
        }
    }
}

/// Physical-position paw mapping (Linux keycodes).
pub fn paw_for_keycode(code: u32) -> Paw {
    const LEFT_KEYS: &[u32] = &[
        1, 2, 3, 4, 5, 6, 7, 15, 16, 17, 18, 19, 20, 29, 30, 31, 32, 33, 34, 41, 42, 44, 45, 46,
        47, 48, 56, 58, 125,
    ];
    if LEFT_KEYS.contains(&code) {
        Paw::Left
    } else {
        Paw::Right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paw_mapping_matches_reference() {
        for code in [1u32, 16, 30, 42, 44, 58, 125] {
            assert_eq!(paw_for_keycode(code), Paw::Left, "keycode {code}");
        }
        for code in [57u32, 28, 103, 106, 108] {
            assert_eq!(paw_for_keycode(code), Paw::Right, "keycode {code}");
        }
    }

    #[test]
    fn swapped_flips() {
        assert_eq!(Paw::Left.swapped(), Paw::Right);
        assert_eq!(Paw::Right.swapped(), Paw::Left);
    }
}
