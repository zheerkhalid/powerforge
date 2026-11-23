//! Build script: renders the PowerForge icon (vector, via tiny-skia) into a
//! multi-size .ico and a PNG for the window, then embeds the icon, manifest
//! and version info.

#![allow(clippy::field_reassign_with_default)]

use std::{env, fs, path::PathBuf};
use tiny_skia::*;

fn rounded_rect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Path {
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.quad_to(x1, y0, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.quad_to(x1, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.quad_to(x0, y1, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.quad_to(x0, y0, x0 + r, y0);
    pb.close();
    pb.finish().expect("rounded rect")
}

/// The lightning bolt, in 256-unit coordinates, centred on (cx, cy).
fn bolt(cx: f32, cy: f32, scale: f32, s: f32) -> Path {
    let pts = [
        (8.0, -46.0),
        (-26.0, 6.0),
        (-2.0, 6.0),
        (-10.0, 46.0),
        (26.0, -8.0),
        (2.0, -8.0),
    ];
    let mut pb = PathBuilder::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        let (px, py) = ((cx + x * scale) * s, (cy + y * scale) * s);
        if i == 0 {
            pb.move_to(px, py);
        } else {
            pb.line_to(px, py);
        }
    }
    pb.close();
    pb.finish().expect("bolt")
}

fn render_icon(size: u32) -> Vec<u8> {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let s = size as f32 / 256.0;
    let id = Transform::identity();

    // Background: rounded square, deep teal (top left) to sun yellow (bottom right).
    let (x0, y0, x1, y1) = (8.0 * s, 8.0 * s, 248.0 * s, 248.0 * s);
    let bg = rounded_rect(x0, y0, x1, y1, 56.0 * s);
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.shader = LinearGradient::new(
        Point::from_xy(x0, y0),
        Point::from_xy(x1, y1),
        vec![
            GradientStop::new(0.0, Color::from_rgba8(0x0a, 0x5c, 0x6e, 255)),
            GradientStop::new(0.5, Color::from_rgba8(0x1f, 0x9a, 0x86, 255)),
            GradientStop::new(1.0, Color::from_rgba8(0xf9, 0xc4, 0x40, 255)),
        ],
        SpreadMode::Pad,
        id,
    )
    .expect("gradient");
    pm.fill_path(&bg, &paint, FillRule::Winding, id, None);

    let mut white = Paint::default();
    white.anti_alias = true;
    white.set_color_rgba8(255, 255, 255, 255);

    // A battery lying on its side: an outline, a terminal nub, and a bolt.
    let small = size < 32;
    let (bx0, by0, bx1, by1, wall) = if small {
        (30.0, 70.0, 206.0, 186.0, 26.0)
    } else {
        (40.0, 80.0, 200.0, 176.0, 16.0)
    };
    let stroke = Stroke {
        width: wall * s,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let body = rounded_rect(bx0 * s, by0 * s, bx1 * s, by1 * s, 22.0 * s);
    pm.stroke_path(&body, &white, &stroke, id, None);
    let nub_h = (by1 - by0) * 0.38;
    let cy = (by0 + by1) / 2.0;
    let nub = rounded_rect(
        (bx1 + wall * 0.5 + 4.0) * s,
        (cy - nub_h / 2.0) * s,
        (bx1 + wall * 0.5 + 22.0) * s,
        (cy + nub_h / 2.0) * s,
        6.0 * s,
    );
    pm.fill_path(&nub, &white, FillRule::Winding, id, None);
    let cx = (bx0 + bx1) / 2.0;
    let scale = if small { 1.05 } else { 0.92 };
    pm.fill_path(&bolt(cx, cy, scale, s), &white, FillRule::Winding, id, None);

    pm.encode_png().expect("png")
}

fn write_ico(path: &PathBuf, sizes: &[u32]) {
    let pngs: Vec<Vec<u8>> = sizes.iter().map(|&s| render_icon(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (i, &s) in sizes.iter().enumerate() {
        let b = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[b, b, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(pngs[i].len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += pngs[i].len() as u32;
    }
    for p in &pngs {
        out.extend_from_slice(p);
    }
    fs::write(path, out).expect("write ico");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("powerforge.ico");
    write_ico(&ico, &[16, 20, 24, 32, 40, 48, 64, 256]);
    let png = render_icon(256);
    fs::write(out.join("icon-256.png"), &png).expect("write png");
    let docs_png = PathBuf::from("docs/assets/powerforge-icon.png");
    if fs::read(&docs_png).ok().as_deref() != Some(&png[..]) {
        let _ = fs::create_dir_all("docs/assets");
        let _ = fs::write(&docs_png, &png);
    }

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ver = env::var("CARGO_PKG_VERSION").expect("version");
    let parts: Vec<u16> = ver
        .split(['.', '-'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    let (ma, mi, pa) = (
        parts[0],
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    );
    let ver4 = format!("{ma}.{mi}.{pa}.0");
    println!("cargo:rerun-if-changed=res/powerforge.manifest.in");
    let manifest = fs::read_to_string("res/powerforge.manifest.in")
        .expect("manifest")
        .replace("@VERSION4@", &ver4);
    let manifest_path = out.join("powerforge.manifest");
    fs::write(&manifest_path, manifest).expect("write manifest");
    let esc = |p: &PathBuf| p.display().to_string().replace('\\', "\\\\");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{ico}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {ma},{mi},{pa},0
PRODUCTVERSION {ma},{mi},{pa},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Zheer Khalid (zheer.dev)"
      VALUE "FileDescription", "PowerForge power and battery tool"
      VALUE "FileVersion", "{ver}"
      VALUE "InternalName", "PowerForge"
      VALUE "LegalCopyright", "Copyright (c) 2026 Zheer Khalid. MIT License."
      VALUE "OriginalFilename", "PowerForge.exe"
      VALUE "ProductName", "PowerForge"
      VALUE "ProductVersion", "{ver}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        ico = esc(&ico),
        manifest = esc(&manifest_path),
    );
    let rc_path = out.join("powerforge.rc");
    fs::write(&rc_path, rc).expect("write rc");
    embed_resource::compile(&rc_path, embed_resource::NONE);
    println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
}
