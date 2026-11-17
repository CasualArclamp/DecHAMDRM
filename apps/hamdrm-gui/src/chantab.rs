//! The channel tab: a row of figures, then the channel response, group delay, impulse
//! response and per-carrier SNR, then the fading map (in the manner of DecDRM's
//! channel tab). Frequency axes are audio Hz; SNR is coloured against what the mode
//! needs.

use crate::plots::{Frame, frame, grid_y, map, polyline};
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use hamdrm_engine::{ChainVisuals, RxStatus};

/// Frequency ticks every 500 Hz along the bottom of a plot whose x axis runs from
/// `f0` to `f1` Hz of audio.
fn grid_hz(ui: &Ui, f: &Frame, f0: f64, f1: f64) {
    let p = ui.painter_at(f.rect);
    let mut v = (f0 / 500.0).ceil() * 500.0;
    while v <= f1 {
        let x = map(v, f0, f1, f.plot.left(), f.plot.right());
        p.line_segment(
            [pos2(x, f.plot.top()), pos2(x, f.plot.bottom())],
            Stroke::new(1.0, GRID),
        );
        p.text(
            pos2(x, f.plot.bottom() + 2.0),
            Align2::CENTER_TOP,
            format!("{v:.0}"),
            FontId::proportional(9.5),
            FAINT,
        );
        v += 500.0;
    }
    p.text(
        pos2(f.plot.right(), f.rect.top() + 3.0),
        Align2::RIGHT_TOP,
        "Hz",
        FontId::proportional(9.5),
        FAINT,
    );
}

/// Fill between a curve and `base_y` (a triangle strip: egui's polygons must be convex).
fn area(ui: &Ui, f: &Frame, pts: &[Pos2], base_y: f32, color: Color32) {
    if pts.len() < 2 {
        return;
    }
    let mut mesh = egui::Mesh::default();
    for (i, q) in pts.iter().enumerate() {
        mesh.colored_vertex(*q, color);
        mesh.colored_vertex(pos2(q.x, base_y), color.gamma_multiply(0.15));
        if i > 0 {
            let n = (2 * i) as u32;
            mesh.add_triangle(n - 2, n - 1, n);
            mesh.add_triangle(n - 1, n, n + 1);
        }
    }
    ui.painter_at(f.plot).add(egui::Shape::mesh(mesh));
}

/// Catmull-Rom interpolation of evenly spaced values, `sub` points per interval.
fn smooth(v: &[f64], sub: usize) -> Vec<f64> {
    if v.len() < 3 {
        return v.to_vec();
    }
    let at = |i: isize| v[i.clamp(0, v.len() as isize - 1) as usize];
    let mut out = Vec::with_capacity(v.len() * sub);
    for i in 0..v.len() - 1 {
        let i = i as isize;
        let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        for j in 0..sub {
            let t = j as f64 / sub as f64;
            out.push(
                0.5 * (2.0 * p1
                    + (p2 - p0) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
                    + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t * t * t),
            );
        }
    }
    out.push(v[v.len() - 1]);
    out
}

/// One figure of the summary row: a small caption over a large value.
fn tile(ui: &mut Ui, w: f32, caption: &str, value: String, color: Color32, hint: &str) {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, WELL);
    p.rect_filled(
        Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
        2.0,
        color,
    );
    p.text(
        rect.left_top() + vec2(10.0, 4.0),
        Align2::LEFT_TOP,
        caption,
        FontId::proportional(10.5),
        DIM,
    );
    p.text(
        rect.left_bottom() + vec2(10.0, -4.0),
        Align2::LEFT_BOTTOM,
        value,
        FontId::monospace(17.0),
        WHITE,
    );
    resp.on_hover_text(hint);
}

fn health(bad: bool, warn: bool) -> Color32 {
    if bad {
        BAD
    } else if warn {
        WARN
    } else {
        GOOD
    }
}

/// Small grey text on a plot.
fn note(p: &egui::Painter, at: Pos2, align: Align2, text: impl ToString) {
    p.text(at, align, text, FontId::proportional(9.5), FAINT);
}

/// `need` is the SNR the mode needs, dB.
pub fn show(
    ui: &mut Ui,
    ch: &ChainVisuals,
    st: &RxStatus,
    need: Option<f64>,
    fading: &mut Option<egui::TextureHandle>,
    palette: &[Color32],
) {
    if ch.chan.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No channel estimate yet").color(FAINT))
        });
        return;
    }
    let gap = 6.0;
    let k0 = f64::from(ch.kmin);
    let k1 = k0 + ch.chan.len() as f64 - 1.0;
    // Audio frequency of carrier k.
    let dc = st.dc_frequency_hz.unwrap_or(350.0);
    let hz = |k: f64| dc + k * ch.spacing_hz;
    let (f0, f1) = (hz(k0), hz(k1));
    let db: Vec<f64> = ch
        .chan
        .iter()
        .map(|z| 10.0 * z.norm_sqr().max(1e-12).log10())
        .collect();
    let mean_db = db.iter().sum::<f64>() / db.len() as f64;
    let rel: Vec<f64> = db.iter().map(|v| v - mean_db).collect();
    let ripple =
        rel.iter().cloned().fold(f64::MIN, f64::max) - rel.iter().cloned().fold(f64::MAX, f64::min);
    let worst = ch.snr_profile.iter().map(|p| p.1).fold(f64::MAX, f64::min);

    // ---- the figures
    let tw = (ui.available_width() - 4.0 * gap) / 5.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let snr = st.snr_db;
        tile(
            ui,
            tw,
            "SNR in 2.5 kHz",
            snr.map_or("–".into(), |s| format!("{s:.1} dB")),
            match (snr, need) {
                (Some(s), Some(n)) => health(s < n, s < n + 3.0),
                _ => FAINT,
            },
            "Signal to noise over the whole signal, in a 2.5 kHz bandwidth.",
        );
        tile(
            ui,
            tw,
            "Weakest carrier",
            if worst < f64::MAX {
                format!("{worst:.1} dB")
            } else {
                "–".into()
            },
            match (worst < f64::MAX, need) {
                (true, Some(n)) => health(worst < n - 6.0, worst < n),
                _ => FAINT,
            },
            "SNR of the worst carrier: a notch from selective fading or an interfering \
             tone shows here first.",
        );
        tile(
            ui,
            tw,
            "Ripple",
            format!("{ripple:.1} dB"),
            health(ripple > 20.0, ripple > 8.0),
            "Strongest carrier against the weakest: 0 dB is a flat channel, tens of dB is \
             deep selective fading.",
        );
        let spread = ch
            .pds_axis
            .as_ref()
            .map(|a| (a.pds_end_ms - a.pds_begin_ms, a.guard_ms.1 - a.guard_ms.0));
        tile(
            ui,
            tw,
            "Echo spread",
            spread.map_or("–".into(), |(s, _)| format!("{s:.2} ms")),
            match spread {
                Some((s, g)) => health(s > g, s > 0.7 * g),
                None => FAINT,
            },
            "Span of the echoes. It has to fit in the guard interval, or symbols run into \
             each other.",
        );
        tile(
            ui,
            tw,
            "Doppler",
            format!("{:.2} Hz", st.doppler_hz),
            health(st.doppler_hz > 3.0, st.doppler_hz > 1.0),
            "How fast the channel changes (fading rate).",
        );
    });
    ui.add_space(gap);

    let w = (ui.available_width() - gap) / 2.0;
    let h = ((ui.available_height() - 2.0 * gap) / 3.0).max(90.0);
    let x_of = |f: &Frame, k: f64| map(hz(k), f0, f1, f.plot.left(), f.plot.right());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        // |H|² per carrier, against its mean.
        let f = frame(ui, vec2(w, h), "Channel response (dB against the mean)");
        let half = (ripple * 0.75).max(6.0);
        let (lo, hi) = (-half, half);
        grid_hz(ui, &f, f0, f1);
        grid_y(ui, &f, lo, hi, (half / 2.0).max(1.0).round(), "");
        let pts: Vec<Pos2> = rel
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                pos2(
                    x_of(&f, k0 + i as f64),
                    map(v.clamp(lo, hi), lo, hi, f.plot.bottom(), f.plot.top()),
                )
            })
            .collect();
        area(ui, &f, &pts, f.plot.bottom(), TRACE.gamma_multiply(0.35));
        polyline(ui, &f, pts, TRACE);

        // Group delay against its median.
        let f = frame(ui, vec2(w, h), "Group delay (ms against the median)");
        let mut gd: Vec<f64> = ch
            .chan
            .windows(2)
            .map(|p| {
                -(p[1] * p[0].conj()).arg() / (2.0 * std::f64::consts::PI * ch.spacing_hz.max(1.0))
                    * 1e3
            })
            .collect();
        let mut sorted = gd.clone();
        sorted.sort_by(f64::total_cmp);
        let med = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
        for v in &mut gd {
            *v -= med;
        }
        let half = gd.iter().fold(0.5_f64, |a, v| a.max(v.abs() * 1.2));
        let (lo, hi) = (-half, half);
        grid_hz(ui, &f, f0, f1);
        grid_y(ui, &f, lo, hi, half / 2.0, "");
        let pts = gd
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                pos2(
                    x_of(&f, k0 + i as f64 + 0.5),
                    map(v, lo, hi, f.plot.bottom(), f.plot.top()),
                )
            })
            .collect();
        polyline(ui, &f, pts, AMBER);
    });
    ui.add_space(gap);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        // Impulse response (power delay profile), dB rel. peak, guard interval shaded.
        let f = frame(ui, vec2(w, h), "Impulse response (dB against the peak)");
        if let (Some(ax), false) = (&ch.pds_axis, ch.pds.is_empty()) {
            const SUB: usize = 8;
            let peak = ch.pds.iter().cloned().fold(1e-30, f64::max);
            let x0 = ax.start_ms;
            let x1 = ax.start_ms + ax.step_ms * (ch.pds.len() as f64 - 1.0);
            let xm = |ms: f64| map(ms, x0, x1, f.plot.left(), f.plot.right());
            let p = ui.painter_at(f.rect);
            let (g0, g1) = ax.guard_ms;
            p.rect_filled(
                Rect::from_x_y_ranges(xm(g0)..=xm(g1), f.plot.y_range()),
                0.0,
                Color32::from_rgba_unmultiplied(80, 200, 255, 18),
            );
            note(
                &p,
                pos2(xm(g1) - 3.0, f.plot.top() + 2.0),
                Align2::RIGHT_TOP,
                "guard interval",
            );
            grid_y(ui, &f, -40.0, 0.0, 10.0, "");
            // Delay ticks every millisecond (every 2 ms when crowded).
            let step = if x1 - x0 > 12.0 { 2.0 } else { 1.0 };
            let mut t = (x0 / step).ceil() * step;
            while t <= x1 {
                p.line_segment(
                    [pos2(xm(t), f.plot.top()), pos2(xm(t), f.plot.bottom())],
                    Stroke::new(1.0, GRID),
                );
                note(
                    &p,
                    pos2(xm(t), f.plot.bottom() + 2.0),
                    Align2::CENTER_TOP,
                    format!("{t:.0}"),
                );
                t += step;
            }
            note(
                &p,
                pos2(f.plot.right(), f.rect.top() + 3.0),
                Align2::RIGHT_TOP,
                "ms",
            );
            for x in [ax.pds_begin_ms, ax.pds_end_ms] {
                p.line_segment(
                    [pos2(xm(x), f.plot.top()), pos2(xm(x), f.plot.bottom())],
                    Stroke::new(1.0, WARN.gamma_multiply(0.7)),
                );
            }
            let dbs: Vec<f64> = ch
                .pds
                .iter()
                .map(|&v| (10.0 * (v / peak).max(1e-4).log10()).max(-40.0))
                .collect();
            let pts: Vec<Pos2> = smooth(&dbs, SUB)
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    pos2(
                        xm(x0 + i as f64 * ax.step_ms / SUB as f64),
                        map(
                            v.clamp(-40.0, 0.0),
                            -40.0,
                            0.0,
                            f.plot.bottom(),
                            f.plot.top(),
                        ),
                    )
                })
                .collect();
            area(ui, &f, &pts, f.plot.bottom(), GOOD.gamma_multiply(0.3));
            polyline(ui, &f, pts, GOOD);
        }

        // Per-carrier SNR, coloured against what the mode needs.
        let f = frame(ui, vec2(w, h), "SNR per carrier (dB)");
        if !ch.snr_profile.is_empty() {
            let top = ch.snr_profile.iter().map(|p| p.1).fold(20.0, f64::max);
            let (lo, hi) = (0.0, (top / 5.0).ceil() * 5.0);
            grid_hz(ui, &f, f0, f1);
            grid_y(ui, &f, lo, hi, 5.0 * ((hi - lo) / 25.0).ceil(), "");
            let p = ui.painter_at(f.plot);
            let bw = (f.plot.width() / (k1 - k0 + 1.0) as f32).max(1.0);
            for &(k, v) in &ch.snr_profile {
                let x = x_of(&f, f64::from(k));
                let y = map(v.clamp(lo, hi), lo, hi, f.plot.bottom(), f.plot.top());
                let col = match need {
                    Some(n) if v < n - 6.0 => BAD,
                    Some(n) if v < n => WARN,
                    _ => GOOD,
                };
                p.rect_filled(
                    Rect::from_min_max(
                        pos2(x - bw * 0.36, y),
                        pos2(x + bw * 0.36, f.plot.bottom()),
                    ),
                    1.5,
                    col.gamma_multiply(0.8),
                );
            }
            if let Some(n) = need.filter(|n| *n < hi) {
                let y = map(n, lo, hi, f.plot.bottom(), f.plot.top());
                p.extend(egui::Shape::dashed_line(
                    &[pos2(f.plot.left(), y), pos2(f.plot.right(), y)],
                    Stroke::new(1.0, WHITE.gamma_multiply(0.7)),
                    5.0,
                    4.0,
                ));
                // On a dark tab so it reads over the bars.
                let font = FontId::proportional(10.0);
                let galley = p.layout_no_wrap(format!("needs {n:.1} dB"), font, TEXT);
                let tab = Rect::from_min_size(
                    pos2(f.plot.left() + 4.0, y - galley.size().y - 3.0),
                    galley.size() + vec2(8.0, 2.0),
                );
                p.rect_filled(tab, 3.0, WELL.gamma_multiply(0.9));
                p.galley(tab.min + vec2(4.0, 1.0), galley, TEXT);
            }
        }
    });
    ui.add_space(gap);

    // Fading map: |H|² of the last symbols, frequency across, time down (newest at top).
    let f = frame(
        ui,
        vec2(ui.available_width(), h),
        "Fading: channel response over time (newest at the top)",
    );
    let rows = &ch.chan_rows;
    if let Some(first) = rows.first() {
        let cols = first.len();
        // Scale about the median, at least ±6 dB, so a flat channel is not noise.
        let mut all: Vec<f32> = rows
            .iter()
            .flatten()
            .copied()
            .filter(|v| v.is_finite())
            .collect();
        all.sort_by(f32::total_cmp);
        let med = f64::from(all.get(all.len() / 2).copied().unwrap_or(0.0));
        let span = all
            .first()
            .zip(all.last())
            .map_or(6.0, |(a, b)| (f64::from(b - a) / 2.0).max(6.0));
        let (lo, hi) = (med - span, med + span);
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
                t.set(img, egui::TextureOptions::LINEAR);
                t
            }
            None => fading.insert(ui.ctx().load_texture(
                "fading",
                img,
                egui::TextureOptions::LINEAR,
            )),
        };
        // The map, a colour scale at its right.
        let map_rect = Rect::from_min_max(f.plot.min, f.plot.max - vec2(50.0, 0.0));
        let p = ui.painter_at(f.rect);
        p.image(
            tex.id(),
            map_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let mut v = (f0 / 500.0).ceil() * 500.0;
        while v <= f1 {
            note(
                &p,
                pos2(
                    map(v, f0, f1, map_rect.left(), map_rect.right()),
                    map_rect.bottom() + 2.0,
                ),
                Align2::CENTER_TOP,
                format!("{v:.0}"),
            );
            v += 500.0;
        }
        let secs = rows.len() as f64 * ch.symbol_s;
        note(
            &p,
            pos2(map_rect.left() - 3.0, map_rect.top()),
            Align2::RIGHT_TOP,
            "now",
        );
        note(
            &p,
            pos2(map_rect.left() - 3.0, map_rect.bottom()),
            Align2::RIGHT_BOTTOM,
            format!("-{secs:.1} s"),
        );
        let bar = Rect::from_min_max(
            pos2(map_rect.right() + 8.0, map_rect.top()),
            pos2(map_rect.right() + 16.0, map_rect.bottom()),
        );
        let steps = 32;
        let dy = bar.height() / steps as f32;
        for i in 0..steps {
            let t = 1.0 - i as f32 / (steps - 1) as f32;
            let y0 = bar.top() + dy * i as f32;
            p.rect_filled(
                Rect::from_min_max(pos2(bar.left(), y0), pos2(bar.right(), y0 + dy + 0.5)),
                0.0,
                palette[(t * 255.0) as usize],
            );
        }
        note(
            &p,
            pos2(bar.right() + 3.0, bar.top()),
            Align2::LEFT_TOP,
            format!("+{span:.0} dB"),
        );
        note(
            &p,
            pos2(bar.right() + 3.0, bar.bottom()),
            Align2::LEFT_BOTTOM,
            format!("-{span:.0} dB"),
        );
    }
}
