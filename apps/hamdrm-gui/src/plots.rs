//! Constellations and the plot frame helpers, drawn with the painter (no plotting
//! crate). The channel tab itself is in `chantab.rs`.

use crate::theme::*;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};

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

pub struct Frame {
    pub rect: Rect,
    pub plot: Rect,
}

pub fn frame(ui: &mut Ui, size: egui::Vec2, title: &str) -> Frame {
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

pub fn grid_y(ui: &Ui, f: &Frame, lo: f64, hi: f64, step: f64, unit: &str) {
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

pub fn map(v: f64, a: f64, b: f64, pa: f32, pb: f32) -> f32 {
    if (b - a).abs() < 1e-12 {
        return pa;
    }
    pa + ((v - a) / (b - a)) as f32 * (pb - pa)
}

pub fn polyline(ui: &Ui, f: &Frame, pts: Vec<Pos2>, color: Color32) {
    ui.painter_at(f.plot.expand(1.0))
        .add(egui::Shape::line(pts, Stroke::new(1.3, color)));
}
