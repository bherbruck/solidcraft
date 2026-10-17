//! Floating controls reserve drawing space without changing rendering or picking projection.
use egui::{Rect, pos2, vec2};
use solidcraft_engine::geom::{Aabb3, Vec3};
use solidcraft_engine::render::Camera;

#[derive(Clone, Debug)]
pub(crate) struct Layout {
    pub viewport: Rect,
    pub drawable: Rect,
    pub content: Rect,
    pub axis_view: Rect,
    pub navigation: Rect,
}

/// Enumerate rectangles whose edges meet the viewport or a bounded number of overlays.
fn free_rect(view: Rect, obstacles: &[Rect]) -> Rect {
    let mut xs = vec![view.left(), view.right()];
    let mut ys = vec![view.top(), view.bottom()];
    let obstacles: Vec<_> = obstacles.iter().take(16).map(|r| r.intersect(view)).filter(|r| r.is_positive() && r.is_finite()).collect();
    for r in &obstacles {
        xs.extend([r.left(), r.right()]);
        ys.extend([r.top(), r.bottom()]);
    }
    xs.sort_by(f32::total_cmp);
    ys.sort_by(f32::total_cmp);
    let mut best = Rect::from_min_size(view.min, vec2(0.0, 0.0));
    for (i, &left) in xs.iter().enumerate() {
        for &right in xs.iter().skip(i + 1) {
            for (j, &top) in ys.iter().enumerate() {
                for &bottom in ys.iter().skip(j + 1) {
                    let r = Rect::from_min_max(pos2(left, top), pos2(right, bottom));
                    if r.area() > best.area().max(0.0) && !obstacles.iter().any(|o| r.intersect(*o).is_positive()) {
                        best = r;
                    }
                }
            }
        }
    }
    best
}

impl Layout {
    pub fn new(viewport: Rect, docks: &[Rect], scale: f32, style: crate::prefs::NavigatorStyle) -> Self {
        Self::with_content(viewport, viewport, docks, scale, style)
    }

    pub fn with_content(viewport: Rect, content: Rect, docks: &[Rect], scale: f32, style: crate::prefs::NavigatorStyle) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let mut obstacles: Vec<_> = docks.iter().take(14).copied().filter(|r| r.is_finite()).map(|r| r.expand(4.0)).collect();
        // Navigation is anchored to the viewport, never to transient dialog space.
        let available = content;
        let cube_size = super::viewport::axis_view_size(style);
        let mut axis_view = if available.width() >= cube_size && available.height() >= cube_size {
            Rect::from_min_size(pos2(available.right() - cube_size, available.top()), vec2(cube_size, cube_size))
        } else {
            Rect::NOTHING
        };
        let nav_space = content;
        let columns = ((nav_space.width() - 10.0) / 30.0).floor().clamp(1.0, 10.0);
        let nav_height = (10.0 / columns).ceil() * 30.0;
        let navigation = if nav_space.width() >= columns * 30.0 + 10.0 && nav_space.height() >= nav_height + 7.0 {
            Rect::from_center_size(pos2(nav_space.center().x, nav_space.bottom() - nav_height * 0.5 - 7.0), vec2(columns * 30.0 + 10.0, nav_height))
        } else {
            Rect::NOTHING
        };
        // On tiny viewports keep the bottom controls usable rather than overlapping them.
        // This decision depends only on viewport size, never on an overlay.
        if axis_view.intersect(navigation).is_positive() {
            let compact = (navigation.top() - content.top() - 4.0).min(cube_size);
            axis_view = if style == crate::prefs::NavigatorStyle::Cube && compact >= 96.0 {
                Rect::from_min_size(pos2(content.right() - compact, content.top()), vec2(compact, compact))
            } else {
                Rect::NOTHING
            };
        }
        if axis_view.is_positive() {
            obstacles.push(axis_view);
        }
        if navigation.is_positive() {
            obstacles.push(navigation);
        }
        let drawable = free_rect(viewport, &obstacles).shrink(16.0 / scale);
        Self { viewport, drawable, content, axis_view, navigation }
    }
}

pub(crate) fn fit_bounds(mut cam: Camera, bounds: &Aabb3, layout: &Layout, perspective: bool) -> Option<Camera> {
    if bounds.is_empty()
        || !bounds.min.is_finite()
        || !bounds.max.is_finite()
        || !cam.is_valid()
        || !layout.viewport.is_positive()
        || !layout.drawable.is_positive()
        || !layout.drawable.is_finite()
    {
        return None;
    }
    let original_fov = cam.fov;
    cam.fov = if perspective { 0.6 } else { 0.0 };
    let (r, u, b) = cam.basis();
    let centre = bounds.center();
    let mut hx: f64 = 0.0;
    let mut hy: f64 = 0.0;
    let mut depth: f64 = 0.0;
    let corners: Vec<_> = [bounds.min.x, bounds.max.x]
        .into_iter()
        .flat_map(|x| {
            [bounds.min.y, bounds.max.y].into_iter().flat_map(move |y| [bounds.min.z, bounds.max.z].into_iter().map(move |z| Vec3::new(x, y, z)))
        })
        .collect();
    for p in &corners {
        let d = *p - centre;
        hx = hx.max(d.dot(r).abs());
        hy = hy.max(d.dot(u).abs());
        depth = depth.max(d.dot(b).abs());
    }
    let h = f64::from(layout.viewport.height());
    let hh = (hx * h / f64::from(layout.drawable.width())).max(hy * h / f64::from(layout.drawable.height())).max(0.5);
    let offset = layout.drawable.center() - layout.viewport.center();
    let tangent = if perspective { (0.3_f64).tan() } else { 0.4 };
    cam.distance = hh / tangent + if perspective { depth } else { 0.0 };
    // Offset the target so the model projects into the free rectangle in the original viewport.
    // Perspective needs a bounded correction because corners have different depths.
    for _ in 0..16 {
        let k = 2.0 * cam.half_height() / h;
        cam.target = centre - r * (f64::from(offset.x) * k) + u * (f64::from(offset.y) * k);
        let radius = (bounds.diagonal() * 0.5 + centre.dist(cam.target)).max(cam.half_height() * 2.0).max(10.0);
        let fits = corners.iter().all(|p| {
            cam.to_screen(*p, f64::from(layout.viewport.width()), h, radius).is_some_and(|(x, y, z)| {
                x.is_finite()
                    && y.is_finite()
                    && (0.0..=1.0).contains(&z)
                    && layout.drawable.contains(pos2(layout.viewport.left() + x as f32, layout.viewport.top() + y as f32))
            })
        });
        if fits {
            cam.fov = original_fov;
            return cam.is_valid().then_some(cam);
        }
        cam.distance *= 1.15;
    }
    None
}

#[cfg(test)]
#[path = "viewport_layout_tests.rs"]
mod tests;
