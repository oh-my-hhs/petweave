//! Graphics helpers: frame conversion and (later) backends.

use petweave_core::render::Frame;

/// Blit an RGBA [`Frame`] into a Wayland `ARGB8888` (BGRA in memory) canvas.
///
/// `dst` must have room for `frame.width * frame.height * 4` bytes; if it is
/// larger (e.g. stride padding), only the frame area is written.
pub fn blit_rgba_to_bgra(frame: &Frame, dst: &mut [u8]) {
    let n = (frame.pixels.len()).min(dst.len());
    let src = &frame.pixels[..n];
    let dst = &mut dst[..n];
    for (chunk, out) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        out[0] = chunk[2]; // B
        out[1] = chunk[1]; // G
        out[2] = chunk[0]; // R
        out[3] = chunk[3]; // A
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petweave_core::render::Frame;

    #[test]
    fn swaps_rgba_to_bgra() {
        let mut frame = Frame::new(1, 1);
        frame.pixels = vec![0x11, 0x22, 0x33, 0x44];
        let mut dst = vec![0u8; 4];
        blit_rgba_to_bgra(&frame, &mut dst);
        assert_eq!(dst, vec![0x33, 0x22, 0x11, 0x44]);
    }

    #[test]
    fn handles_short_dst() {
        let mut frame = Frame::new(2, 1);
        frame.pixels = vec![0xff; 8];
        let mut dst = vec![0u8; 4];
        blit_rgba_to_bgra(&frame, &mut dst);
        assert_eq!(dst, vec![0xff; 4]);
    }
}
