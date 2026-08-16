//! Role packages: `.petweave` format, local repository, install tooling.
//!
//! A package is a directory (or zip) containing `pet.toml` (see
//! `petweave_core::manifest`) plus assets. `petweave install` copies it into
//! the local repository (`$XDG_DATA_HOME/petweave/pets/<name>/`); the runtime
//! loads it via `[pet] kind = "sprite"` + `package = "<name>"`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use petweave_core::manifest::Manifest;

use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Manifest file name inside a package.
pub const MANIFEST_NAME: &str = "pet.toml";
/// Package file extension.
pub const PACKAGE_EXT: &str = "petweave";

/// An installed package summary (for `petweave list`).
#[derive(Debug, Clone)]
pub struct InstalledPkg {
    pub name: String,
    pub version: String,
    pub kind: String,
    pub description: Option<String>,
}

/// Local package repository directory.
pub fn repo_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("petweave").join("pets")
}

/// Resolve a `pet.package` value to a package directory.
///
/// Accepts an existing directory (dev mode) or an installed package name.
pub fn resolve(package: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(package);
    if p.is_dir() {
        return Ok(p);
    }
    let installed = repo_dir().join(package);
    if installed.join(MANIFEST_NAME).is_file() {
        return Ok(installed);
    }
    Err(format!(
        "package {package:?} not found (not an installed package; run `petweave install`, \
         or pass a package directory path)"
    ))
}

/// Read + validate the manifest from a package directory.
pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join(MANIFEST_NAME);
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Manifest::from_toml(&text).map_err(|e| e.to_string())
}

/// Install a package (directory or `.petweave` file) into the repository.
/// Returns the installed package name.
pub fn install(src: &Path) -> Result<String, String> {
    let (name, is_zip) = if src.is_dir() {
        let m = read_manifest(src)?;
        (m.meta.name, false)
    } else if src.is_file() && src.extension().map_or(false, |e| e == PACKAGE_EXT) {
        let m = zip_manifest(src)?;
        (m.meta.name, true)
    } else {
        return Err(format!(
            "{} is neither a package directory nor a .{PACKAGE_EXT} file",
            src.display()
        ));
    };

    let dest = repo_dir().join(&name);
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| format!("cannot replace {}: {e}", dest.display()))?;
    }
    fs::create_dir_all(&dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;

    if is_zip {
        extract_zip(src, &dest)?;
    } else {
        copy_dir(src, &dest)?;
    }
    tracing::info!("installed package {name:?} -> {}", dest.display());
    Ok(name)
}

/// Remove an installed package.
pub fn uninstall(name: &str) -> Result<(), String> {
    let dir = repo_dir().join(name);
    if !dir.join(MANIFEST_NAME).is_file() {
        return Err(format!("package {name:?} is not installed"));
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
    tracing::info!("uninstalled package {name:?}");
    Ok(())
}

/// List installed packages.
pub fn list() -> Vec<InstalledPkg> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(repo_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        match read_manifest(&dir) {
            Ok(m) => out.push(InstalledPkg {
                name: m.meta.name.clone(),
                version: m.meta.version.clone(),
                kind: m.pet.kind.clone(),
                description: m.meta.description.clone(),
            }),
            Err(e) => tracing::warn!("skipping invalid package {}: {e}", dir.display()),
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Build a `.petweave` zip from a package directory.
pub fn build(src: &Path, out: &Path) -> Result<(), String> {
    let m = read_manifest(src)?;
    let file = fs::File::create(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let mut zw = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_dir() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(src)
            .map_err(|_| "path prefix error")?
            .to_string_lossy()
            .replace('\\', "/");
        zw.start_file(rel, opts).map_err(|e| e.to_string())?;
        let bytes = fs::read(entry.path()).map_err(|e| e.to_string())?;
        zw.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    zw.finish().map_err(|e| e.to_string())?;
    tracing::info!("built {} (package {})", out.display(), m.meta.name);
    Ok(())
}

// --- zip helpers -----------------------------------------------------------

fn open_zip(path: &Path) -> Result<zip::ZipArchive<fs::File>, String> {
    let f = fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    zip::ZipArchive::new(f).map_err(|e| format!("invalid package {}: {e}", path.display()))
}

fn zip_manifest(path: &Path) -> Result<Manifest, String> {
    let mut z = open_zip(path)?;
    let mut entry = z
        .by_name(MANIFEST_NAME)
        .map_err(|e| format!("package {} has no {MANIFEST_NAME}: {e}", path.display()))?;
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    Manifest::from_toml(&text).map_err(|e| e.to_string())
}

fn extract_zip(path: &Path, dest: &Path) -> Result<(), String> {
    let mut z = open_zip(path)?;
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        // Reject path traversal.
        let clean = Path::new(&name);
        if clean
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        {
            return Err(format!("unsafe path in package: {name:?}"));
        }
        let target = dest.join(clean);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = fs::File::create(&target).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn copy_dir(src: &Path, dest: &Path) -> Result<(), String> {
    for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let rel = entry
            .path()
            .strip_prefix(src)
            .map_err(|_| "path prefix error")?;
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            // Explicit read/write loop rather than fs::copy: works on any
            // filesystem (and avoids sandbox quirks with copy_file_range).
            let mut src_file = fs::File::open(entry.path()).map_err(|e| e.to_string())?;
            let mut dst_file = fs::File::create(&target).map_err(|e| e.to_string())?;
            std::io::copy(&mut src_file, &mut dst_file).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// --- XPM importer (Oneko-style sprite sheets) ------------------------------

/// Convert an XPM sprite sheet (e.g. `oneko.xpm`) to a PNG.
///
/// Supports the classic format: a header line `"W H N C"` followed by `N`
/// color-map lines `"<chars> c <color>",` and `H` pixel rows. Colors support
/// `None` (transparent), `#RGB`/`#RRGGBB`, `gray(N)`/`grey(N)` and a small set
/// of named colors.
pub fn import_xpm(input: &Path, output: &Path) -> Result<(), String> {
    let text = fs::read_to_string(input)
        .map_err(|e| format!("cannot read {}: {e}", input.display()))?;
    let (w, h, rgba) = parse_xpm(&text)?;
    image::save_buffer(output, &rgba, w, h, image::ColorType::Rgba8)
        .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    tracing::info!(
        "imported {} -> {} ({}x{})",
        input.display(),
        output.display(),
        w,
        h
    );
    Ok(())
}

fn parse_xpm(text: &str) -> Result<(u32, u32, Vec<u8>), String> {
    // Header: "W H N C" (possibly with trailing comma inside the quotes).
    let is_header = |body: &str| -> bool {
        let toks: Vec<&str> = body.trim_end_matches(['"', ',']).split_whitespace().collect();
        toks.len() >= 4 && toks.iter().take(4).all(|p| p.chars().all(|c| c.is_ascii_digit()))
    };
    let header = text
        .lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix('"').map(|b| (l, b)))
        .find(|(_, b)| is_header(b))
        .ok_or("XPM header not found (\"W H N C\")")?;
    let header = header.0;
    let mut it = header[1..].trim_end_matches(['"', ',']).split_whitespace();
    let w: u32 = it.next().unwrap().parse().map_err(|_| "bad width")?;
    let h: u32 = it.next().unwrap().parse().map_err(|_| "bad height")?;
    let ncolors: usize = it.next().unwrap().parse().map_err(|_| "bad color count")?;
    let cpp: usize = it.next().unwrap().parse().map_err(|_| "bad chars-per-pixel")?;

    let lines = text.lines().map(str::trim);
    // Skip to the first color-map line after the header.
    let mut color_map: Vec<(String, [u8; 4])> = Vec::new();
    let mut rows: Vec<String> = Vec::new();
    let mut in_colors = false;
    for line in lines {
        if line == header {
            in_colors = true;
            continue;
        }
        if !in_colors {
            continue;
        }
        let Some(body) = line.strip_prefix('"') else { continue };
        let Some(body) = body.strip_suffix('"').or_else(|| body.strip_suffix("\",")) else {
            continue;
        };
        if color_map.len() < ncolors {
            // "<chars> c <color>"
            let Some((chars, color)) = body.split_once(" c ") else {
                return Err(format!("malformed XPM color entry: {body:?}"));
            };
            color_map.push((chars.to_string(), parse_color(color.trim_matches('"'))));
        } else {
            rows.push(body.to_string());
            if rows.len() == h as usize {
                break;
            }
        }
    }
    if color_map.len() != ncolors || rows.len() != h as usize {
        return Err(format!(
            "incomplete XPM: {}/{} colors, {}/{} rows",
            color_map.len(),
            ncolors,
            rows.len(),
            h
        ));
    }

    // Map chars -> color.
    let lookup = |cell: &str| -> Result<[u8; 4], String> {
        color_map
            .iter()
            .find(|(chars, _)| chars == cell)
            .map(|(_, c)| *c)
            .ok_or_else(|| format!("unknown XPM color cell {cell:?}"))
    };

    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (y, row) in rows.iter().enumerate() {
        let row = row.trim_end_matches(',');
        for x in 0..w as usize {
            let start = x * cpp;
            let cell = &row[start..start + cpp];
            let c = lookup(cell)?;
            let i = (y * w as usize + x) * 4;
            rgba[i..i + 4].copy_from_slice(&c);
        }
    }
    Ok((w, h, rgba))
}

/// Parse an XPM color value into RGBA.
fn parse_color(s: &str) -> [u8; 4] {
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") {
        return [0, 0, 0, 0];
    }
    if let Some(hex) = s.strip_prefix('#') {
        let (r, g, b) = match hex.len() {
            3 => (
                u8::from_str_radix(&hex[0..1], 16).unwrap_or(0) * 17,
                u8::from_str_radix(&hex[1..2], 16).unwrap_or(0) * 17,
                u8::from_str_radix(&hex[2..3], 16).unwrap_or(0) * 17,
            ),
            6 => (
                u8::from_str_radix(&hex[0..2], 16).unwrap_or(0),
                u8::from_str_radix(&hex[2..4], 16).unwrap_or(0),
                u8::from_str_radix(&hex[4..6], 16).unwrap_or(0),
            ),
            _ => return [0, 0, 0, 255],
        };
        return [r, g, b, 255];
    }
    let low = s.to_ascii_lowercase();
    match low.as_str() {
        "black" => [0, 0, 0, 255],
        "white" => [255, 255, 255, 255],
        "red" => [255, 0, 0, 255],
        "green" => [0, 128, 0, 255],
        "blue" => [0, 0, 255, 255],
        "yellow" => [255, 255, 0, 255],
        "orange" => [255, 165, 0, 255],
        "gray" | "grey" => [128, 128, 128, 255],
        "darkgray" | "darkgrey" => [64, 64, 64, 255],
        "lightgray" | "lightgrey" => [211, 211, 211, 255],
        _ => {
            if let Some(rest) = low
                .strip_prefix("gray")
                .or_else(|| low.strip_prefix("grey"))
                .and_then(|r| r.strip_prefix('(').and_then(|r| r.strip_suffix(')')))
            {
                let v = rest.parse::<u8>().unwrap_or(128);
                [v, v, v, 255]
            } else {
                [0, 0, 0, 255]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONEOK_STYLE: &str = r#"
/* XPM */
static char *oneko[] = {
"3 2 2 1",
"  c None",
"x c #ff0000",
"xxx",
"x x",
};
"#;

    #[test]
    fn parses_simple_xpm() {
        let (w, h, rgba) = parse_xpm(ONEOK_STYLE).expect("parse");
        assert_eq!((w, h), (3, 2));
        // Row 0: all red.
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[8..12], &[255, 0, 0, 255]);
        // Row 1: red, transparent, red.
        assert_eq!(&rgba[12..16], &[255, 0, 0, 255]);
        assert_eq!(&rgba[16..20], &[0, 0, 0, 0]);
        assert_eq!(&rgba[20..24], &[255, 0, 0, 255]);
    }

    #[test]
    fn parses_hex_shorthand_and_none() {
        assert_eq!(parse_color("None"), [0, 0, 0, 0]);
        assert_eq!(parse_color("#f00"), [255, 0, 0, 255]);
        assert_eq!(parse_color("#123456"), [0x12, 0x34, 0x56, 255]);
        assert_eq!(parse_color("gray(17)"), [17, 17, 17, 255]);
    }
}
