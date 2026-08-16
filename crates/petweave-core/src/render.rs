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

    /// Composite `img` (RGBA, src-over) at offset `(x, y)`, clipped to bounds.
    ///
    /// Used to draw transparent sprites (PNG/SVG pets) on top of a cleared
    /// frame. Premultiplied-style src-over with per-pixel alpha.
    pub fn draw_image(&mut self, x: i32, y: i32, img: &Frame) {
        let fw = self.width as i32;
        let fh = self.height as i32;
        let iw = img.width as i32;
        let ih = img.height as i32;
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + iw).min(fw);
        let y1 = (y + ih).min(fh);
        for yy in y0..y1 {
            let di = yy as usize * self.width as usize;
            let si = (yy - y) as usize * img.width as usize;
            for xx in x0..x1 {
                let d = (di + xx as usize) * 4;
                let s = (si + (xx - x) as usize) * 4;
                let a = img.pixels[s + 3];
                if a == 255 {
                    self.pixels[d..d + 4].copy_from_slice(&img.pixels[s..s + 4]);
                } else if a > 0 {
                    let sa = a as f32 / 255.0;
                    let da = self.pixels[d + 3] as f32 / 255.0;
                    let oa = sa + da * (1.0 - sa);
                    if oa > 0.0 {
                        for c in 0..3 {
                            let sc = img.pixels[s + c] as f32;
                            let dc = self.pixels[d + c] as f32;
                            // round(): truncation would drop 1 LSB on
                            // partially transparent pixels (float error).
                            self.pixels[d + c] =
                                ((sc * sa + dc * da * (1.0 - sa)) / oa).round() as u8;
                        }
                        self.pixels[d + 3] = (oa * 255.0).round() as u8;
                    }
                }
            }
        }
    }

    /// Flip the frame horizontally in place (used for `mirror_x` pets).
    pub fn flip_horizontal(&mut self) {
        let w = self.width as usize;
        let h = self.height as usize;
        for row in 0..h {
            let line = row * w;
            for col in 0..w / 2 {
                let a = (line + col) * 4;
                let b = (line + w - 1 - col) * 4;
                self.pixels.swap(a, b);
                self.pixels.swap(a + 1, b + 1);
                self.pixels.swap(a + 2, b + 2);
                self.pixels.swap(a + 3, b + 3);
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

    #[test]
    fn draw_image_opaque_overwrites() {
        let mut dst = Frame::new(4, 4);
        dst.fill([0, 0, 0, 255]);
        let mut src = Frame::new(2, 2);
        src.fill([255, 0, 0, 255]);
        dst.draw_image(1, 1, &src);
        // Outside the sprite stays black, inside becomes red.
        assert_eq!(&dst.pixels[0..4], &[0, 0, 0, 255]);
        let i = (1 * 4 + 1) * 4;
        assert_eq!(&dst.pixels[i..i + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn draw_image_alpha_blends() {
        let mut dst = Frame::new(2, 2);
        dst.fill([0, 0, 0, 255]); // opaque black dest
        let mut src = Frame::new(2, 2);
        // half-transparent red
        src.fill([255, 0, 0, 128]);
        dst.draw_image(0, 0, &src);
        let i = 0;
        // Result ≈ red at 50% over black: r≈128
        assert!((dst.pixels[i] as i32 - 128).abs() <= 2);
        assert_eq!(dst.pixels[i + 1], 0);
        assert_eq!(dst.pixels[i + 2], 0);
        assert_eq!(dst.pixels[i + 3], 255);
    }

    #[test]
    fn flip_horizontal_mirrors() {
        let mut f = Frame::new(2, 1);
        f.pixels = vec![1, 2, 3, 4, 5, 6, 7, 8];
        f.flip_horizontal();
        assert_eq!(f.pixels, vec![5, 6, 7, 8, 1, 2, 3, 4]);
    }
}
