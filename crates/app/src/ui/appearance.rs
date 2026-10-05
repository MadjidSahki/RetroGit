//! View > Appearance: color scheme, interface font and size, with a live preview.

use std::sync::Arc;

use egui::{RichText, Vec2};
use win95::theme::{self, Font};
use win95::{Bevel, Button95, Dialog, Scheme, TitleBar, bevel_frame, combo_box};

use super::Ctx;
use crate::state::{SIZES, size_label};
use crate::strings as s;

const BUTTON: Vec2 = egui::vec2(80.0, 23.0);

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(mut d) = cx.state.appearance_dialog.clone() else {
        return;
    };
    let (mut ok, mut apply, mut cancel) = (false, false, false);
    let r = Dialog::new("appearance", s::APPEARANCE_TITLE)
        .width(460.0)
        .show(egui_ctx, |ui| {
            preview(ui, d.scheme, d.font);
            ui.add_space(8.0);
            egui::Grid::new("appearance_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label(s::SCHEME);
                    combo_box(ui, "appearance_scheme", d.scheme.name(), 220.0, |ui| {
                        for scheme in Scheme::ALL {
                            if ui
                                .selectable_label(d.scheme == scheme, scheme.name())
                                .clicked()
                            {
                                d.scheme = scheme;
                            }
                        }
                    });
                    ui.end_row();
                    ui.label(s::FONT);
                    combo_box(ui, "appearance_font", d.font.name(), 220.0, |ui| {
                        for font in Font::ALL {
                            if ui.selectable_label(d.font == font, font.name()).clicked() {
                                d.font = font;
                            }
                        }
                    });
                    ui.end_row();
                    ui.label(s::SIZE);
                    combo_box(ui, "appearance_size", &size_label(d.zoom), 220.0, |ui| {
                        for z in SIZES {
                            let on = (d.zoom - z).abs() < 0.001;
                            if ui.selectable_label(on, size_label(z)).clicked() {
                                d.zoom = z;
                            }
                        }
                    });
                    ui.end_row();
                });
            ui.label(RichText::new(s::ZOOM_HINT).color(theme::palette(ui.ctx()).gray_text));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ok = ui.add(Button95::new(s::OK).min_size(BUTTON)).clicked();
                cancel = ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked();
                apply = ui.add(Button95::new(s::APPLY).min_size(BUTTON)).clicked();
            });
        });
    if let Some(open) = cx.state.appearance_dialog.as_mut() {
        open.scheme = d.scheme;
        open.font = d.font;
        open.zoom = d.zoom;
    }
    if cancel || r.close_requested {
        cx.state.appearance_cancel();
    } else if ok {
        cx.state.appearance_ok();
    } else if apply {
        cx.state.appearance_apply();
    }
}

/// Two small windows drawn with `scheme`: title bars, a menu line, a button, text, diff
/// lines and colored code, and a sample of `font`.
fn preview(ui: &mut egui::Ui, scheme: Scheme, font: Font) {
    let pal = scheme.palette();
    theme::with_palette(ui, pal, |ui| {
        bevel_frame(ui, Bevel::Sunken, pal.face, 6, |ui| {
            ui.set_width(ui.available_width());
            bevel_frame(ui, Bevel::Window, pal.face, 1, |ui| {
                ui.set_width(300.0);
                ui.push_id("preview_inactive", |ui| {
                    TitleBar::new(s::PREVIEW_INACTIVE)
                        .active(false)
                        .close_only()
                        .decorative()
                        .show(ui)
                });
            });
            ui.add_space(4.0);
            bevel_frame(ui, Bevel::Window, pal.face, 1, |ui| {
                ui.set_width(ui.available_width());
                ui.push_id("preview_active", |ui| {
                    TitleBar::new(s::PREVIEW_ACTIVE)
                        .close_only()
                        .decorative()
                        .show(ui)
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new(s::PREVIEW_NORMAL).color(pal.text));
                    ui.label(RichText::new(s::PREVIEW_DISABLED).color(pal.gray_text));
                    ui.label(
                        RichText::new(format!(" {} ", s::PREVIEW_SELECTED))
                            .color(pal.selection_text)
                            .background_color(pal.selection),
                    );
                    ui.add(Button95::new(s::PREVIEW_BUTTON));
                });
                bevel_frame(ui, Bevel::Field, pal.window, 2, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(s::PREVIEW_WINDOW_TEXT).color(pal.window_text));
                        ui.label(RichText::new(s::PREVIEW_LINK).color(pal.link));
                    });
                    code_lines(ui, &pal);
                });
                ui.label(
                    RichText::new(s::FONT_SAMPLE)
                        .family(font.family_on(ui.ctx()))
                        .color(pal.text),
                );
            });
        });
    });
}

fn code_lines(ui: &mut egui::Ui, pal: &win95::Palette) {
    let mono = egui::FontId::monospace(theme::FONT_SIZE);
    let lines = [
        (" ", "fn main() {", pal.window),
        ("-", "    println!(\"old\");", pal.removed),
        ("+", "    println!(\"new\");", pal.added),
    ];
    let spans = preview_spans(ui.ctx(), &lines.map(|(_, t, _)| t), pal.dark);
    ui.spacing_mut().item_spacing.y = 0.0;
    for (i, (sign, text, bg)) in lines.iter().enumerate() {
        let job = crate::highlight::colored_line(
            pal,
            &format!("{sign} "),
            text,
            spans.as_deref().and_then(|s| s.get(i)).map(Vec::as_slice),
            "",
            mono.clone(),
            egui::Color32::TRANSPARENT,
        );
        crate::highlight::diff_row(ui, job, 17.0, *bg);
    }
}

type Spans = Option<Vec<Vec<crate::highlight::Span>>>;

/// The preview's code colors, computed once per light or dark theme (kept in egui's data).
fn preview_spans(ctx: &egui::Context, lines: &[&str], dark: bool) -> Arc<Spans> {
    let id = egui::Id::new(("appearance_preview_spans", dark));
    if let Some(spans) = ctx.data(|d| d.get_temp::<Arc<Spans>>(id)) {
        return spans;
    }
    let texts: Vec<String> = lines.iter().map(|t| format!("{t}\n")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let spans = Arc::new(crate::highlight::highlight("preview.rs", &refs, dark));
    ctx.data_mut(|d| d.insert_temp(id, Arc::clone(&spans)));
    spans
}
