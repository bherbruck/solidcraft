//! Collected glyph artwork. Caption geometry adapted from VectorCraft (MIT OR Apache-2.0).
use crate::titlebar::Caption;
use egui::{Color32, Pos2, Rect, Stroke, Ui, pos2, vec2};

pub(crate) fn transport_glyph(p: &egui::Painter, c: egui::Pos2, what: &str, col: Color32) {
    let tri = |dir: f32, dx: f32| {
        let pts = vec![pos2(c.x + dx - 4.0 * dir, c.y - 5.0), pos2(c.x + dx + 4.0 * dir, c.y), pos2(c.x + dx - 4.0 * dir, c.y + 5.0)];
        p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
    };
    let bar = |dx: f32| p.line_segment([pos2(c.x + dx, c.y - 5.0), pos2(c.x + dx, c.y + 5.0)], Stroke::new(1.6, col));
    match what {
        "first" => {
            bar(-5.0);
            tri(-1.0, 1.0);
        }
        "back" => tri(-1.0, 0.0),
        "forward" => tri(1.0, 0.0),
        _ => {
            tri(1.0, -1.0);
            bar(5.0);
        }
    }
}

pub(crate) fn paint_glyph(ui: &Ui, c: Caption, maximized: bool, at: egui::Pos2, s: Stroke) {
    let p = ui.painter();
    let g = Rect::from_center_size(at, vec2(10.0, 10.0));
    match c {
        Caption::Minimize => {
            p.line_segment([g.left_center(), g.right_center()], s);
        }
        // Restore: a front square with the corner of the one behind it.
        Caption::Maximize if maximized => {
            let front = Rect::from_min_max(g.min + vec2(0.0, 2.0), g.max - vec2(2.0, 0.0));
            p.rect_stroke(front, 0.0, s, egui::StrokeKind::Middle);
            p.line(vec![g.min + vec2(2.0, 2.0), g.min + vec2(2.0, 0.0), g.right_top(), g.max - vec2(0.0, 2.0), g.max - vec2(2.0, 2.0)], s);
        }
        Caption::Maximize => {
            p.rect_stroke(g, 0.0, s, egui::StrokeKind::Middle);
        }
        Caption::Close => {
            p.line_segment([g.left_top(), g.right_bottom()], s);
            p.line_segment([g.right_top(), g.left_bottom()], s);
        }
    }
}

pub(crate) fn badge(p: &egui::Painter, r: Rect, name: &str, c: Color32) {
    let s = Stroke::new(1.3, c);
    match name {
        "lock" => {
            let body = Rect::from_min_max(pos2(r.left() + 1.5, r.center().y - 0.5), pos2(r.right() - 1.5, r.bottom() - 0.5));
            p.rect_filled(body, 1.5, c);
            let top = pos2(r.center().x, r.top() + 4.0);
            p.add(egui::Shape::line(
                (0..=8)
                    .map(|k| {
                        let a = std::f32::consts::PI * (k as f32 / 8.0);
                        pos2(top.x - 3.2 * a.cos(), top.y - 3.2 * a.sin() + 1.0)
                    })
                    .chain([pos2(top.x + 3.2, body.top())])
                    .collect(),
                s,
            ));
            p.line_segment([pos2(top.x - 3.2, top.y + 1.0), pos2(top.x - 3.2, body.top())], s);
        }
        "pin" => {
            let head = pos2(r.center().x + 2.0, r.top() + 4.0);
            p.circle_filled(head, 3.2, c);
            p.line_segment([head, pos2(r.left() + 1.5, r.bottom() - 1.0)], Stroke::new(1.6, c));
        }
        _ => {}
    }
}

pub(crate) fn pin_icon(p: &egui::Painter, r: Rect, on: bool, col: Color32) {
    let c = r.center();
    let (sin, cos) = if on { (0.0, 1.0) } else { std::f32::consts::FRAC_PI_4.sin_cos() };
    // Local coordinates: the pin points down, its head on top.
    let at = |x: f32, y: f32| c + vec2(x * cos - y * sin, x * sin + y * cos);
    let head = vec![at(-2.5, -7.0), at(2.5, -7.0), at(3.0, -2.0), at(-3.0, -2.0)];
    let stroke = Stroke::new(1.4, col);
    if on {
        p.add(egui::Shape::convex_polygon(head, col, stroke));
    } else {
        p.add(egui::Shape::closed_line(head, stroke));
    }
    p.line_segment([at(-5.5, -2.0), at(5.5, -2.0)], stroke);
    p.line_segment([at(0.0, -2.0), at(0.0, 7.0)], stroke);
}

pub(crate) fn brand_mark(p: &egui::Painter, c: Pos2) {
    let s = 20.0 / 412.217;
    let m = |pts: &[(f32, f32)]| -> Vec<egui::Pos2> { pts.iter().map(|&(x, y)| pos2(c.x + (x - 178.5) * s, c.y + (y - 206.1) * s)).collect() };
    // egui only fills convex shapes, so each face is a few overlapping convex pieces.
    let face = |col: Color32, pts: &[(f32, f32)]| {
        p.add(egui::Shape::convex_polygon(m(pts), col, Stroke::NONE));
    };
    // The faces are separated by gaps that show the application bar, not by white lines.
    let [light, mid, dark, _] = crate::theme::Tokens::get().logo;
    let edge_col = crate::theme::Tokens::get().app_bar;
    face(light, &[(3.5, 105.9), (177.0, 5.8), (263.4, 55.6), (263.8, 156.0), (177.0, 406.4)]);
    face(light, &[(177.0, 206.1), (263.8, 156.0), (350.5, 206.2), (350.5, 306.3), (177.0, 406.4)]);
    face(mid, &[(3.5, 105.9), (177.0, 206.1), (177.0, 406.4)]);
    face(mid, &[(3.5, 105.9), (3.5, 205.7), (90.3, 256.2), (177.0, 206.1)]);
    face(mid, &[(177.0, 206.1), (90.3, 256.2), (90.0, 356.2), (177.0, 406.4)]);
    face(dark, &[(264.1, 155.8), (177.0, 206.1), (177.0, 406.4), (263.8, 256.2)]);
    face(dark, &[(263.8, 256.2), (350.5, 206.2), (350.5, 306.3), (177.0, 406.4)]);
    face(dark, &[(177.0, 206.1), (177.0, 406.4), (350.5, 306.3)]);
    face(dark, &[(263.8, 55.9), (263.8, 156.0), (177.1, 105.9)]);
    // White edge lines.
    let edge = Stroke::new(10.0 * s, edge_col);
    let line = |pts: &[(f32, f32)]| {
        p.add(egui::Shape::line(m(pts), edge));
    };
    line(&[(263.8, 55.9), (177.1, 105.9), (263.8, 156.0), (263.8, 256.2), (350.6, 206.1)]);
    line(&[(5.0, 105.9), (178.5, 206.1), (178.5, 406.4)]);
    line(&[(177.0, 206.1), (263.8, 156.0)]);
    p.add(egui::Shape::closed_line(
        m(&[
            (178.5, 5.8),
            (5.0, 105.9),
            (5.0, 206.1),
            (91.7, 256.2),
            (91.7, 356.4),
            (178.5, 406.4),
            (352.0, 306.3),
            (352.0, 206.1),
            (265.2, 156.0),
            (265.2, 55.9),
        ]),
        edge,
    ));
}

pub(crate) fn cross(p: &egui::Painter, r: Rect, s: Stroke) {
    p.line_segment([r.left_top(), r.right_bottom()], s);
    p.line_segment([r.right_top(), r.left_bottom()], s);
}
pub(crate) fn radial_centre(p: &egui::Painter, c: Pos2, direction: Option<(egui::Vec2, bool)>, t: crate::theme::Tokens) {
    p.circle(c, 15.0, t.panel.gamma_multiply(0.92), Stroke::new(1.0, t.border));
    if let Some((d, enabled)) = direction {
        p.line_segment([c + d * 6.0, c + d * 15.0], Stroke::new(3.0, if enabled { t.accent } else { t.border }));
    } else {
        p.circle_filled(c, 3.0, t.text_dim);
    }
}

pub(crate) fn disclosure(p: &egui::Painter, ar: Rect, open: bool, ink: Color32) {
    let pts = if open {
        vec![pos2(ar.left() + 2.0, ar.top() + 4.0), pos2(ar.right() - 2.0, ar.top() + 4.0), pos2(ar.center().x, ar.bottom() - 3.0)]
    } else {
        vec![pos2(ar.left() + 4.0, ar.top() + 2.0), pos2(ar.right() - 3.0, ar.center().y), pos2(ar.left() + 4.0, ar.bottom() - 2.0)]
    };
    p.add(egui::Shape::convex_polygon(pts, ink, Stroke::NONE));
}
