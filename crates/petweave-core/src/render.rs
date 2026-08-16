//! Software render target shared by pets and backends.

use crate::error::Error;

/// A software pixel frame a pet draws into.
///
/// Pixel layout is **RGBA8** (platform-neutral), top-left origin, row-major.
/// The host converts to the backend's native format (e.g. Wayland
/// `ARGB8888` = BGRA in memory) at presentation time.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    /// Create a new transparent frame.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    /// Clear the whole frame to transparent black.
    pub fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// Fill the whole frame with a solid color.
    pub fn fill(&mut self, color: [u8; 4]) {
        for px in self.pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&color);
        }
    }

    /// Fill an axis-aligned solid rectangle (clipped to the frame bounds).
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: [u8; 4]) {
        let fw = self.width as i32;
        let fh = self.height as i32;
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x.saturating_add(w as i32)).min(fw);
        let y1 = (y.saturating_add(h as i32)).min(fh);
        for yy in y0..y1 {
            let row = yy as usize * self.width as usize;
            for xx in x0..x1 {
                let i = (row + xx as usize) * 4;
                self.pixels[i..i + 4].copy_from_slice(&color);
            }
        }
    }
}

/// Abstraction over a presentation backend.
///
/// MVP implements the CPU/SHM path (see `petweave::platform::wayland`); a GPU
/// backend (wgpu + linux-dmabuf, for Live2D/effects) plugs in behind the same
/// trait later.
pub trait RenderBackend {
    /// Present `frame` on screen.
    fn present(&mut self, frame: &Frame) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_size_is_rgba() {
        let f = Frame::new(4, 3);
        assert_eq!(f.pixels.len(), 4 * 3 * 4);
    }

    #[test]
    fn fill_rect_is_clipped() {
        let mut f = Frame::new(10, 10);
        f.fill_rect(-5, -5, 20, 20, [255, 0, 0, 255]);
        // Everything inside bounds painted, nothing outside (no panic).
        assert!(f.pixels.iter().all(|&b| b == 255 || b == 0));
        assert_eq!(&f.pixels[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn fill_rect_out_of_bounds_is_noop() {
        let mut f = Frame::new(10, 10);
        f.fill([0xff, 0xff, 0xff, 0xff]);
        f.fill_rect(100, 100, 5, 5, [0, 0, 0, 0]);
        assert!(f.pixels.iter().all(|&b| b == 0xff));
    }
}
