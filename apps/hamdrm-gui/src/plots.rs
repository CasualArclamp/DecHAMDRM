//! Constellations and channel plots drawn with the painter (no plotting crate), in the
//! manner of DecDRM's channel tab: |H|², group delay, impulse response with the guard
//! interval, per-carrier SNR and the fading map of the last symbols.

use crate::theme::*;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use hamdrm_engine::ChainVisuals;

/// A square constellation of the given cells, small dots (as DecDVB-T draws them).
pub fn constellation(
    ui: &mut Ui,
    title: &str,
    cells: &[hamdrm_engine::Cplx],
    side: f32,
    range: f64,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, WELL);
    let c = rect.center();
    p.line_segment(
        [pos2(c.x, rect.top()), pos2(c.x, rect.bottom())],
        Stroke::new(1.0, AXIS),
    );
    p.line_segment(
        [pos2(rect.left(), c.y), pos2(rect.right(), c.y)],
        Stroke::new(1.0, AXIS),
    );
    let scale = f64::from(side) / 2.0 / range;
    for z in cells {
        let q = pos2(c.x + (z.re * scale) as f32, c.y - (z.im * scale) as f32);
        if rect.contains(q) {
            // A 1.6 px square is cheaper than a circle and reads as a dot.
            p.rect_filled(Rect::from_center_size(q, vec2(1.6, 1.6)), 0.0, DOT);
        }
    }
    p.text(
        rect.left_top() + vec2(5.0, 3.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(11.0),
        DIM,
    );
}

struct Frame {
    rect: Rect,
    plot: Rect,
}

fn frame(ui: &mut Ui, size: egui::Vec2, title: &str) -> Frame {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, WELL);
    p.text(
        rect.left_top() + vec2(6.0, 3.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(11.0),
        DIM,
    );
    let plot = Rect::from_min_max(rect.min + vec2(34.0, 18.0), rect.max - vec2(8.0, 16.0));
    Frame { rect, plot }
}

fn grid_y(ui: &Ui, f: &Frame, lo: f64, hi: f64, step: f64, unit: &str) {
    let p = ui.painter_at(f.rect);
    let mut v = (lo / step).ceil() * step;
    while v <= hi + 1e-9 {
        let y = map(v, lo, hi, f.plot.bottom(), f.plot.top());
        p.line_segment(
            [pos2(f.plot.left(), y), pos2(f.plot.right(), y)],
            Stroke::new(1.0, GRID),
        );
        p.text(
            pos2(f.plot.left() - 3.0, y),
            Align2::RIGHT_CENTER,
            if step < 1.0 {
                format!("{v:.1}{unit}")
            } else {
                format!("{v:.0}{unit}")
            },
            FontId::proportional(9.5),
            FAINT,
        );
        v += step;
    }
}

fn map(v: f64, a: f64, b: f64, pa: f32, pb: f32) -> f32 {
    if (b - a).abs() < 1e-12 {
        return pa;
    }
    pa + ((v - a) / (b - a)) as f32 * (pb - pa)
}

fn x_label(ui: &Ui, f: &Frame, text: &str) {
    ui.painter_at(f.rect).text(
        pos2(f.plot.center().x, f.rect.bottom() - 2.0),
        Align2::CENTER_BOTTOM,
        text,
        FontId::proportional(9.5),
        FAINT,
    );
}

fn polyline(ui: &Ui, f: &Frame, pts: Vec<Pos2>, color: Color32) {
    ui.painter_at(f.plot.expand(1.0))
        .add(egui::Shape::line(pts, Stroke::new(1.3, color)));
}

/// Robust display range (5th..95th percentile, widened).
fn range(values: impl Iterator<Item = f64>, min_span: f64) -> (f64, f64) {
    let mut v: Vec<f64> = values.filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return (-min_span / 2.0, min_span / 2.0);
    }
    v.sort_by(f64::total_cmp);
    let q = |x: f64| v[((v.len() - 1) as f64 * x).round() as usize];
    let (lo, hi) = (q(0.05), q(0.95));
    let mid = (lo + hi) / 2.0;
    let half = ((hi - lo) * 1.3).max(min_span) / 2.0;
    (mid - half, mid + half)
}

/// The channel tab: five plots in a grid.
pub fn channel_tab(
    ui: &mut Ui,
    ch: &ChainVisuals,
    fading: &mut Option<egui::TextureHandle>,
    palette: &[Color32],
) {
    if ch.chan.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No channel estimate yet").color(FAINT))
        });
        return;
    }
    let w = (ui.available_width() - 8.0) / 2.0;
    let h = ((ui.available_height() - 12.0) / 3.0).max(90.0);
    let k0 = f64::from(ch.kmin);
    let k1 = k0 + ch.chan.len() as f64 - 1.0;
    ui.horizontal(|ui| {
        // |H|² per carrier.
        let f = frame(ui, vec2(w, h), "Channel |H|² (dB)");
        let db: Vec<f64> = ch
            .chan
            .iter()
            .map(|z| 10.0 * z.norm_sqr().max(1e-12).log10())
            .collect();
        let (lo, hi) = range(db.iter().copied(), 12.0);
        grid_y(ui, &f, lo, hi, ((hi - lo) / 4.0).max(1.0).round(), "");
        let pts = db
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                pos2(
                    map(k0 + i as f64, k0, k1, f.plot.left(), f.plot.right()),
                    map(v, lo, hi, f.plot.bottom(), f.plot.top()),
                )
            })
            .collect();
        polyline(ui, &f, pts, TRACE);
        x_label(ui, &f, "carrier");
        // Group delay.
        let f = frame(ui, vec2(w, h), "Group delay (ms)");
        let gd: Vec<f64> = ch
            .chan
            .windows(2)
            .map(|p| {
                -(p[1] * p[0].conj()).arg() / (2.0 * std::f64::consts::PI * ch.spacing_hz.max(1.0))
                    * 1e3
            })
            .collect();
        let (lo, hi) = range(gd.iter().copied(), 1.0);
        grid_y(ui, &f, lo, hi, ((hi - lo) / 4.0).max(0.1), "");
        let pts = gd
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                pos2(
                    map(k0 + i as f64 + 0.5, k0, k1, f.plot.left(), f.plot.right()),
                    map(v, lo, hi, f.plot.bottom(), f.plot.top()),
                )
            })
            .collect();
        polyline(ui, &f, pts, AMBER);
        x_label(ui, &f, "carrier");
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        // Impulse response (power delay profile), dB rel. peak, guard interval shaded.
        let f = frame(ui, vec2(w, h), "Impulse response (dB rel. peak)");
        if let (Some(ax), false) = (&ch.pds_axis, ch.pds.is_empty()) {
            let peak = ch.pds.iter().cloned().fold(1e-30, f64::max);
            let x0 = ax.start_ms;
            let x1 = ax.start_ms + ax.step_ms * (ch.pds.len() as f64 - 1.0);
            let p = ui.painter_at(f.plot);
            let (g0, g1) = ax.guard_ms;
            p.rect_filled(
                Rect::from_x_y_ranges(
                    map(g0, x0, x1, f.plot.left(), f.plot.right())
                        ..=map(g1, x0, x1, f.plot.left(), f.plot.right()),
                    f.plot.y_range(),
                ),
                0.0,
                Color32::from_rgba_unmultiplied(80, 200, 255, 22),
            );
            for x in [ax.pds_begin_ms, ax.pds_end_ms] {
                let xx = map(x, x0, x1, f.plot.left(), f.plot.right());
                p.line_segment(
                    [pos2(xx, f.plot.top()), pos2(xx, f.plot.bottom())],
                    Stroke::new(1.0, WARN),
                );
            }
            grid_y(ui, &f, -40.0, 0.0, 10.0, "");
            let pts = ch
                .pds
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let db = (10.0 * (v / peak).max(1e-4).log10()).max(-40.0);
                    pos2(
                        map(
                            x0 + i as f64 * ax.step_ms,
                            x0,
                            x1,
                            f.plot.left(),
                            f.plot.right(),
                        ),
                        map(db, -40.0, 0.0, f.plot.bottom(), f.plot.top()),
                    )
                })
                .collect();
            polyline(ui, &f, pts, GOOD);
            x_label(
                ui,
                &f,
                &format!("delay, ms ({x0:.1} … {x1:.1}); shaded: guard interval"),
            );
        }
        // Per-carrier SNR.
        let f = frame(ui, vec2(w, h), "SNR per carrier (dB)");
        if !ch.snr_profile.is_empty() {
            let (lo, hi) = (
                0.0,
                ch.snr_profile
                    .iter()
                    .map(|p| p.1)
                    .fold(10.0, f64::max)
                    .ceil(),
            );
            grid_y(ui, &f, lo, hi, ((hi - lo) / 4.0).max(1.0).round(), "");
            let p = ui.painter_at(f.plot);
            let bw = (f.plot.width() / (k1 - k0 + 1.0) as f32).max(1.0);
            for &(k, v) in &ch.snr_profile {
                let x = map(f64::from(k), k0, k1, f.plot.left(), f.plot.right());
                let y = map(v.clamp(lo, hi), lo, hi, f.plot.bottom(), f.plot.top());
                p.rect_filled(
                    Rect::from_min_max(pos2(x - bw * 0.4, y), pos2(x + bw * 0.4, f.plot.bottom())),
                    0.0,
                    VFO,
                );
            }
            x_label(ui, &f, "carrier");
        }
    });
    ui.add_space(4.0);
    // Fading map: |H|² of the last symbols, carrier across, time down (newest at top).
    let f = frame(
        ui,
        vec2(2.0 * w + 8.0, h),
        "Fading: |H|² over the last symbols",
    );
    let rows = &ch.chan_rows;
    if let Some(first) = rows.first() {
        let cols = first.len();
        let all = rows.iter().flatten().map(|&v| f64::from(v));
        let (lo, hi) = range(all, 20.0);
        let mut px = Vec::with_capacity(cols * rows.len());
        for r in rows.iter().rev() {
            for &v in r {
                let t = ((f64::from(v) - lo) / (hi - lo)).clamp(0.0, 1.0);
                px.push(palette[(t * 255.0) as usize]);
            }
        }
        let img = egui::ColorImage::new([cols, rows.len()], px);
        let tex = match fading {
            Some(t) => {
                t.set(img, egui::TextureOptions::NEAREST);
                t
            }
            None => fading.insert(ui.ctx().load_texture(
                "fading",
                img,
                egui::TextureOptions::NEAREST,
            )),
        };
        ui.painter_at(f.rect).image(
            tex.id(),
            f.plot,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        x_label(
            ui,
            &f,
            &format!(
                "carrier {k0:.0} … {k1:.0};  {} symbols ({:.1} s), newest at the top",
                rows.len(),
                rows.len() as f64 * ch.symbol_s
            ),
        );
    }
}
