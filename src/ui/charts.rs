//! Hand-painted charts: a line chart with hover read-out, a charge ring and
//! a stacked bar.

use super::theme::{pal, regular, semibold};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, Ui,
};

/// One line of a chart.
pub struct Series<'a> {
    pub points: &'a [(f64, f32)],
    pub color: Color32,
    /// Fill the area under the line.
    pub fill: bool,
}

/// What the axes show.
pub struct Axes<'a> {
    pub x_range: (f64, f64),
    pub y_range: (f32, f32),
    /// Values with a label on the y axis.
    pub y_ticks: &'a [f32],
    pub y_label: &'a dyn Fn(f32) -> String,
    /// Positions with a label on the x axis.
    pub x_ticks: &'a [(f64, String)],
    /// Hover read-out: (x, y of the first series) -> text.
    pub hover: &'a dyn Fn(f64, f32) -> String,
    /// A horizontal reference line (e.g. 0 W or 80 %).
    pub reference: Option<(f32, &'a str)>,
}

/// Nice tick values between lo and hi (about `n` of them).
pub fn ticks(lo: f32, hi: f32, n: usize) -> Vec<f32> {
    let span = (hi - lo).max(1e-3);
    let raw = span / n.max(1) as f32;
    let mag = 10f32.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(raw);
    let mut v = (lo / step).ceil() * step;
    let mut out = Vec::new();
    while v <= hi + step * 1e-3 && out.len() < 20 {
        out.push(if v.abs() < step * 1e-3 { 0.0 } else { v });
        v += step;
    }
    out
}

pub fn line_chart(ui: &mut Ui, id: &str, height: f32, series: &[Series], axes: &Axes) {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect.expand(2.0));
    let plot = Rect::from_min_max(
        pos2(rect.left() + 46.0, rect.top() + 6.0),
        pos2(rect.right() - 8.0, rect.bottom() - 22.0),
    );
    let (x0, x1) = axes.x_range;
    let (y0, y1) = axes.y_range;
    let span_x = (x1 - x0).max(1e-9);
    let span_y = (y1 - y0).max(1e-6);
    let to_screen = |x: f64, y: f32| {
        pos2(
            plot.left() + ((x - x0) / span_x) as f32 * plot.width(),
            plot.bottom() - (y - y0) / span_y * plot.height(),
        )
    };
    for &t in axes.y_ticks {
        let y = to_screen(x0, t).y;
        painter.hline(
            plot.x_range(),
            y,
            Stroke::new(1.0_f32, p.border.gamma_multiply(0.7)),
        );
        painter.text(
            pos2(plot.left() - 8.0, y),
            Align2::RIGHT_CENTER,
            (axes.y_label)(t),
            regular(11.5),
            p.faint,
        );
    }
    let last = axes.x_ticks.len().saturating_sub(1);
    for (i, (x, label)) in axes.x_ticks.iter().enumerate() {
        let pt = to_screen(*x, y0);
        let anchor = if i == 0 && pt.x - plot.left() < 30.0 {
            Align2::LEFT_TOP
        } else if i == last && plot.right() - pt.x < 30.0 {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        painter.text(
            pos2(pt.x, plot.bottom() + 6.0),
            anchor,
            label,
            regular(11.5),
            p.faint,
        );
    }
    if let Some((v, label)) = axes.reference {
        if (y0..=y1).contains(&v) {
            let y = to_screen(x0, v).y;
            painter.add(Shape::dashed_line(
                &[pos2(plot.left(), y), pos2(plot.right(), y)],
                Stroke::new(1.0_f32, p.muted),
                4.0,
                4.0,
            ));
            painter.text(
                pos2(plot.right() - 4.0, y - 3.0),
                Align2::RIGHT_BOTTOM,
                label,
                regular(11.0),
                p.muted,
            );
        }
    }
    for s in series {
        if s.points.len() < 2 {
            continue;
        }
        let pts: Vec<Pos2> = s
            .points
            .iter()
            .filter(|(x, _)| *x >= x0 && *x <= x1)
            .map(|(x, y)| to_screen(*x, y.clamp(y0, y1)))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        if s.fill {
            let base = to_screen(x0, y0.max(0.0).min(y1)).y;
            let fill = s.color.gamma_multiply(0.14);
            for w in pts.windows(2) {
                let quad = vec![w[0], w[1], pos2(w[1].x, base), pos2(w[0].x, base)];
                painter.add(Shape::convex_polygon(quad, fill, Stroke::NONE));
            }
        }
        painter.add(Shape::line(pts, Stroke::new(2.0_f32, s.color)));
    }
    // Hover: a vertical line and the nearest value of the first series.
    if let (Some(pos), Some(first)) = (resp.hover_pos(), series.first()) {
        if plot.contains(pos) && !first.points.is_empty() {
            let x = x0 + ((pos.x - plot.left()) / plot.width()) as f64 * span_x;
            let nearest = first
                .points
                .iter()
                .min_by(|a, b| (a.0 - x).abs().total_cmp(&(b.0 - x).abs()));
            if let Some(&(nx, ny)) = nearest {
                let pt = to_screen(nx, ny.clamp(y0, y1));
                painter.vline(pt.x, plot.y_range(), Stroke::new(1.0_f32, p.faint));
                painter.circle_filled(pt, 4.0, first.color);
                let text = (axes.hover)(nx, ny);
                let galley = painter.layout_no_wrap(text, semibold(12.5), p.text);
                let w = galley.size().x + 16.0;
                let mut bx = pt.x + 10.0;
                if bx + w > rect.right() {
                    bx = pt.x - 10.0 - w;
                }
                let tip = Rect::from_min_size(pos2(bx, plot.top() + 4.0), vec2(w, 24.0));
                painter.rect_filled(tip, CornerRadius::same(6), p.card);
                painter.rect_stroke(
                    tip,
                    CornerRadius::same(6),
                    Stroke::new(1.0_f32, p.border),
                    egui::StrokeKind::Inside,
                );
                painter.galley(
                    pos2(tip.left() + 8.0, tip.center().y - galley.size().y / 2.0),
                    galley,
                    p.text,
                );
            }
        }
    }
    let _ = id;
}

/// A ring showing `fraction` (0..=1) with big text in the middle.
pub fn ring(ui: &mut Ui, size: f32, fraction: f32, color: Color32, big: &str, small: &str) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let c = rect.center();
    let r = size / 2.0 - 9.0;
    let painter = ui.painter();
    painter.circle_stroke(c, r, Stroke::new(12.0_f32, p.hover));
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let n = (f * 90.0).ceil().max(2.0) as usize;
        let start = -std::f32::consts::FRAC_PI_2;
        let pts: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = start + f * std::f32::consts::TAU * i as f32 / n as f32;
                c + vec2(a.cos(), a.sin()) * r
            })
            .collect();
        painter.add(Shape::line(pts, Stroke::new(12.0_f32, color)));
        // Round caps.
        let a0 = start;
        let a1 = start + f * std::f32::consts::TAU;
        painter.circle_filled(c + vec2(a0.cos(), a0.sin()) * r, 6.0, color);
        painter.circle_filled(c + vec2(a1.cos(), a1.sin()) * r, 6.0, color);
    }
    painter.text(
        c - vec2(0.0, 8.0),
        Align2::CENTER_CENTER,
        big,
        semibold(size * 0.2),
        p.text,
    );
    painter.text(
        c + vec2(0.0, size * 0.13),
        Align2::CENTER_CENTER,
        small,
        regular(12.5),
        p.muted,
    );
}

/// A horizontal bar split into coloured parts (fractions summing to <= 1).
pub fn stacked_bar(ui: &mut Ui, parts: &[(f32, Color32)], height: f32) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let r = CornerRadius::same((height / 2.0) as u8);
    ui.painter().rect_filled(rect, r, p.hover);
    let mut x = rect.left();
    let total: f32 = parts.iter().map(|(f, _)| f.max(0.0)).sum::<f32>().max(1.0);
    for (i, (f, color)) in parts.iter().enumerate() {
        let w = rect.width() * f.max(0.0) / total;
        if w < 0.5 {
            continue;
        }
        let part = Rect::from_min_size(pos2(x, rect.top()), vec2(w, height));
        let radius = if i == 0 || i == parts.len() - 1 {
            r
        } else {
            CornerRadius::ZERO
        };
        ui.painter().rect_filled(part, radius, *color);
        x += w;
    }
}

/// A legend entry: dot, label, value.
pub fn legend(ui: &mut Ui, color: Color32, label: &str, value: &str) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 18.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 4.5, color);
        ui.label(egui::RichText::new(label).color(p.muted).size(13.0));
        ui.label(egui::RichText::new(value).font(semibold(13.0)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_ticks() {
        assert_eq!(ticks(0.0, 100.0, 4), vec![0.0, 25.0, 50.0, 75.0, 100.0]);
        assert_eq!(ticks(-12.0, 20.0, 4), vec![-10.0, 0.0, 10.0, 20.0]);
        assert!(ticks(80.0, 100.0, 4).contains(&90.0));
        assert!(!ticks(0.0, 0.0, 4).is_empty());
    }
}
