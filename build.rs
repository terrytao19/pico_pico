//! This build script does two things:
//!
//! 1. Copies `memory.x` from the crate root into `OUT_DIR` so the linker
//!    can always find it, and sets up the required linker arguments for
//!    the RP2040 (memory layout, defmt, no-magic linking). This part is
//!    needed regardless of sprite sheets -- it's how the firmware links
//!    and how defmt log output works at all.
//!
//! 2. Flattens every `.bmp` in `src/sheets/` (which may have a raw alpha
//!    channel, e.g. straight from Aseprite's BMP export) into a plain
//!    24-bit BI_RGB BMP in `OUT_DIR/sheets/`, since `tinybmp` can't parse
//!    32-bit BMPs with a nonzero alpha mask. Rust code then does
//!    `include_bytes!(concat!(env!("OUT_DIR"), "/sheets/xxx.bmp"))`.

use std::env;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    // --- memory.x / linker setup ---

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(include_bytes!("memory.x"))
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tlink-rp.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");

    // --- sprite sheet flattening ---

    flatten_sprite_sheets(&out);
}

fn flatten_sprite_sheets(out_dir: &Path) {
    let sheets_dir = Path::new("src/sheets");
    let out_sheets_dir = out_dir.join("sheets");
    fs::create_dir_all(&out_sheets_dir).unwrap();

    let entries = fs::read_dir(sheets_dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", sheets_dir.display(), e));

    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("bmp") {
            continue;
        }

        println!("cargo:rerun-if-changed={}", path.display());

        let img = image::open(&path)
            .unwrap_or_else(|e| panic!("failed to decode {}: {}", path.display(), e))
            .to_rgba8();

        // Flatten alpha onto black, matching lcd.clear()'s default background,
        // so "transparent" areas blend seamlessly on-screen.
        let (w, h) = img.dimensions();
        let mut flattened = image::RgbImage::new(w, h);
        for (x, y, px) in img.enumerate_pixels() {
            let [r, g, b, a] = px.0;
            let a = a as u32;
            let blend = |c: u8| ((c as u32 * a) / 255) as u8;
            flattened.put_pixel(x, y, image::Rgb([blend(r), blend(g), blend(b)]));
        }

        let out_path = out_sheets_dir.join(path.file_name().unwrap());
        flattened
            .save_with_format(&out_path, image::ImageFormat::Bmp)
            .unwrap_or_else(|e| panic!("failed to write {}: {}", out_path.display(), e));
    }

    println!("cargo:rerun-if-changed=src/sheets");
}