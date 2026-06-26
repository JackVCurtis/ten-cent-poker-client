//! The shared free-play design system: every color/radius/size token from the handoff README, the
//! egui `Visuals`/`Style` setup, font installation, and a handful of low-level painter helpers
//! (button/pill frames, hairlines, glow rings) that components layer to approximate the prototype's
//! gradients and soft shadows. egui has no native gradients, so "gradient" surfaces are approximated
//! with a single representative flat fill (documented at each call site).
//!
//! Import convention for components: `use crate::freeplay::theme::{self, Palette, rad, size};`.

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Stroke, Ui, Vec2};

/// Every color token from the README's color table, as `Color32` consts. Hairline/translucent
/// borders are expressed as fixed alphas over white to match the `rgba(255,255,255,0.0x)` lines.
pub struct Palette;

impl Palette {
    // --- window / chrome backgrounds ---
    /// Grid screen window base (`#0a0b0e`).
    pub const APP_BG_GRID: Color32 = Color32::from_rgb(0x0a, 0x0b, 0x0e);
    /// Host screen window base (`#0b0c0f`).
    pub const APP_BG_HOST: Color32 = Color32::from_rgb(0x0b, 0x0c, 0x0f);
    /// Title bar chrome (`#101116`).
    pub const TITLE_BAR: Color32 = Color32::from_rgb(0x10, 0x11, 0x16);
    /// Grid toolbar (`#0c0d11`).
    pub const TOOLBAR: Color32 = Color32::from_rgb(0x0c, 0x0d, 0x11);

    // --- tiles / panels / surfaces ---
    /// Tile + host side-rail base (`#0d0f12`).
    pub const TILE_BASE: Color32 = Color32::from_rgb(0x0d, 0x0f, 0x12);
    /// Host side-rail base (`#0d0e12`), a hair different from the tile base in the prototype.
    pub const PANEL_BASE: Color32 = Color32::from_rgb(0x0d, 0x0e, 0x12);
    /// Surface / input / chip / menu-item base (`#16181e`).
    pub const SURFACE: Color32 = Color32::from_rgb(0x16, 0x18, 0x1e);
    /// Raised surface — summary panels, modal rows (`#14161b`).
    pub const RAISED: Color32 = Color32::from_rgb(0x14, 0x16, 0x1b);
    /// Seated-player pill background (`#161a1f`).
    pub const SEAT_PILL: Color32 = Color32::from_rgb(0x16, 0x1a, 0x1f);
    /// Neutral action-button fill on a tile bottom strip (`#1a1c21`).
    pub const NEUTRAL_BTN: Color32 = Color32::from_rgb(0x1a, 0x1c, 0x21);
    /// Slate confirm button (`#262a31`) — the calm "Leave table" / "Done" CTA.
    pub const SLATE_BTN: Color32 = Color32::from_rgb(0x26, 0x2a, 0x31);
    /// Tile bottom strip background (`#0c0e11`).
    pub const STRIP_BG: Color32 = Color32::from_rgb(0x0c, 0x0e, 0x11);
    /// Modal panel background (`#0e0f13`).
    pub const MODAL_BG: Color32 = Color32::from_rgb(0x0e, 0x0f, 0x13);
    /// Menu dropdown background (`#15171c`).
    pub const MENU_BG: Color32 = Color32::from_rgb(0x15, 0x17, 0x1c);

    // --- felt (gradients approximated by a single representative fill) ---
    /// Felt area radial-gradient mid tone (`#11161a`).
    pub const FELT_AREA: Color32 = Color32::from_rgb(0x11, 0x16, 0x1a);
    /// Felt oval surface — linear-gradient top stop (`#141a1e`).
    pub const FELT_TOP: Color32 = Color32::from_rgb(0x14, 0x1a, 0x1e);
    /// Felt oval surface — linear-gradient bottom stop (`#0e1216`).
    pub const FELT_BOTTOM: Color32 = Color32::from_rgb(0x0e, 0x12, 0x16);

    // --- card / chips ---
    /// Playing-card face (`#ecedf0`).
    pub const CARD_FACE: Color32 = Color32::from_rgb(0xec, 0xed, 0xf0);

    // --- text ---
    /// Primary text (`#eef0f3`).
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xee, 0xf0, 0xf3);
    /// Primary text, slightly dimmer variant (`#e6e8eb`).
    pub const TEXT_PRIMARY_DIM: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xeb);
    /// Secondary text — labels (`#9aa0ab`).
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x9a, 0xa0, 0xab);
    /// Muted text (`#6e737d`).
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x6e, 0x73, 0x7d);
    /// Muted text, dimmest variant (`#5b606b`).
    pub const TEXT_MUTED_DIM: Color32 = Color32::from_rgb(0x5b, 0x60, 0x6b);
    /// "On accent" near-black text used on emerald CTAs (`#04241a`).
    pub const ON_ACCENT: Color32 = Color32::from_rgb(0x04, 0x24, 0x1a);

    // --- accent (emerald) ---
    /// Accent emerald (`#2fd6a0`) — active/selected, your-turn ring, primary CTA.
    pub const ACCENT: Color32 = Color32::from_rgb(0x2f, 0xd6, 0xa0);
    /// Accent text on tint (`#7ff0c8`).
    pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0x7f, 0xf0, 0xc8);
    /// Seat-stack mono color, a desaturated accent (`#74a591`).
    pub const ACCENT_STACK: Color32 = Color32::from_rgb(0x74, 0xa5, 0x91);
    /// Accent tint background `rgba(47,214,160,0.12)`.
    pub const ACCENT_TINT: Color32 = Color32::from_rgba_premultiplied(0x2f, 0xd6, 0xa0, 31);

    // --- suits ---
    /// Hearts/diamonds (`#d6443c`).
    pub const SUIT_RED: Color32 = Color32::from_rgb(0xd6, 0x44, 0x3c);
    /// Spades/clubs (`#15171b`).
    pub const SUIT_BLACK: Color32 = Color32::from_rgb(0x15, 0x17, 0x1b);

    // --- timer / action ---
    /// Timer amber, 18–40% remaining (`#e0b15a`).
    pub const TIMER_AMBER: Color32 = Color32::from_rgb(0xe0, 0xb1, 0x5a);
    /// Timer red, <18% remaining (`#e0685a`).
    pub const TIMER_RED: Color32 = Color32::from_rgb(0xe0, 0x68, 0x5a);
    /// Fold-button text (`#c6776f`).
    pub const FOLD_TEXT: Color32 = Color32::from_rgb(0xc6, 0x77, 0x6f);

    // --- borders (rgba over white) ---
    /// Faintest hairline `rgba(255,255,255,0.05)`.
    pub const BORDER_05: Color32 = Color32::from_rgba_premultiplied(13, 13, 13, 13);
    /// Standard hairline `rgba(255,255,255,0.07)`.
    pub const BORDER_07: Color32 = Color32::from_rgba_premultiplied(18, 18, 18, 18);
    /// Stronger panel border `rgba(255,255,255,0.10)`.
    pub const BORDER_10: Color32 = Color32::from_rgba_premultiplied(26, 26, 26, 26);
    /// Dashed "Open seat" border `rgba(255,255,255,0.13)`.
    pub const BORDER_DASH: Color32 = Color32::from_rgba_premultiplied(33, 33, 33, 33);

    /// Accent border at ~0.4 alpha (your-turn / seat to-act rim).
    pub const ACCENT_BORDER: Color32 = Color32::from_rgba_premultiplied(0x13, 0x55, 0x40, 102);
    /// Faint accent border on the felt oval `rgba(47,214,160,0.18)`.
    pub const ACCENT_BORDER_FAINT: Color32 = Color32::from_rgba_premultiplied(8, 38, 28, 46);
}

/// Corner-radius tokens (px), per the README "Radius / spacing" line.
pub mod rad {
    /// Table tiles — 14px.
    pub const TILE: u8 = 14;
    /// Modal — 16px.
    pub const MODAL: u8 = 16;
    /// Panels / buttons — 11px.
    pub const PANEL: u8 = 11;
    /// Inputs / chips / menu items — 9px.
    pub const INPUT: u8 = 9;
    /// Playing cards — 5px.
    pub const CARD: u8 = 5;
    /// Badges — 5px.
    pub const BADGE: u8 = 5;
}

/// Fixed layout sizes (px) from the README.
pub mod size {
    /// Title bar height — 46px.
    pub const TITLE_BAR_H: f32 = 46.0;
    /// Grid toolbar height — 54px.
    pub const TOOLBAR_H: f32 = 54.0;
    /// Tile header height — 42px.
    pub const TILE_HEADER_H: f32 = 42.0;
    /// Grid gap between tiles — 14px.
    pub const GRID_GAP: f32 = 14.0;
    /// Outer grid padding — 16px.
    pub const OUTER_PAD: f32 = 16.0;
    /// Host left config rail width — 392px (standalone) / 380px (slide-over).
    pub const HOST_RAIL_W: f32 = 392.0;
    /// Host slide-over width — 380px.
    pub const SLIDEOVER_W: f32 = 380.0;
    /// Leave/cash-out modal width — 404px.
    pub const MODAL_W: f32 = 404.0;
}

/// Logical font weight, mapped to a family + (when real fonts are wired) a named font.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    SemiBold,
    Bold,
}

/// A proportional (Hanken Grotesk) `FontId` at `px`, resolving to the bundled static weight that
/// [`install_fonts`] registered. Missing glyphs (card suits, etc.) fall through to egui's default
/// fonts via the per-weight family's fallback chain.
pub fn ui_font(px: f32, weight: Weight) -> FontId {
    let family = match weight {
        Weight::Regular => "hanken-400",
        Weight::Medium => "hanken-500",
        Weight::SemiBold => "hanken-600",
        Weight::Bold => "hanken-700",
    };
    FontId::new(px, FontFamily::Name(family.into()))
}

/// A monospace (JetBrains Mono) `FontId` at `px`, for chip counts/stacks/blinds/timers/IDs. JetBrains
/// Mono ships 400/500/600; `Bold` maps to the 600 SemiBold.
pub fn mono_font(px: f32, weight: Weight) -> FontId {
    let family = match weight {
        Weight::Regular => "jbmono-400",
        Weight::Medium => "jbmono-500",
        Weight::SemiBold | Weight::Bold => "jbmono-600",
    };
    FontId::new(px, FontFamily::Name(family.into()))
}

/// Install the bundled Hanken Grotesk (UI) + JetBrains Mono (numeric) static weights as named font
/// families (`hanken-400…700`, `jbmono-400…600`), each falling back to egui's default fonts so glyphs
/// the typefaces lack (the card suits ♠♥♦♣, etc.) still render. Also points the base Proportional /
/// Monospace families at the 400 weights so any stray egui widget text matches. Idempotent.
pub fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};

    let mut fonts = egui::FontDefinitions::default();

    let mut add = |key: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(key.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add(
        "hanken-400",
        include_bytes!("fonts/HankenGrotesk-Regular.ttf"),
    );
    add(
        "hanken-500",
        include_bytes!("fonts/HankenGrotesk-Medium.ttf"),
    );
    add(
        "hanken-600",
        include_bytes!("fonts/HankenGrotesk-SemiBold.ttf"),
    );
    add("hanken-700", include_bytes!("fonts/HankenGrotesk-Bold.ttf"));
    add(
        "jbmono-400",
        include_bytes!("fonts/JetBrainsMono-Regular.ttf"),
    );
    add(
        "jbmono-500",
        include_bytes!("fonts/JetBrainsMono-Medium.ttf"),
    );
    add(
        "jbmono-600",
        include_bytes!("fonts/JetBrainsMono-SemiBold.ttf"),
    );

    // Default fallback chains (egui's bundled fonts carry the suit/symbol glyphs Hanken/JBMono lack).
    let prop_fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mono_fallback = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();

    for key in ["hanken-400", "hanken-500", "hanken-600", "hanken-700"] {
        let mut list = vec![key.to_owned()];
        list.extend(prop_fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(key.into()), list);
    }
    for key in ["jbmono-400", "jbmono-500", "jbmono-600"] {
        let mut list = vec![key.to_owned()];
        list.extend(mono_fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(key.into()), list);
    }

    // Stray egui text (anything not routed through ui_font/mono_font) uses the 400 weights.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "hanken-400".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "jbmono-400".to_owned());

    ctx.set_fonts(fonts);
}

/// Apply the dark free-play visuals + spacing to the egui context. Idempotent; call once at startup.
pub fn apply_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;

    v.dark_mode = true;
    v.panel_fill = Palette::APP_BG_GRID;
    v.window_fill = Palette::MODAL_BG;
    v.extreme_bg_color = Palette::SURFACE;
    v.faint_bg_color = Palette::RAISED;
    v.override_text_color = Some(Palette::TEXT_PRIMARY);
    v.window_stroke = Stroke::new(1.0, Palette::BORDER_10);
    v.window_corner_radius = CornerRadius::same(rad::MODAL);
    v.selection.bg_fill = Palette::ACCENT_TINT;
    v.selection.stroke = Stroke::new(1.0, Palette::ACCENT);
    v.hyperlink_color = Palette::ACCENT;

    // Widget states default to the neutral surface; components mostly paint their own frames, so
    // these are just sensible fallbacks for stray egui widgets.
    let wv = &mut v.widgets;
    for w in [
        &mut wv.noninteractive,
        &mut wv.inactive,
        &mut wv.hovered,
        &mut wv.active,
        &mut wv.open,
    ] {
        w.bg_fill = Palette::SURFACE;
        w.weak_bg_fill = Palette::SURFACE;
        w.bg_stroke = Stroke::new(1.0, Palette::BORDER_07);
        w.fg_stroke = Stroke::new(1.0, Palette::TEXT_SECONDARY);
        w.corner_radius = CornerRadius::same(rad::INPUT);
    }
    wv.inactive.fg_stroke = Stroke::new(1.0, Palette::TEXT_PRIMARY);
    wv.hovered.bg_stroke = Stroke::new(1.0, Palette::BORDER_10);

    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);

    ctx.set_style(style);
}

// ---------------------------------------------------------------------------
// Painter helpers — small, reusable primitives every component layers.
// ---------------------------------------------------------------------------

/// A 1px hairline stroke at the given border color.
pub fn hairline(color: Color32) -> Stroke {
    Stroke::new(1.0, color)
}

/// Fill `rect` with `fill` and an optional hairline `stroke`, rounded to `radius` px.
pub fn fill_rect(ui: &Ui, rect: Rect, radius: u8, fill: Color32, stroke: Stroke) {
    ui.painter().rect(
        rect,
        CornerRadius::same(radius),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
}

/// Paint a primary accent (emerald) button frame into `rect`. Caller draws the label on top.
pub fn accent_button_frame(ui: &Ui, rect: Rect, radius: u8) {
    fill_rect(ui, rect, radius, Palette::ACCENT, Stroke::NONE);
}

/// Paint a neutral surface button frame (`#1a1c21` + hairline) into `rect`.
pub fn neutral_button_frame(ui: &Ui, rect: Rect, radius: u8) {
    fill_rect(
        ui,
        rect,
        radius,
        Palette::NEUTRAL_BTN,
        hairline(Palette::BORDER_07),
    );
}

/// Paint a slate confirm-button frame (`#262a31` + hairline) into `rect`.
pub fn slate_button_frame(ui: &Ui, rect: Rect, radius: u8) {
    fill_rect(
        ui,
        rect,
        radius,
        Palette::SLATE_BTN,
        hairline(Palette::BORDER_07),
    );
}

/// Paint a pill frame: rounded surface fill + hairline. When `accent` is set, swaps to the accent
/// tint + accent border (selected segmented-control / to-act seat styling).
pub fn pill_frame(ui: &Ui, rect: Rect, radius: u8, fill: Color32, accent: bool) {
    let (fill, stroke) = if accent {
        (Palette::ACCENT_TINT, hairline(Palette::ACCENT_BORDER))
    } else {
        (fill, hairline(Palette::BORDER_07))
    };
    fill_rect(ui, rect, radius, fill, stroke);
}

/// Paint a soft outer glow/halo around `rect` for the prototype's `box-shadow: 0 0 Npx rgba(...)`
/// your-turn / to-act accent. egui's [`epaint::Shadow`] renders a genuine gaussian blur, so this reads
/// as one soft halo (not the stacked hard rings a multi-stroke stand-in produces). `layers` scales the
/// blur radius; `color` carries the base alpha. Painted under the element fill so it haloes the edge.
pub fn soft_ring(ui: &Ui, rect: Rect, radius: u8, color: Color32, layers: u8) {
    // Keep the blur tight enough that the halo stays within the tile/seat gap and doesn't bleed into
    // neighbours: ~3px per requested layer, capped.
    let blur = (layers.max(1)).saturating_mul(3).min(22);
    let shadow = egui::epaint::Shadow {
        offset: [0, 0],
        blur,
        spread: 1,
        color,
    };
    ui.painter()
        .add(shadow.as_shape(rect, CornerRadius::same(radius)));
}

/// Paint a filled circle of `diameter` centered at `center` (avatars, dealer chip, status dots).
pub fn dot(ui: &Ui, center: Pos2, diameter: f32, fill: Color32) {
    ui.painter().circle_filled(center, diameter / 2.0, fill);
}
