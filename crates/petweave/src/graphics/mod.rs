//! Graphics helpers: frame conversion, scaling and SVG rasterization.

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

/// Resize a frame to `w x h` (Triangle filter), normalizing stray RGB in
/// fully transparent pixels (premultiplied-alpha hygiene).
pub fn scale_frame(frame: &Frame, w: u32, h: u32) -> Frame {
    let Some(buf) = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels.clone())
    else {
        return frame.clone();
    };
    let mut resized = image::imageops::resize(&buf, w, h, image::imageops::FilterType::Triangle);
    for px in resized.pixels_mut() {
        if px[3] == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
    }
    Frame {
        width: w,
        height: h,
        pixels: resized.into_raw(),
    }
}

/// Bounding box of pixels with alpha above the threshold, in frame coordinates.
/// `None` when the frame is fully transparent (click-through everywhere).
pub fn alpha_bbox(frame: &Frame, alpha_threshold: u8) -> Option<(i32, i32, u32, u32)> {
    let (w, h) = (frame.width as i32, frame.height as i32);
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = -1;
    let mut max_y = -1;
    for y in 0..h {
        for x in 0..w {
            let i = ((y as usize * frame.width as usize + x as usize) * 4) + 3;
            if frame.pixels[i] > alpha_threshold {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if max_x < 0 {
        None
    } else {
        Some((min_x, min_y, (max_x - min_x + 1) as u32, (max_y - min_y + 1) as u32))
    }
}

/// Rasterize an SVG at `w x h` (aspect-preserving, centered), returning a
/// transparent RGBA frame. `None` on parse/render failure.
pub fn svg_to_frame(svg_bytes: &[u8], w: u32, h: u32) -> Option<Frame> {
    use resvg::usvg;

    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg_bytes, &opt).ok()?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }
    // Rasterize at natural size, then scale (keeps the artwork aspect).
    let nw = size.width().ceil().max(1.0) as u32;
    let nh = size.height().ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(nw, nh)?;
    let mut pm = pixmap.as_mut();
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pm);
    // tiny-skia pixels are premultiplied RGBA -> straight RGBA.
    let mut pixels = Vec::with_capacity((nw * nh * 4) as usize);
    for px in pixmap.data().chunks_exact(4) {
        let (r, g, b, a) = (px[0] as u32, px[1] as u32, px[2] as u32, px[3] as u32);
        if a == 0 {
            pixels.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            pixels.extend_from_slice(&[
                ((r * 255) / a) as u8,
                ((g * 255) / a) as u8,
                ((b * 255) / a) as u8,
                a as u8,
            ]);
        }
    }
    let natural = Frame {
        width: nw,
        height: nh,
        pixels,
    };
    if nw == w && nh == h {
        Some(natural)
    } else {
        Some(scale_frame(&natural, w, h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn svg_rasterizes_with_alpha() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
            <rect x="2" y="2" width="6" height="6" fill="#ff0000"/>
        </svg>"##;
        let frame = svg_to_frame(svg, 5, 5).expect("render");
        assert_eq!((frame.width, frame.height), (5, 5));
        // Corner essentially transparent (allow AA spill), center red.
        assert!(frame.pixels[3] < 16, "corner nearly transparent");
        let center = ((5 / 2) * 5 + 5 / 2) * 4;
        assert_eq!(&frame.pixels[center..center + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn alpha_bbox_finds_content_and_empty() {
        let mut f = Frame::new(20, 10);
        // Opaque dot at (5,3) size 4x2.
        f.fill_rect(5, 3, 4, 2, [255, 0, 0, 255]);
        let bbox = alpha_bbox(&f, 8).expect("content");
        assert_eq!(bbox, (5, 3, 4, 2));
        // Fully transparent frame -> None (click-through everywhere).
        let empty = Frame::new(10, 10);
        assert_eq!(alpha_bbox(&empty, 8), None);
        // Semi-transparent pixel below threshold is ignored.
        let mut faint = Frame::new(10, 10);
        faint.fill_rect(0, 0, 1, 1, [0, 0, 0, 4]);
        assert_eq!(alpha_bbox(&faint, 8), None);
    }

    #[test]
    fn scale_frame_resizes() {
        let mut big = Frame::new(4, 2);
        big.fill([255, 0, 0, 255]);
        let small = scale_frame(&big, 2, 1);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(&small.pixels[0..4], &[255, 0, 0, 255]);
    }
}
