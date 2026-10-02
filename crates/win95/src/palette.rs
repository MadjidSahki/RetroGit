//! Color schemes: every color the interface paints, named by its role (as in the Windows 95
//! Appearance settings), in six schemes.

use egui::Color32;

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// All the colors of one scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Buttons, panels, dialogs ("3D objects").
    pub face: Color32,
    /// Wells: lists, text fields, diffs.
    pub window: Color32,
    /// 3D edges, from the lightest to the darkest.
    pub highlight: Color32,
    pub light: Color32,
    pub shadow: Color32,
    pub dark_shadow: Color32,
    /// Text on `face`.
    pub text: Color32,
    /// Text on `window`.
    pub window_text: Color32,
    /// Disabled text.
    pub gray_text: Color32,
    pub selection: Color32,
    pub selection_text: Color32,
    /// Active title bar gradient, and its text.
    pub title: Color32,
    pub title_end: Color32,
    pub title_text: Color32,
    pub inactive_title: Color32,
    pub inactive_title_end: Color32,
    pub inactive_title_text: Color32,
    /// Status colors, used as text.
    pub link: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub success: Color32,
    pub merged: Color32,
    /// Backgrounds behind `window_text`.
    pub note_bg: Color32,
    pub comment_bg: Color32,
    pub pending_bg: Color32,
    pub line_selected: Color32,
    pub added: Color32,
    pub removed: Color32,
    pub hunk: Color32,
    pub conflict_block: Color32,
    pub conflict_current: Color32,
    pub code_bg: Color32,
    /// History graph lanes.
    pub lanes: [Color32; 8],
    /// Ref labels in History (text: `window_text`).
    pub ref_head: Color32,
    pub ref_branch: Color32,
    pub ref_remote: Color32,
    pub ref_tag: Color32,
    /// Dark scheme: syntax highlighting uses a dark theme.
    pub dark: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Scheme {
    #[default]
    Standard,
    Dark,
    HighContrastBlack,
    HighContrastWhite,
    Slate,
    RainyDay,
}

impl Scheme {
    pub const ALL: [Scheme; 6] = [
        Scheme::Standard,
        Scheme::Dark,
        Scheme::HighContrastBlack,
        Scheme::HighContrastWhite,
        Scheme::Slate,
        Scheme::RainyDay,
    ];

    /// Name shown to the user and stored in the configuration.
    pub fn name(self) -> &'static str {
        match self {
            Scheme::Standard => "Windows Standard",
            Scheme::Dark => "Dark",
            Scheme::HighContrastBlack => "High Contrast Black",
            Scheme::HighContrastWhite => "High Contrast White",
            Scheme::Slate => "Slate",
            Scheme::RainyDay => "Rainy Day",
        }
    }

    pub fn from_name(name: &str) -> Option<Scheme> {
        Scheme::ALL.into_iter().find(|s| s.name() == name)
    }

    pub fn palette(self) -> Palette {
        match self {
            Scheme::Standard => STANDARD,
            Scheme::Dark => DARK,
            Scheme::HighContrastBlack => HIGH_CONTRAST_BLACK,
            Scheme::HighContrastWhite => HIGH_CONTRAST_WHITE,
            Scheme::Slate => SLATE,
            Scheme::RainyDay => RAINY_DAY,
        }
    }
}

/// WCAG contrast ratio between two colors (1 to 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    fn lum(c: Color32) -> f32 {
        let ch = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Light schemes share these backgrounds and status colors (a light `window`).
const LIGHT_LANES: [Color32; 8] = [
    rgb(0x000080),
    rgb(0x800000),
    rgb(0x008000),
    rgb(0x800080),
    rgb(0x008080),
    rgb(0x808000),
    rgb(0xC04000),
    rgb(0x404040),
];

pub const STANDARD: Palette = Palette {
    face: rgb(0xC0C0C0),
    window: rgb(0xFFFFFF),
    highlight: rgb(0xFFFFFF),
    light: rgb(0xDFDFDF),
    shadow: rgb(0x808080),
    dark_shadow: rgb(0x000000),
    text: rgb(0x000000),
    window_text: rgb(0x000000),
    gray_text: rgb(0x808080),
    selection: rgb(0x000080),
    selection_text: rgb(0xFFFFFF),
    title: rgb(0x000080),
    title_end: rgb(0x1084D0),
    title_text: rgb(0xFFFFFF),
    inactive_title: rgb(0x808080),
    inactive_title_end: rgb(0xB5B5B5),
    inactive_title_text: rgb(0xC0C0C0),
    link: rgb(0x000080),
    error: rgb(0xA00000),
    warning: rgb(0x805000),
    success: rgb(0x006000),
    merged: rgb(0x602090),
    note_bg: rgb(0xFFFFC0),
    comment_bg: rgb(0xFFFFE8),
    pending_bg: rgb(0xFFF0C0),
    line_selected: rgb(0xD0E0FF),
    added: rgb(0xE6FFE6),
    removed: rgb(0xFFE6E6),
    hunk: rgb(0xE0E0F0),
    conflict_block: rgb(0xFFF6C8),
    conflict_current: rgb(0xFFD890),
    code_bg: rgb(0xF0F0F0),
    lanes: LIGHT_LANES,
    ref_head: rgb(0xFFFF80),
    ref_branch: rgb(0xC0FFC0),
    ref_remote: rgb(0xC0D8FF),
    ref_tag: rgb(0xFFD8A0),
    dark: false,
};

pub const DARK: Palette = Palette {
    face: rgb(0x3A3A3A),
    window: rgb(0x1E1E1E),
    highlight: rgb(0x6A6A6A),
    light: rgb(0x4A4A4A),
    shadow: rgb(0x262626),
    dark_shadow: rgb(0x101010),
    text: rgb(0xE0E0E0),
    window_text: rgb(0xE0E0E0),
    gray_text: rgb(0x8A8A8A),
    selection: rgb(0x2D5FA0),
    selection_text: rgb(0xFFFFFF),
    title: rgb(0x000060),
    title_end: rgb(0x205090),
    title_text: rgb(0xFFFFFF),
    inactive_title: rgb(0x404040),
    inactive_title_end: rgb(0x5A5A5A),
    inactive_title_text: rgb(0xA0A0A0),
    link: rgb(0x8AB4F8),
    error: rgb(0xFF7B7B),
    warning: rgb(0xE0B050),
    success: rgb(0x6CCB6C),
    merged: rgb(0xC090F0),
    note_bg: rgb(0x4A4520),
    comment_bg: rgb(0x2E2C22),
    pending_bg: rgb(0x4A3A10),
    line_selected: rgb(0x2A3A5A),
    added: rgb(0x1E3A24),
    removed: rgb(0x4A2226),
    hunk: rgb(0x2A2A40),
    conflict_block: rgb(0x3E3820),
    conflict_current: rgb(0x5A4520),
    code_bg: rgb(0x2A2A2A),
    lanes: [
        rgb(0x6A9FFF),
        rgb(0xFF7A7A),
        rgb(0x6CCB6C),
        rgb(0xD08AFF),
        rgb(0x4FD0D0),
        rgb(0xD0C050),
        rgb(0xFF9A40),
        rgb(0xB0B0B0),
    ],
    ref_head: rgb(0x5A5418),
    ref_branch: rgb(0x2A4A2A),
    ref_remote: rgb(0x26405E),
    ref_tag: rgb(0x5A3E18),
    dark: true,
};

pub const HIGH_CONTRAST_BLACK: Palette = Palette {
    face: rgb(0x000000),
    window: rgb(0x000000),
    highlight: rgb(0xFFFFFF),
    light: rgb(0x808080),
    shadow: rgb(0x808080),
    dark_shadow: rgb(0xFFFFFF),
    text: rgb(0xFFFFFF),
    window_text: rgb(0xFFFFFF),
    gray_text: rgb(0x00FF00),
    selection: rgb(0x800080),
    selection_text: rgb(0xFFFFFF),
    title: rgb(0x800080),
    title_end: rgb(0x800080),
    title_text: rgb(0xFFFFFF),
    inactive_title: rgb(0x008000),
    inactive_title_end: rgb(0x008000),
    inactive_title_text: rgb(0xFFFFFF),
    link: rgb(0xFFFF00),
    error: rgb(0xFF8080),
    warning: rgb(0xFFFF00),
    success: rgb(0x00FF00),
    merged: rgb(0xFF80FF),
    note_bg: rgb(0x333300),
    comment_bg: rgb(0x1A1A1A),
    pending_bg: rgb(0x4D3300),
    line_selected: rgb(0x000080),
    added: rgb(0x003300),
    removed: rgb(0x4D0000),
    hunk: rgb(0x000066),
    conflict_block: rgb(0x333300),
    conflict_current: rgb(0x4D3300),
    code_bg: rgb(0x1A1A1A),
    lanes: [
        rgb(0x00FFFF),
        rgb(0xFFFF00),
        rgb(0x00FF00),
        rgb(0xFF00FF),
        rgb(0xFF8000),
        rgb(0x8080FF),
        rgb(0xFF8080),
        rgb(0xFFFFFF),
    ],
    ref_head: rgb(0x666600),
    ref_branch: rgb(0x006600),
    ref_remote: rgb(0x000099),
    ref_tag: rgb(0x663300),
    dark: true,
};

pub const HIGH_CONTRAST_WHITE: Palette = Palette {
    face: rgb(0xFFFFFF),
    window: rgb(0xFFFFFF),
    highlight: rgb(0x000000),
    light: rgb(0xFFFFFF),
    shadow: rgb(0x808080),
    dark_shadow: rgb(0x000000),
    text: rgb(0x000000),
    window_text: rgb(0x000000),
    gray_text: rgb(0x6B6B6B),
    selection: rgb(0x000000),
    selection_text: rgb(0xFFFFFF),
    title: rgb(0x000000),
    title_end: rgb(0x000000),
    title_text: rgb(0xFFFFFF),
    inactive_title: rgb(0xFFFFFF),
    inactive_title_end: rgb(0xFFFFFF),
    inactive_title_text: rgb(0x000000),
    link: rgb(0x0000C0),
    error: rgb(0xA00000),
    warning: rgb(0x704800),
    success: rgb(0x006000),
    merged: rgb(0x602090),
    note_bg: rgb(0xFFFFE0),
    comment_bg: rgb(0xFFFFFF),
    pending_bg: rgb(0xFFF0C0),
    line_selected: rgb(0xC0D8FF),
    added: rgb(0xD8FFD8),
    removed: rgb(0xFFD8D8),
    hunk: rgb(0xE0E0FF),
    conflict_block: rgb(0xFFF6C8),
    conflict_current: rgb(0xFFD890),
    code_bg: rgb(0xF0F0F0),
    lanes: [
        rgb(0x000080),
        rgb(0x800000),
        rgb(0x006000),
        rgb(0x800080),
        rgb(0x006060),
        rgb(0x606000),
        rgb(0xA03000),
        rgb(0x000000),
    ],
    ref_head: rgb(0xFFFF80),
    ref_branch: rgb(0xC0FFC0),
    ref_remote: rgb(0xC0D8FF),
    ref_tag: rgb(0xFFD8A0),
    dark: false,
};

pub const SLATE: Palette = Palette {
    face: rgb(0xA0B0C0),
    window: rgb(0xF4F7FA),
    highlight: rgb(0xE0E8F0),
    light: rgb(0xC0CCD8),
    shadow: rgb(0x607080),
    dark_shadow: rgb(0x202830),
    text: rgb(0x000000),
    window_text: rgb(0x000000),
    gray_text: rgb(0x505A68),
    selection: rgb(0x405870),
    selection_text: rgb(0xFFFFFF),
    title: rgb(0x405870),
    title_end: rgb(0x7890A8),
    title_text: rgb(0xFFFFFF),
    inactive_title: rgb(0x8090A0),
    inactive_title_end: rgb(0xA8B4C0),
    inactive_title_text: rgb(0xD8E0E8),
    link: rgb(0x203C78),
    error: rgb(0x900000),
    warning: rgb(0x6A4400),
    success: rgb(0x005000),
    merged: rgb(0x502080),
    note_bg: rgb(0xFFFFC0),
    comment_bg: rgb(0xFFFFE8),
    pending_bg: rgb(0xFFF0C0),
    line_selected: rgb(0xD0E0FF),
    added: rgb(0xE6FFE6),
    removed: rgb(0xFFE6E6),
    hunk: rgb(0xDDE4EE),
    conflict_block: rgb(0xFFF6C8),
    conflict_current: rgb(0xFFD890),
    code_bg: rgb(0xE8EDF2),
    lanes: LIGHT_LANES,
    ref_head: rgb(0xFFFF80),
    ref_branch: rgb(0xC0FFC0),
    ref_remote: rgb(0xC0D8FF),
    ref_tag: rgb(0xFFD8A0),
    dark: false,
};

pub const RAINY_DAY: Palette = Palette {
    face: rgb(0xB4BCC8),
    window: rgb(0xFFFFFF),
    highlight: rgb(0xE8ECF2),
    light: rgb(0xD0D6DE),
    shadow: rgb(0x707A88),
    dark_shadow: rgb(0x202430),
    text: rgb(0x000000),
    window_text: rgb(0x000000),
    gray_text: rgb(0x5C6470),
    selection: rgb(0x4E6080),
    selection_text: rgb(0xFFFFFF),
    title: rgb(0x4E6080),
    title_end: rgb(0x8898B0),
    title_text: rgb(0xFFFFFF),
    inactive_title: rgb(0x7C8494),
    inactive_title_end: rgb(0xA8AEB8),
    inactive_title_text: rgb(0xD0D4DC),
    link: rgb(0x203C78),
    error: rgb(0x900000),
    warning: rgb(0x6A4400),
    success: rgb(0x005000),
    merged: rgb(0x502080),
    note_bg: rgb(0xFFFFC0),
    comment_bg: rgb(0xFFFFE8),
    pending_bg: rgb(0xFFF0C0),
    line_selected: rgb(0xD0E0FF),
    added: rgb(0xE6FFE6),
    removed: rgb(0xFFE6E6),
    hunk: rgb(0xE0E4EC),
    conflict_block: rgb(0xFFF6C8),
    conflict_current: rgb(0xFFD890),
    code_bg: rgb(0xEEF0F4),
    lanes: LIGHT_LANES,
    ref_head: rgb(0xFFFF80),
    ref_branch: rgb(0xC0FFC0),
    ref_remote: rgb(0xC0D8FF),
    ref_tag: rgb(0xFFD8A0),
    dark: false,
};
