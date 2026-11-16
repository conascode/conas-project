//! v1.1 SVG icons (note.html) rasterised with 4×4 supersampling into premultiplied
//! 32-bpp bitmaps, cached per (icon, size, colour, variant) and drawn with AlphaBlend.
//! Report §6.6: "DPI별 32bpp 프리멀티플라이드 비트맵, AlphaBlend".

use crate::win::px;
use std::cell::RefCell;
use std::collections::HashMap;
use windows_sys::Win32::Graphics::Gdi::*;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Pin,
    New,
    More,
    Restore,
    Close,
    Lock,
    Check,
    SwatchCheck,
    Bullets,
    Image,
}

enum Shape {
    /// points, closed, stroke width, round caps
    Line(&'static [(f32, f32)], bool, f32, bool),
    Fill(&'static [(f32, f32)]),
    Ring(f32, f32, f32, f32),
    Disc(f32, f32, f32),
}

const PIN_HEAD: &[(f32, f32)] = &[
    (8.2, 2.2), (11.8, 5.8), (10.0, 6.4), (8.1, 8.3), (8.3, 10.2), (7.4, 11.1), (3.9, 6.6), (4.8, 5.7), (6.7, 5.9), (8.6, 4.0), (8.2, 2.2),
];
const LOCK_BODY: &[(f32, f32)] = &[(3.0, 7.2), (3.3, 6.5), (4.0, 6.2), (10.0, 6.2), (10.7, 6.5), (11.0, 7.2), (11.0, 10.8), (10.7, 11.5), (10.0, 11.8), (4.0, 11.8), (3.3, 11.5), (3.0, 10.8), (3.0, 7.2)];
const LOCK_SHACKLE: &[(f32, f32)] = &[
    (4.8, 6.2), (4.8, 4.6), (4.95, 3.8), (5.44, 3.04), (6.2, 2.55), (7.0, 2.4), (7.8, 2.55), (8.56, 3.04), (9.05, 3.8), (9.2, 4.6), (9.2, 6.2),
];
const IMG_FRAME: &[(f32, f32)] = &[(2.0, 4.0), (2.3, 3.3), (3.0, 3.0), (11.0, 3.0), (11.7, 3.3), (12.0, 4.0), (12.0, 10.0), (11.7, 10.7), (11.0, 11.0), (3.0, 11.0), (2.3, 10.7), (2.0, 10.0), (2.0, 4.0)];

fn shapes(icon: Icon, filled: bool) -> (f32, Vec<Shape>) {
    use Shape::*;
    match icon {
        Icon::Pin => {
            let mut v = vec![Line(PIN_HEAD, true, 1.3, true), Line(&[(5.6, 8.4), (2.5, 11.5)], false, 1.3, true)];
            if filled {
                v.push(Fill(PIN_HEAD));
            }
            (14.0, v)
        }
        Icon::New => (14.0, vec![Line(&[(7.0, 2.5), (7.0, 11.5)], false, 1.5, false), Line(&[(2.5, 7.0), (11.5, 7.0)], false, 1.5, false)]),
        Icon::More => (
            14.0,
            vec![
                Fill(&[(2.0, 6.0), (4.0, 6.0), (4.0, 8.0), (2.0, 8.0)]),
                Fill(&[(6.0, 6.0), (8.0, 6.0), (8.0, 8.0), (6.0, 8.0)]),
                Fill(&[(10.0, 6.0), (12.0, 6.0), (12.0, 8.0), (10.0, 8.0)]),
            ],
        ),
        Icon::Restore => (
            14.0,
            vec![
                Line(&[(2.5, 2.5), (5.7, 5.7)], false, 1.3, true),
                Line(&[(5.7, 2.8), (5.7, 5.7), (2.8, 5.7)], false, 1.3, true),
                Line(&[(11.5, 11.5), (8.3, 8.3)], false, 1.3, true),
                Line(&[(8.3, 11.2), (8.3, 8.3), (11.2, 8.3)], false, 1.3, true),
            ],
        ),
        Icon::Close => (14.0, vec![Line(&[(3.5, 3.5), (10.5, 10.5)], false, 1.5, false), Line(&[(10.5, 3.5), (3.5, 10.5)], false, 1.5, false)]),
        Icon::Lock => (14.0, vec![Line(LOCK_BODY, true, 1.3, true), Line(LOCK_SHACKLE, false, 1.3, true)]),
        Icon::Check => (14.0, vec![Line(&[(3.0, 7.4), (5.6, 10.0), (11.0, 4.4)], false, 1.3, true)]),
        Icon::SwatchCheck => (10.0, vec![Line(&[(2.0, 5.2), (4.0, 7.2), (8.0, 2.8)], false, 2.0, false)]),
        Icon::Bullets => (
            14.0,
            vec![
                Line(&[(5.5, 3.5), (11.5, 3.5)], false, 1.3, true),
                Line(&[(5.5, 7.0), (11.5, 7.0)], false, 1.3, true),
                Line(&[(5.5, 10.5), (11.5, 10.5)], false, 1.3, true),
                Disc(2.6, 3.5, 0.9),
                Disc(2.6, 7.0, 0.9),
                Disc(2.6, 10.5, 0.9),
            ],
        ),
        Icon::Image => (
            14.0,
            vec![
                Line(IMG_FRAME, true, 1.3, true),
                Ring(5.0, 5.8, 1.0, 1.3),
                Line(&[(2.6, 10.4), (5.6, 7.4), (7.6, 9.4), (9.0, 8.0), (11.4, 10.4)], false, 1.3, true),
            ],
        ),
    }
}

fn seg_cov(px_: f32, py: f32, a: (f32, f32), b: (f32, f32), hw: f32, round: bool) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-6 {
        return (px_ - a.0).hypot(py - a.1) <= hw;
    }
    let t = ((px_ - a.0) * dx + (py - a.1) * dy) / len2;
    if round {
        let t = t.clamp(0.0, 1.0);
        (px_ - (a.0 + t * dx)).hypot(py - (a.1 + t * dy)) <= hw
    } else {
        // square cap: extend by half width along the segment
        let ext = hw / len2.sqrt();
        if t < -ext || t > 1.0 + ext {
            return false;
        }
        let d = ((px_ - a.0) * dy - (py - a.1) * dx).abs() / len2.sqrt();
        d <= hw
    }
}

fn inside(px_: f32, py: f32, poly: &[(f32, f32)]) -> bool {
    let mut c = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > py) != (yj > py) && px_ < (xj - xi) * (py - yi) / (yj - yi) + xi {
            c = !c;
        }
        j = i;
    }
    c
}

fn coverage(icon: Icon, filled: bool, size: i32) -> Vec<u8> {
    let (vb, shapes) = shapes(icon, filled);
    let ss = 4;
    let n = size as usize;
    let mut out = vec![0u8; n * n];
    let scale = vb / size as f32;
    for y in 0..n {
        for x in 0..n {
            let mut hits = 0;
            for sy in 0..ss {
                for sx in 0..ss {
                    let fx = (x as f32 + (sx as f32 + 0.5) / ss as f32) * scale;
                    let fy = (y as f32 + (sy as f32 + 0.5) / ss as f32) * scale;
                    let hit = shapes.iter().any(|s| match s {
                        Shape::Line(pts, closed, w, round) => {
                            let hw = w / 2.0;
                            let mut any = pts.windows(2).any(|p| seg_cov(fx, fy, p[0], p[1], hw, *round));
                            if *closed && !any {
                                any = seg_cov(fx, fy, pts[pts.len() - 1], pts[0], hw, *round);
                            }
                            any
                        }
                        Shape::Fill(pts) => inside(fx, fy, pts),
                        Shape::Ring(cx, cy, r, w) => ((fx - cx).hypot(fy - cy) - r).abs() <= w / 2.0,
                        Shape::Disc(cx, cy, r) => (fx - cx).hypot(fy - cy) <= *r,
                    });
                    hits += hit as u32;
                }
            }
            out[y * n + x] = (hits * 255 / (ss * ss) as u32) as u8;
        }
    }
    out
}

thread_local! {
    static CACHE: RefCell<HashMap<(Icon, bool, i32, u32), HBITMAP>> = RefCell::new(HashMap::new());
}

fn bitmap(icon: Icon, filled: bool, size: i32, color: u32) -> HBITMAP {
    CACHE.with(|c| {
        *c.borrow_mut().entry((icon, filled, size, color)).or_insert_with(|| unsafe {
            let cov = coverage(icon, filled, size);
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = size;
            bi.bmiHeader.biHeight = -size;
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(std::ptr::null_mut(), &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            let px_ = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
            let (r, g, b) = (color & 0xFF, (color >> 8) & 0xFF, (color >> 16) & 0xFF);
            for (i, &a) in cov.iter().enumerate() {
                let a = a as u32;
                px_[i] = (a << 24) | ((r * a / 255) << 16) | ((g * a / 255) << 8) | (b * a / 255);
            }
            bmp
        })
    })
}

/// Draws `icon` with its top-left at (x, y), `css` logical px square, in `color`.
pub fn draw(hdc: HDC, icon: Icon, x: i32, y: i32, css: f32, dpi: u32, color: u32, filled: bool) {
    let size = px(css, dpi).max(1);
    unsafe {
        let bmp = bitmap(icon, filled, size, color);
        let mem = CreateCompatibleDC(hdc);
        let old = SelectObject(mem, bmp);
        let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        GdiAlphaBlend(hdc, x, y, size, size, mem, 0, 0, size, size, bf);
        SelectObject(mem, old);
        DeleteDC(mem);
    }
}

/// Draws `icon` centred in the rectangle.
pub fn draw_centered(hdc: HDC, icon: Icon, r: &windows_sys::Win32::Foundation::RECT, css: f32, dpi: u32, color: u32, filled: bool) {
    let size = px(css, dpi);
    draw(hdc, icon, (r.left + r.right - size) / 2, (r.top + r.bottom - size) / 2, css, dpi, color, filled);
}
