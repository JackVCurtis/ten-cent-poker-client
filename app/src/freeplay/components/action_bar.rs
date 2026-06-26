//! The tile bottom strip: your hole cards (larger) + `You` + your chip stack on the left. When it is
//! your turn: a thin timer bar across the top (accent → amber → red as it depletes) and the action
//! controls — a bet/raise SIZING stepper (`− value +` over the live `[min_bet | min_raise_to .. max_to]`
//! range) plus `Fold` / `Check`-or-`Call <amount>` / `Bet`-or-`Raise <to>` / `All-in`, each gated by
//! the live legal-action bounds. When it is not your turn: a muted `Waiting on <name>` line with a
//! pulsing dot.
//!
//! Painted by hand (not in-flow widgets) so the strip can sit at an explicit `rect` inside the tile,
//! mirroring the prototype's `padding:11px 14px` row. The buttons are square-cornered surface pills
//! laid out right-aligned; the returned [`BarOutcome`] carries the chosen [`Act`] and any sizing
//! step the stepper requested (applied to the app-owned bet target).

use eframe::egui::{pos2, Align2, Color32, Rect, Sense, Stroke, Ui, Vec2};

use crate::freeplay::cards;
use crate::freeplay::model::{Act, Table};
use crate::freeplay::theme::{self, rad, Palette, Weight};

/// Timer-bar track height (px) — the prototype's `height:3px` rail across the top of the strip.
const TIMER_H: f32 = 3.0;
/// Horizontal / vertical inner padding of the content row (prototype `padding:11px 14px`).
const PAD_X: f32 = 14.0;
const PAD_Y: f32 = 11.0;
/// Gap between the action buttons (prototype `gap:6px`).
const BTN_GAP: f32 = 6.0;
/// Gap between the hero cards and the `You`/stack column (prototype `gap:9px`).
const HERO_GAP: f32 = 9.0;

/// What the bottom strip produced this frame: a chosen [`Act`] (a button click) and/or a sizing
/// step (the `−`/`+` stepper). Both can be empty in the same frame.
#[derive(Clone, Copy, Default)]
pub struct BarOutcome {
    /// The action the player committed to this frame (a button click).
    pub act: Option<Act>,
    /// Requested change (chips, signed) to the app-owned bet/raise sizing target, from the stepper.
    pub size_delta: i64,
}

/// Render the bottom strip into `rect` for `table`. `pulse` is the eased opacity for the
/// waiting/pulsing dot. Returns the chosen action and/or sizing step for this frame.
pub fn render(ui: &mut Ui, rect: Rect, table: &Table, pulse: f32) -> BarOutcome {
    // Strip backing (`#0c0e11`) with the top hairline (`rgba(255,255,255,0.05)`) the prototype draws
    // as a `border-top` on the strip container.
    ui.painter().rect_filled(rect, 0.0, Palette::STRIP_BG);
    ui.painter().hline(
        rect.x_range(),
        rect.min.y,
        theme::hairline(Palette::BORDER_05),
    );

    // The your-turn timer rail consumes the top `TIMER_H` px; the content row fills the rest.
    let content_top = if table.your_turn {
        paint_timer(ui, rect, table);
        rect.min.y + TIMER_H
    } else {
        rect.min.y
    };
    let content = Rect::from_min_max(
        pos2(rect.min.x + PAD_X, content_top + PAD_Y),
        pos2(rect.max.x - PAD_X, rect.max.y - PAD_Y),
    );

    paint_hero(ui, content, table);

    if table.your_turn {
        paint_actions(ui, content, table)
    } else {
        paint_waiting(ui, content, table, pulse);
        BarOutcome::default()
    }
}

/// Paint the depleting timer rail at the top of `rect`: a faint full-width track with a fill whose
/// width tracks `time_left/18000` and whose color steps accent (>40%) → amber (18–40%) → red (<18%),
/// matching the prototype's `pct>40?accent:pct>18?amber:red`.
fn paint_timer(ui: &Ui, rect: Rect, table: &Table) {
    let painter = ui.painter();
    let track = Rect::from_min_size(rect.min, Vec2::new(rect.width(), TIMER_H));
    painter.rect_filled(track, 0.0, Palette::BORDER_05);

    let frac = table.timer_frac();
    let color = if frac > 0.40 {
        Palette::ACCENT
    } else if frac > 0.18 {
        Palette::TIMER_AMBER
    } else {
        Palette::TIMER_RED
    };
    if frac > 0.0 {
        let fill = Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac, TIMER_H));
        painter.rect_filled(fill, 0.0, color);
    }
}

/// Paint the left cluster: the larger hero hole cards followed by the `You` label over the hero
/// stack (mono, desaturated accent), vertically centred in the content row.
fn paint_hero(ui: &Ui, content: Rect, table: &Table) {
    let hero = cards::parse(&table.hero);
    let cy = content.center().y;
    let mut x = content.min.x;

    // Hero cards (big), laid out left-to-right with the prototype's 4px gap.
    if !hero.is_empty() {
        let size = cards::card_size(true);
        let gap = cards::card_gap(true);
        let top = cy - size.y / 2.0;
        for c in &hero {
            let card_rect = Rect::from_min_size(pos2(x, top), size);
            cards::draw_card(ui.painter(), c, card_rect, true);
            x += size.x + gap;
        }
        x += HERO_GAP - gap;
    }

    // `You` (accent text, 11px/600) stacked over the stack count (mono 10px/500, desaturated accent).
    let stack = group_thousands(table.hero_stack());
    ui.painter().text(
        pos2(x, cy - 6.0),
        Align2::LEFT_CENTER,
        "You",
        theme::ui_font(11.0, Weight::SemiBold),
        Palette::ACCENT_TEXT,
    );
    ui.painter().text(
        pos2(x, cy + 7.0),
        Align2::LEFT_CENTER,
        stack,
        theme::mono_font(10.0, Weight::Medium),
        Palette::ACCENT_STACK,
    );
}

/// One action button descriptor (label + styling + the [`Act`] it emits).
struct Btn {
    label: String,
    text: Color32,
    accent: bool,
    act: Act,
}

/// Button height + corner padding (prototype `padding:8px 12px`, accent a hair wider).
const BTN_H: f32 = 30.0;

/// Paint the right-aligned action controls — the bet/raise SIZING stepper plus the gated action
/// buttons (`Fold` / `Check`-or-`Call <amount>` / `Bet`-or-`Raise <to>` / `All-in`) — and return the
/// chosen [`Act`] and/or sizing step. Each control is included only when the live legal bounds allow
/// it; the row is measured then placed right-to-left so it hugs the strip's right edge.
fn paint_actions(ui: &mut Ui, content: Rect, table: &Table) -> BarOutcome {
    let font = theme::ui_font(12.0, Weight::SemiBold);

    // --- gated button set, in left-to-right visual order ---
    let mut btns: Vec<Btn> = Vec::with_capacity(4);
    // Fold is always available on your turn.
    btns.push(Btn {
        label: "Fold".to_string(),
        text: Palette::FOLD_TEXT,
        accent: false,
        act: Act::Fold,
    });
    // Check (nothing owed) or Call <amount>.
    if table.can_check {
        btns.push(Btn {
            label: "Check".to_string(),
            text: Color32::from_rgb(0xd4, 0xd8, 0xde),
            accent: false,
            act: Act::CheckCall,
        });
    } else if table.can_call {
        btns.push(Btn {
            label: format!("Call {}", group_thousands(table.to_call)),
            text: Color32::from_rgb(0xd4, 0xd8, 0xde),
            accent: false,
            act: Act::CheckCall,
        });
    }
    // Open a bet, or raise to the chosen sizing target.
    if table.can_bet {
        btns.push(Btn {
            label: format!("Bet {}", group_thousands(table.bet_to)),
            text: Palette::ON_ACCENT,
            accent: true,
            act: Act::Bet(table.bet_to),
        });
    } else if table.can_raise {
        btns.push(Btn {
            label: format!("Raise {}", group_thousands(table.bet_to)),
            text: Palette::ON_ACCENT,
            accent: true,
            act: Act::Raise(table.bet_to),
        });
    }
    // Explicit All-in (commit the whole stack).
    if table.can_all_in {
        btns.push(Btn {
            label: "All-in".to_string(),
            text: Color32::from_rgb(0xd4, 0xd8, 0xde),
            accent: false,
            act: Act::AllIn,
        });
    }

    // --- sizing stepper, shown only when a bet/raise is on offer ---
    let show_sizing = table.can_bet || table.can_raise;
    let sizing_w = if show_sizing {
        stepper_width(ui, table)
    } else {
        0.0
    };

    // Measure each button (label + side padding; accent a hair wider).
    let widths: Vec<f32> = btns
        .iter()
        .map(|b| {
            let lw = ui
                .painter()
                .layout_no_wrap(b.label.clone(), font.clone(), b.text)
                .size()
                .x;
            let pad = if b.accent { 14.0 } else { 12.0 };
            lw + pad * 2.0
        })
        .collect();

    let n_segments = btns.len() + if show_sizing { 1 } else { 0 };
    let total: f32 =
        sizing_w + widths.iter().sum::<f32>() + BTN_GAP * (n_segments.saturating_sub(1)) as f32;

    let mut x = content.max.x - total;
    let top = content.center().y - BTN_H / 2.0;
    let mut out = BarOutcome::default();

    // Sizing stepper leftmost in the cluster.
    if show_sizing {
        let rect = Rect::from_min_size(pos2(x, top), Vec2::new(sizing_w, BTN_H));
        out.size_delta += paint_stepper(ui, rect, table);
        x += sizing_w + BTN_GAP;
    }

    // Then the buttons.
    for (i, (b, w)) in btns.iter().zip(widths.iter()).enumerate() {
        let btn = Rect::from_min_size(pos2(x, top), Vec2::new(*w, BTN_H));
        let resp = ui.interact(btn, ui.id().with(("act_btn", table.id, i)), Sense::click());
        if b.accent {
            let fill = if resp.hovered() {
                Color32::from_rgb(0x3a, 0xe0, 0xac)
            } else {
                Palette::ACCENT
            };
            theme::fill_rect(ui, btn, rad::INPUT, fill, Stroke::NONE);
        } else {
            let fill = if resp.hovered() {
                Palette::SURFACE
            } else {
                Palette::NEUTRAL_BTN
            };
            theme::fill_rect(
                ui,
                btn,
                rad::INPUT,
                fill,
                theme::hairline(Palette::BORDER_07),
            );
        }
        ui.painter().text(
            btn.center(),
            Align2::CENTER_CENTER,
            &b.label,
            font.clone(),
            b.text,
        );
        if resp.clicked() {
            out.act = Some(b.act);
        }
        x += w + BTN_GAP;
    }
    out
}

/// The bet/raise sizing increment (chips): one big-blind-ish step derived from the legal minimum bet.
fn sizing_step(table: &Table) -> u64 {
    table.min_bet.max(1)
}

/// Width of the `− value +` sizing stepper pill: two square arrow zones plus the centred mono value.
fn stepper_width(ui: &Ui, table: &Table) -> f32 {
    let value = group_thousands(table.bet_to);
    let value_w = ui
        .painter()
        .layout_no_wrap(
            value,
            theme::mono_font(11.0, Weight::SemiBold),
            Palette::TEXT_PRIMARY,
        )
        .size()
        .x;
    // Two BTN_H-wide arrow zones + the value column + a little breathing room each side.
    BTN_H * 2.0 + value_w + 12.0
}

/// Paint the `− value +` stepper into `rect`; clicking an arrow returns the signed sizing step. The
/// value reads the app-owned target (`table.bet_to`); the actual clamp into the legal range happens
/// when the app applies the step.
fn paint_stepper(ui: &mut Ui, rect: Rect, table: &Table) -> i64 {
    theme::fill_rect(
        ui,
        rect,
        rad::INPUT,
        Palette::NEUTRAL_BTN,
        theme::hairline(Palette::BORDER_07),
    );

    let step = sizing_step(table) as i64;
    let arrow_w = BTN_H;
    let down = Rect::from_min_size(rect.min, Vec2::new(arrow_w, rect.height()));
    let up = Rect::from_min_size(
        pos2(rect.max.x - arrow_w, rect.min.y),
        Vec2::new(arrow_w, rect.height()),
    );

    let down_resp = ui.interact(down, ui.id().with(("bet_down", table.id)), Sense::click());
    let up_resp = ui.interact(up, ui.id().with(("bet_up", table.id)), Sense::click());

    let arrow_color = |hovered: bool| {
        if hovered {
            Palette::TEXT_PRIMARY
        } else {
            Palette::TEXT_SECONDARY
        }
    };
    ui.painter().text(
        down.center(),
        Align2::CENTER_CENTER,
        "\u{2212}",
        theme::ui_font(15.0, Weight::Medium),
        arrow_color(down_resp.hovered()),
    );
    ui.painter().text(
        up.center(),
        Align2::CENTER_CENTER,
        "+",
        theme::ui_font(15.0, Weight::Medium),
        arrow_color(up_resp.hovered()),
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        group_thousands(table.bet_to),
        theme::mono_font(11.0, Weight::SemiBold),
        Palette::TEXT_PRIMARY,
    );

    let mut delta = 0;
    if down_resp.clicked() {
        delta -= step;
    }
    if up_resp.clicked() {
        delta += step;
    }
    delta
}

/// Paint the not-your-turn line: a muted pulsing dot (`#5b606b`) followed by the `Waiting on <name>`
/// copy (or `Waiting for players` when no one is seated to act), right-aligned in the content row.
fn paint_waiting(ui: &mut Ui, content: Rect, table: &Table, pulse: f32) {
    let label = match table.acting_name() {
        Some(name) => format!("Waiting on {name}"),
        None => "Waiting for players".to_string(),
    };
    let font = theme::ui_font(11.0, Weight::Regular);
    let cy = content.center().y;

    let dot_d = 6.0;
    let gap = 8.0;
    let text_w = ui
        .painter()
        .layout_no_wrap(label.clone(), font.clone(), Palette::TEXT_MUTED)
        .size()
        .x;
    let total = dot_d + gap + text_w;
    let mut x = content.max.x - total;

    // Pulsing status dot — same alpha easing as `widgets::status_dot`, applied here so the dot can be
    // hand-placed at the start of the right-aligned waiting line. `pulse` comes from the shared
    // animation clock (see `widgets::pulse_opacity`).
    let dot_c = Palette::TEXT_MUTED_DIM;
    let a = (255.0 * pulse.clamp(0.0, 1.0)) as u8;
    theme::dot(
        ui,
        pos2(x + dot_d / 2.0, cy),
        dot_d,
        Color32::from_rgba_premultiplied(dot_c.r(), dot_c.g(), dot_c.b(), a),
    );
    x += dot_d + gap;
    ui.painter().text(
        pos2(x, cy),
        Align2::LEFT_CENTER,
        label,
        font,
        Palette::TEXT_MUTED,
    );
}

/// Format a chip count with comma thousands separators (`1700` → `"1,700"`), matching the prototype's
/// free-play `Number(n).toLocaleString()`. (Local copy — the felt module's grouping helper is private.)
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (idx, b) in bytes.iter().enumerate() {
        if idx > 0 && (len - idx) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
