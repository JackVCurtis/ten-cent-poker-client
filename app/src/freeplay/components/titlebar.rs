//! The 46px top title-bar chrome: the bordered `10¢` dime logo, the "Ten Cent Poker" wordmark, and
//! a muted contextual sub-label (e.g. "My Tables" on the grid, "New Table" on the host screen). The
//! right-side window controls from the prototype (`–  ▢  ✕`) are cosmetic and out of scope for the
//! native frame; this renders the brand cluster only. The prototype's crypto ETH-balance chip on the
//! right is also out of scope for free play.

use eframe::egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::freeplay::theme::{self, size, Palette, Weight};

/// Left padding inside the bar (prototype `padding:0 14px`).
const PAD_X: f32 = 14.0;
/// Gap between logo / wordmark / sub-label (prototype `gap:10px`).
const CLUSTER_GAP: f32 = 10.0;
/// Diameter of the bordered dime logo circle (prototype `width/height:22px`).
const LOGO_D: f32 = 22.0;
/// Extra left margin before the sub-label (prototype `margin-left:4px`).
const SUB_MARGIN: f32 = 4.0;

/// Render the title bar across the full available width at [`size::TITLE_BAR_H`]. `sub` is the muted
/// context label shown after the wordmark.
pub fn render(ui: &mut Ui, sub: &str) {
    // Claim the full-width 46px bar; paint our own chrome (fill + bottom hairline) rather than rely
    // on egui frames so the bar reads as one quiet strip regardless of the parent layout.
    let bar = Rect::from_min_size(
        ui.max_rect().min,
        Vec2::new(ui.available_width(), size::TITLE_BAR_H),
    );
    ui.allocate_rect(bar, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(bar, 0.0, Palette::TITLE_BAR);
    // Bottom hairline (`border-bottom:1px solid rgba(255,255,255,0.06)`).
    painter.hline(
        bar.x_range(),
        bar.max.y - 0.5,
        theme::hairline(Palette::BORDER_07),
    );

    // --- left brand cluster, vertically centered, laid out left→right by hand ---
    let mid_y = bar.center().y;
    let mut x = bar.min.x + PAD_X;

    // Dime logo: a 22px circle with a translucent-accent border and centered mono "10¢".
    let logo_center = Pos2::new(x + LOGO_D / 2.0, mid_y);
    painter.circle_stroke(logo_center, LOGO_D / 2.0, Stroke::new(1.0, LOGO_BORDER));
    painter.text(
        logo_center,
        Align2::CENTER_CENTER,
        "10\u{00a2}", // 10¢ — cent sign
        theme::mono_font(8.5, Weight::SemiBold),
        Palette::ACCENT,
    );
    x += LOGO_D + CLUSTER_GAP;

    // Wordmark: "Ten Cent Poker" (Hanken 600 13px, primary text).
    let word_font = theme::ui_font(13.0, Weight::SemiBold);
    let word_galley = painter.layout_no_wrap("Ten Cent Poker".to_owned(), word_font.clone(), Palette::TEXT_PRIMARY);
    painter.text(
        Pos2::new(x, mid_y),
        Align2::LEFT_CENTER,
        "Ten Cent Poker",
        word_font,
        Palette::TEXT_PRIMARY,
    );
    x += word_galley.size().x + CLUSTER_GAP + SUB_MARGIN;

    // Muted contextual sub-label (Hanken 500 12px, dimmest muted text). Skipped when empty so the
    // wordmark stands alone.
    if !sub.is_empty() {
        painter.text(
            Pos2::new(x, mid_y),
            Align2::LEFT_CENTER,
            sub,
            theme::ui_font(12.0, Weight::Medium),
            Palette::TEXT_MUTED_DIM,
        );
    }
}

/// Dime-logo circle border: accent emerald at ~0.55 alpha (prototype `rgba(47,214,160,0.55)`).
const LOGO_BORDER: Color32 =
    Color32::from_rgba_premultiplied(0x1a, 0x76, 0x58, 140);
