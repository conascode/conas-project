//! v1.1 design tokens (ui/common.css, report table 3-4) and note geometry (table 3-5).

use crate::win::rgb;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn toggled(self) -> Theme {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Band {
    None,
    Thin,
    Full,
}

impl Band {
    /// The ⋯ menu row cycles 없음 → 얇은 띠 → 제목 줄.
    pub fn next(self) -> Band {
        match self {
            Band::None => Band::Thin,
            Band::Thin => Band::Full,
            Band::Full => Band::None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Band::None => "없음",
            Band::Thin => "얇은 띠",
            Band::Full => "제목 줄",
        }
    }
    /// Height of the coloured strip above the title bar (logical px).
    pub fn strip(self) -> f32 {
        if self == Band::Thin {
            4.0
        } else {
            0.0
        }
    }
}

pub struct Pal {
    pub paper: u32,
    pub paper2: u32,
    pub paper3: u32,
    pub rule: u32,
    pub ink: u32,
    pub muted: u32,
    pub strong: u32,
    pub on_strong: u32,
}

const DARK: Pal = Pal {
    paper: rgb(0x20, 0x20, 0x20),
    paper2: rgb(0x2A, 0x2A, 0x2A),
    paper3: rgb(0x35, 0x35, 0x35),
    rule: rgb(0x47, 0x47, 0x47),
    ink: rgb(0xF0, 0xF0, 0xF0),
    muted: rgb(0xA6, 0xA6, 0xA6),
    strong: rgb(0xF0, 0xF0, 0xF0),
    on_strong: rgb(0x1A, 0x1A, 0x1A),
};

const LIGHT: Pal = Pal {
    paper: rgb(0xFF, 0xFF, 0xFF),
    paper2: rgb(0xF5, 0xF5, 0xF3),
    paper3: rgb(0xEB, 0xEB, 0xE8),
    rule: rgb(0xDD, 0xDD, 0xD8),
    ink: rgb(0x22, 0x22, 0x20),
    muted: rgb(0x6B, 0x6B, 0x67),
    strong: rgb(0x1A, 0x1A, 0x18),
    on_strong: rgb(0xFF, 0xFF, 0xFF),
};

pub fn pal(t: Theme) -> &'static Pal {
    match t {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

pub const EDGE: u32 = rgb(0, 0, 0);
pub const fn rgb_white() -> u32 {
    rgb(0xFF, 0xFF, 0xFF)
}
pub const ON_ACCENT: u32 = rgb(0x22, 0x22, 0x20);
pub const ON_ACCENT_SOFT: u32 = rgb(0x4A, 0x4A, 0x46);
pub const SCRIM: f32 = 0.78;

pub const PALETTE: [u32; 6] = [
    rgb(0xAF, 0xA9, 0xEC),
    rgb(0xF4, 0xC0, 0xD1),
    rgb(0x9F, 0xE1, 0xCB),
    rgb(0xF6, 0xC9, 0xA0),
    rgb(0xB5, 0xD4, 0xF4),
    rgb(0xF3, 0xE9, 0xC6),
];
pub const COLOR_NAMES: [&str; 6] = ["라벤더", "로즈", "민트", "살구", "하늘", "크림"];

/// CSS `color-mix(in srgb, a t, b)`.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| -> u32 {
        let x = ((a >> s) & 0xFF) as f32 * t + ((b >> s) & 0xFF) as f32 * (1.0 - t);
        (x.round().clamp(0.0, 255.0) as u32) << s
    };
    ch(0) | ch(8) | ch(16)
}

/// Note window geometry, logical px (note.css / common.css / model.rs).
pub mod g {
    pub const BAR: f32 = 34.0;
    pub const BAR_PAD_L: f32 = 12.0;
    pub const BAR_PAD_R: f32 = 5.0;
    pub const BAR_GAP: f32 = 4.0;
    pub const TOOL: f32 = 24.0;
    pub const TOOL_GAP: f32 = 1.0;
    pub const ICON: f32 = 14.0;
    pub const FMT_PAD_Y: f32 = 3.0;
    pub const FMT_PAD_X: f32 = 8.0;
    pub const FMT_BTN_W: f32 = 28.0;
    pub const FMT_BTN_H: f32 = 26.0;
    pub const FMT_SEP_M: f32 = 5.0;
    pub const NOTE_W: f32 = 260.0;
    pub const NOTE_H: f32 = 260.0;
    pub const MIN_W: f32 = 180.0;
    pub const MIN_H: f32 = 130.0;
    pub const FONT_MIN: u8 = 10;
    pub const FONT_MAX: u8 = 32;
    pub const FONT_DEFAULT: u8 = 14;
    // ⋯ menu (common.css .menu / .menu-item / .swatches)
    pub const MENU_MIN_W: f32 = 156.0;
    pub const MENU_PAD_Y: f32 = 4.0;
    pub const ITEM_H: f32 = 28.0;
    pub const ITEM_H_COMPACT: f32 = 20.0;
    pub const ITEM_PAD_X: f32 = 12.0;
    pub const SWATCH: f32 = 16.0;
    pub const SWATCH_GAP: f32 = 6.0;
    // confirm card
    pub const CARD_MAX_W: f32 = 220.0;
    // list window (list.css)
    pub const LIST_W: f32 = 280.0;
    pub const LIST_H: f32 = 380.0;
    pub const LIST_MIN_W: f32 = 220.0;
    pub const LIST_MIN_H: f32 = 240.0;
    pub const LIST_BAR: f32 = 40.0;
    pub const SEARCH_H: f32 = 30.0;
}
