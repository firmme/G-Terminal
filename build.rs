//! Embeds the application icon in the executable.
//!
//! The `.ico` is generated from the same pixel code the window icon uses, so
//! there is one definition of the mark, then compiled to a resource by the
//! Windows SDK's `rc.exe`. No build dependency is involved: `winresource` and
//! friends would have to shell out to a resource compiler anyway, and one that
//! the GNU toolchain does not ship would turn a cosmetic asset into a build
//! failure.
//!
//! Everything here is best-effort. A missing or unusable resource compiler
//! produces a warning and an executable without a file icon, never a failed
//! build — the mingw toolchain is expected to work too.

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "src/appicon.rs"]
mod appicon;

/// The sizes Explorer picks between: 16 for the list view, 256 for large tiles.
const SIZES: [u32; 5] = [16, 24, 32, 48, 256];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/appicon.rs");

    // Only the MSVC linker is handed the resource. mingw wants a windres object
    // rather than a `.res`, and going without a file icon beats failing to build.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    match embed(&out) {
        Ok(resource) => println!("cargo:rustc-link-arg-bins={}", resource.display()),
        Err(e) => println!("cargo:warning=no exe icon: {e}"),
    }
}

fn embed(out: &Path) -> Result<PathBuf, String> {
    std::fs::write(out.join("g-terminal.ico"), ico_bytes()).map_err(|e| e.to_string())?;
    let script = out.join("g-terminal.rc");
    std::fs::write(&script, "1 ICON \"g-terminal.ico\"\n").map_err(|e| e.to_string())?;

    let rc = find_rc().ok_or("rc.exe not found in the Windows Kits")?;
    let resource = out.join("g-terminal.res");
    let status = Command::new(&rc)
        .current_dir(out)
        .arg("/nologo")
        .arg(format!("/fo{}", resource.display()))
        .arg(&script)
        .status()
        .map_err(|e| format!("could not run {}: {e}", rc.display()))?;
    if !status.success() {
        return Err(format!("rc.exe exited with {status}"));
    }
    Ok(resource)
}

/// The SDK's resource compiler. Found by walking the Windows Kits the same way
/// the build script does, because `rc.exe` is not on PATH for a plain
/// `cargo build`.
fn find_rc() -> Option<PathBuf> {
    let mut roots = Vec::new();
    for key in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Ok(root) = std::env::var(key) {
            roots.push(PathBuf::from(root).join("Windows Kits/10/bin"));
        }
    }
    let mut candidates = Vec::new();
    for root in roots {
        let Ok(versions) = std::fs::read_dir(&root) else {
            continue;
        };
        for version in versions.flatten() {
            let candidate = version.path().join("x64").join("rc.exe");
            if candidate.is_file() {
                candidates.push(candidate);
            }
        }
    }
    // The newest SDK, matching what the linker will end up using.
    candidates.sort();
    candidates.pop()
}

/// A multi-size ICO in the uncompressed BMP form, which every Windows version
/// reads without needing a PNG decoder.
fn ico_bytes() -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = SIZES.iter().map(|&size| (size, bmp_image(size))).collect();
    let mut out = Vec::new();
    // ICONDIR
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    // One ICONDIRENTRY per image, then the images themselves.
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, image) in &images {
        // 256 is encoded as zero; the field is a single byte.
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.push(dim);
        out.push(dim);
        out.push(0); // palette size
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for (_, image) in &images {
        out.extend_from_slice(image);
    }
    out
}

/// One image in the BMP form an ICO entry expects: a `BITMAPINFOHEADER` with a
/// doubled height, bottom-up BGRA pixels, then a zeroed AND mask.
fn bmp_image(size: u32) -> Vec<u8> {
    let rgba = appicon::pixels(size);
    let side = size as usize;
    let stride = side * 4;
    let mut out = Vec::with_capacity(40 + stride * side + mask_len(size) * side);
    let dword = |out: &mut Vec<u8>, value: u32| out.extend_from_slice(&value.to_le_bytes());
    dword(&mut out, 40); // header size
    dword(&mut out, size); // width
    dword(&mut out, size * 2); // height: colour bitmap plus mask
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    dword(&mut out, 0); // uncompressed
    dword(&mut out, (stride * side) as u32);
    for _ in 0..4 {
        dword(&mut out, 0); // resolution and palette counts
    }
    // Bottom-up, and BGRA rather than RGBA.
    for row in (0..side).rev() {
        for column in 0..side {
            let i = (row * side + column) * 4;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    // The AND mask is all zero: the alpha channel already carries the shape.
    out.resize(out.len() + mask_len(size) * side, 0);
    out
}

/// Rows in an ICO's AND mask are padded to four bytes.
fn mask_len(size: u32) -> usize {
    (size as usize).div_ceil(32) * 4
}
