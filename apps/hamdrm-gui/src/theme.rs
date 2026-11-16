//! DecSAT's colours and waterfall palette (copied from DecAero's `theme.rs`, which takes
//! them from DecSAT), so the apps look alike.
//! The values are DecSAT's (`side_panel.rs`, `band_view.rs`, `top_bar.rs`,
//! `waterfall.rs`).

use eframe::egui::{self, Color32};

pub const BG: Color32 = Color32::from_rgb(17, 17, 18);
pub const PANEL: Color32 = Color32::from_rgb(27, 27, 28);
pub const PANEL2: Color32 = Color32::from_rgb(40, 40, 43);
/// Behind the constellation, the bars and the spectrum.
pub const WELL: Color32 = Color32::from_rgb(10, 11, 17);
pub const WELL_BAR: Color32 = Color32::from_rgb(44, 44, 46);
pub const AXIS: Color32 = Color32::from_rgb(22, 23, 32);
pub const GRID: Color32 = Color32::from_rgb(38, 38, 46);
pub const TEXT: Color32 = Color32::from_rgb(214, 214, 214);
pub const DIM: Color32 = Color32::from_rgb(140, 140, 144);
pub const FAINT: Color32 = Color32::from_rgb(94, 94, 98);
pub const WHITE: Color32 = Color32::WHITE;
/// Health colours (DecSAT `HEALTH_OK`, `HEALTH_WARN`, `HEALTH_BAD`).
pub const GOOD: Color32 = Color32::from_rgb(110, 220, 110);
pub const WARN: Color32 = Color32::from_rgb(240, 200, 80);
pub const BAD: Color32 = Color32::from_rgb(230, 110, 110);
pub const AMBER: Color32 = Color32::from_rgb(255, 196, 80);
pub const GREEN_STRIP: Color32 = Color32::from_rgb(47, 107, 57);
/// The first VFO colour and the constellation dots.
pub const VFO: Color32 = Color32::from_rgb(80, 200, 255);
pub const DOT: Color32 = Color32::from_rgb(120, 200, 255);
pub const TRACE: Color32 = Color32::from_rgb(150, 205, 255);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    // egui's proportional font has no arrows or loudspeaker; its monospace
    // font does, so let text fall back to it (as DecSAT does).
    let mut fonts = egui::FontDefinitions::default();
    if let Some(f) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        f.push("Hack".into());
    }
    ctx.set_fonts(fonts);
}

/// The waterfall's 256 colours: DecSAT's stops, black through purple and
/// orange to pale yellow.
pub fn palette() -> Vec<Color32> {
    const STOPS: [(f32, [u8; 3]); 6] = [
        (0.0, [0, 0, 4]),
        (0.2, [40, 11, 84]),
        (0.4, [101, 21, 110]),
        (0.6, [159, 42, 99]),
        (0.8, [237, 105, 37]),
        (1.0, [252, 255, 164]),
    ];
    (0..256)
        .map(|i| {
            let t = i as f32 / 255.0;
            let k = STOPS
                .iter()
                .rposition(|(at, _)| *at <= t)
                .unwrap_or(0)
                .min(STOPS.len() - 2);
            let ((t0, c0), (t1, c1)) = (STOPS[k], STOPS[k + 1]);
            let f = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
            let mix =
                |a: u8, b: u8| (f32::from(a) + f * (f32::from(b) - f32::from(a))).round() as u8;
            Color32::from_rgb(mix(c0[0], c1[0]), mix(c0[1], c1[1]), mix(c0[2], c1[2]))
        })
        .collect()
}
