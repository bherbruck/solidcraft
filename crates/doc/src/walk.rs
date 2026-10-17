//! The walks over a feature: its expressions (`params`), its references to the rest of the design
//! (`feature_refs`) and its geometry (`feature_geometry`). `#[derive(Walk)]`
//! (`solidcraft-derive`) generates them from `#[walk(...)]` roles on the fields of `FeatureKind`
//! and the types inside it, so what a field is, is written once, where it is declared. The
//! traits here carry the walks through the types the fields have: options, lists, arrays, planes
//! and construction definitions.

pub(crate) use solidcraft_geom::Vec3;

pub(crate) use crate::ProfileSel;
use crate::construct::{AxisDef, GeoRef, PointDef};
pub(crate) use crate::expr::Kind;
pub(crate) use crate::feature_geometry::Geo;
pub(crate) use crate::feature_refs::Ref;
pub(crate) use crate::params::ExprRole;
use crate::{BodyName, Dir, Expr, FacePick, FeatureName, PlaneRef, PointExpr, Position, SketchId};

/// An expression: its label, the expression, its kind and its role.
pub(crate) type ExprItem<'a> = (&'static str, &'a Expr, Kind, ExprRole);
pub(crate) type ExprItemMut<'a> = (&'static str, &'a mut Expr, Kind, ExprRole);
pub(crate) type RefItem<'a> = Ref<&'a SketchId, &'a FeatureName, &'a BodyName>;
pub(crate) type RefItemMut<'a> = Ref<&'a mut SketchId, &'a mut FeatureName, &'a mut BodyName>;
pub(crate) type GeoItem<'a> = Geo<&'a Vec3, &'a PlaneRef, &'a GeoRef, &'a PointExpr, &'a ProfileSel>;
pub(crate) type GeoItemMut<'a> = Geo<&'a mut Vec3, &'a mut PlaneRef, &'a mut GeoRef, &'a mut PointExpr, &'a mut ProfileSel>;

/// A value's expressions, references and geometry, each walked in order. A type with none of
/// them implements it empty.
pub(crate) trait Walk {
    fn walk_exprs<'a>(&'a self, _: &mut dyn FnMut(ExprItem<'a>)) {}
    fn walk_exprs_mut<'a>(&'a mut self, _: &mut dyn FnMut(ExprItemMut<'a>)) {}
    fn walk_refs<'a>(&'a self, _: &mut dyn FnMut(RefItem<'a>)) {}
    fn walk_refs_mut<'a>(&'a mut self, _: &mut dyn FnMut(RefItemMut<'a>)) {}
    fn walk_geometry<'a>(&'a self, _: &mut dyn FnMut(GeoItem<'a>)) {}
    fn walk_geometry_mut<'a>(&'a mut self, _: &mut dyn FnMut(GeoItemMut<'a>)) {}
}

impl Walk for String {}
impl Walk for std::sync::Arc<str> {}
impl Walk for bool {}
impl Walk for u64 {}
impl Walk for i8 {}
impl Walk for f64 {}
impl Walk for usize {}
impl Walk for u32 {}
impl Walk for ProfileSel {}
impl Walk for solidcraft_sketch::Sketch {}
impl Walk for solidcraft_sketch::PatternAlignment {}
impl Walk for solidcraft_kernel::StepNode {}
impl Walk for crate::document::Operation {}
impl Walk for solidcraft_kernel::Continuity {}
impl Walk for crate::document::Direction {}
impl Walk for crate::document::FilletFit {}
impl Walk for crate::document::FilletCorner {}
impl Walk for crate::document::MeshData {}
impl Walk for crate::document::RevolveConvention {}
impl Walk for crate::document::RailFit {}
impl Walk for crate::document::CoilSection {}
impl Walk for crate::document::ThinSide {}
impl Walk for crate::document::ExtendMode {}
impl Walk for crate::document::RuledMode {}
impl Walk for crate::document::RuledCorners {}
impl Walk for crate::document::ExtrudeConvention {}

/// A reference walks itself: a sketch, a feature or a body.
macro_rules! reference {
    ($t:ty, $role:ident) => {
        impl Walk for $t {
            fn walk_refs<'a>(&'a self, f: &mut dyn FnMut(RefItem<'a>)) {
                f(Ref::$role(self))
            }
            fn walk_refs_mut<'a>(&'a mut self, f: &mut dyn FnMut(RefItemMut<'a>)) {
                f(Ref::$role(self))
            }
        }
    };
}

reference!(SketchId, Sketch);
reference!(FeatureName, Feature);
reference!(BodyName, Body);

/// A point walks itself with its role: a face pick, a position or a direction. As a plain
/// `Vec3` it also serves a field whose attribute gives it another role (`pinned`, `pick_if`).
macro_rules! point {
    ($t:ty, $role:ident) => {
        impl Walk for $t {
            fn walk_geometry<'a>(&'a self, f: &mut dyn FnMut(GeoItem<'a>)) {
                f(Geo::$role(&self.0))
            }
            fn walk_geometry_mut<'a>(&'a mut self, f: &mut dyn FnMut(GeoItemMut<'a>)) {
                f(Geo::$role(&mut self.0))
            }
        }

        impl Each<Vec3> for $t {
            fn each<'a>(&'a self, f: &mut dyn FnMut(&'a Vec3)) {
                f(&self.0)
            }
            fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(&'a mut Vec3)) {
                f(&mut self.0)
            }
        }
    };
}

point!(FacePick, Pick);
point!(Position, Pos);
point!(Dir, Dir);

/// Containers walk what they hold, in order.
macro_rules! walk_each {
    ($([$($g:tt)*] $t:ty),*) => {$(
        impl<$($g)*> Walk for $t {
            fn walk_exprs<'a>(&'a self, f: &mut dyn FnMut(ExprItem<'a>)) {
                for t in self {
                    t.walk_exprs(f)
                }
            }
            fn walk_exprs_mut<'a>(&'a mut self, f: &mut dyn FnMut(ExprItemMut<'a>)) {
                for t in self {
                    t.walk_exprs_mut(f)
                }
            }
            fn walk_refs<'a>(&'a self, f: &mut dyn FnMut(RefItem<'a>)) {
                for t in self {
                    t.walk_refs(f)
                }
            }
            fn walk_refs_mut<'a>(&'a mut self, f: &mut dyn FnMut(RefItemMut<'a>)) {
                for t in self {
                    t.walk_refs_mut(f)
                }
            }
            fn walk_geometry<'a>(&'a self, f: &mut dyn FnMut(GeoItem<'a>)) {
                for t in self {
                    t.walk_geometry(f)
                }
            }
            fn walk_geometry_mut<'a>(&'a mut self, f: &mut dyn FnMut(GeoItemMut<'a>)) {
                for t in self {
                    t.walk_geometry_mut(f)
                }
            }
        }
    )*};
}

walk_each!([T: Walk] Option<T>, [T: Walk] Vec<T>, [T: Walk, const N: usize] [T; N]);

/// The values of a field with a role (`T`, an optional one or a list), in order.
pub(crate) trait Each<T> {
    fn each<'a>(&'a self, f: &mut dyn FnMut(&'a T));
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(&'a mut T));
}

impl<T> Each<T> for T {
    fn each<'a>(&'a self, f: &mut dyn FnMut(&'a T)) {
        f(self)
    }
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(&'a mut T)) {
        f(self)
    }
}

impl<T> Each<T> for Option<T> {
    fn each<'a>(&'a self, f: &mut dyn FnMut(&'a T)) {
        if let Some(t) = self {
            f(t)
        }
    }
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(&'a mut T)) {
        if let Some(t) = self {
            f(t)
        }
    }
}

impl<T> Each<T> for Vec<T> {
    fn each<'a>(&'a self, f: &mut dyn FnMut(&'a T)) {
        for t in self {
            f(t)
        }
    }
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(&'a mut T)) {
        for t in self {
            f(t)
        }
    }
}

impl<T, const N: usize> Each<T> for [T; N] {
    fn each<'a>(&'a self, f: &mut dyn FnMut(&'a T)) {
        for t in self {
            f(t)
        }
    }
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(&'a mut T)) {
        for t in self {
            f(t)
        }
    }
}

impl<T, const N: usize> Each<T> for Option<[T; N]> {
    fn each<'a>(&'a self, f: &mut dyn FnMut(&'a T)) {
        if let Some(a) = self {
            for t in a {
                f(t)
            }
        }
    }
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(&'a mut T)) {
        if let Some(a) = self {
            for t in a {
                f(t)
            }
        }
    }
}

impl<T> Each<T> for Option<Vec<T>> {
    fn each<'a>(&'a self, f: &mut dyn FnMut(&'a T)) {
        for t in self.iter().flatten() {
            f(t)
        }
    }
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(&'a mut T)) {
        for t in self.iter_mut().flatten() {
            f(t)
        }
    }
}

/// The expressions of an expression field, each with its index and their count (for its label).
pub(crate) trait ExprField {
    fn each<'a>(&'a self, f: &mut dyn FnMut(usize, usize, &'a Expr));
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(usize, usize, &'a mut Expr));
}

impl ExprField for Expr {
    fn each<'a>(&'a self, f: &mut dyn FnMut(usize, usize, &'a Expr)) {
        f(0, 1, self)
    }
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(usize, usize, &'a mut Expr)) {
        f(0, 1, self)
    }
}

/// A position along an edge with its radius (a variable fillet's mid radii).
impl ExprField for (f64, Expr) {
    fn each<'a>(&'a self, f: &mut dyn FnMut(usize, usize, &'a Expr)) {
        f(0, 1, &self.1)
    }
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(usize, usize, &'a mut Expr)) {
        f(0, 1, &mut self.1)
    }
}

impl<T: ExprField> ExprField for Option<T> {
    fn each<'a>(&'a self, f: &mut dyn FnMut(usize, usize, &'a Expr)) {
        if let Some(t) = self {
            t.each(f)
        }
    }
    fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(usize, usize, &'a mut Expr)) {
        if let Some(t) = self {
            t.each_mut(f)
        }
    }
}

/// Lists and arrays number their elements.
macro_rules! expr_list {
    ($([$($g:tt)*] $t:ty),*) => {$(
        impl<$($g)*> ExprField for $t {
            fn each<'a>(&'a self, f: &mut dyn FnMut(usize, usize, &'a Expr)) {
                let n = self.len();
                for (i, t) in self.iter().enumerate() {
                    t.each(&mut |_, _, e| f(i, n, e));
                }
            }
            fn each_mut<'a>(&'a mut self, f: &mut dyn FnMut(usize, usize, &'a mut Expr)) {
                let n = self.len();
                for (i, t) in self.iter_mut().enumerate() {
                    t.each_mut(&mut |_, _, e| f(i, n, e));
                }
            }
        }
    )*};
}

expr_list!([T: ExprField] Vec<T>, [T: ExprField, const N: usize] [T; N]);

/// An expression's label: one for every element, one per element, or worked out from the
/// element's index and the count.
pub(crate) trait Label {
    fn at(&self, i: usize, n: usize) -> &'static str;
}

impl Label for &'static str {
    fn at(&self, _: usize, _: usize) -> &'static str {
        self
    }
}

impl<const N: usize> Label for [&'static str; N] {
    fn at(&self, i: usize, _: usize) -> &'static str {
        self.get(i).copied().unwrap_or("")
    }
}

impl<F: Fn(usize, usize) -> &'static str> Label for F {
    fn at(&self, i: usize, n: usize) -> &'static str {
        self(i, n)
    }
}

/// The offsets and angles of a plane, outermost first, and an explicit origin's coordinates
/// (nesting is capped like every walk over a plane), the construction plane it is built on, and
/// the plane itself.
impl Walk for PlaneRef {
    fn walk_exprs<'a>(&'a self, f: &mut dyn FnMut(ExprItem<'a>)) {
        let mut p = self;
        for _ in 0..64 {
            match p {
                PlaneRef::Offset { base, distance } => {
                    f(("Offset", distance, Kind::Length, ExprRole::Input));
                    p = base;
                }
                PlaneRef::AtAngle { base, angle, .. } => {
                    f(("Angle", angle, Kind::Angle, ExprRole::Input));
                    p = base;
                }
                PlaneRef::AlongPath { t, .. } => return f(("Distance (0 to 1)", t, Kind::Unitless, ExprRole::Input)),
                PlaneRef::Placed { origin, .. } => {
                    for (label, e) in ["X", "Y", "Z"].into_iter().zip(origin) {
                        f((label, e, Kind::Length, ExprRole::Placement));
                    }
                    return;
                }
                _ => return,
            }
        }
    }
    fn walk_exprs_mut<'a>(&'a mut self, f: &mut dyn FnMut(ExprItemMut<'a>)) {
        let mut p = self;
        for _ in 0..64 {
            match p {
                PlaneRef::Offset { base, distance } => {
                    f(("Offset", distance, Kind::Length, ExprRole::Input));
                    p = base;
                }
                PlaneRef::AtAngle { base, angle, .. } => {
                    f(("Angle", angle, Kind::Angle, ExprRole::Input));
                    p = base;
                }
                PlaneRef::AlongPath { t, .. } => return f(("Distance (0 to 1)", t, Kind::Unitless, ExprRole::Input)),
                PlaneRef::Placed { origin, .. } => {
                    for (label, e) in ["X", "Y", "Z"].into_iter().zip(origin) {
                        f((label, e, Kind::Length, ExprRole::Placement));
                    }
                    return;
                }
                _ => return,
            }
        }
    }
    fn walk_refs<'a>(&'a self, f: &mut dyn FnMut(RefItem<'a>)) {
        let mut p = self;
        for _ in 0..64 {
            match p {
                PlaneRef::Offset { base, .. } | PlaneRef::AtAngle { base, .. } | PlaneRef::Flipped { base, .. } => p = base,
                PlaneRef::Construction { name } => return f(Ref::Feature(name)),
                _ => return,
            }
        }
    }
    fn walk_refs_mut<'a>(&'a mut self, f: &mut dyn FnMut(RefItemMut<'a>)) {
        let mut p = self;
        for _ in 0..64 {
            match p {
                PlaneRef::Offset { base, .. } | PlaneRef::AtAngle { base, .. } | PlaneRef::Flipped { base, .. } => p = base,
                PlaneRef::Construction { name } => return f(Ref::Feature(name)),
                _ => return,
            }
        }
    }
    fn walk_geometry<'a>(&'a self, f: &mut dyn FnMut(GeoItem<'a>)) {
        f(Geo::Plane(self))
    }
    fn walk_geometry_mut<'a>(&'a mut self, f: &mut dyn FnMut(GeoItemMut<'a>)) {
        f(Geo::Plane(self))
    }
}

/// Construction axes and points: the expressions of the planes they are built from (and a point
/// along a path's distance), and their references to the model. They name no sketches, features
/// or bodies: copies remap their geometry references on their own (`Feature::remap_clone`).
fn plane_exprs<'a>(refs: Vec<&'a GeoRef>, f: &mut dyn FnMut(ExprItem<'a>)) {
    for r in refs {
        if let GeoRef::Plane { plane } = r {
            plane.walk_exprs(f);
        }
    }
}

fn plane_exprs_mut<'a>(refs: Vec<&'a mut GeoRef>, f: &mut dyn FnMut(ExprItemMut<'a>)) {
    for r in refs {
        if let GeoRef::Plane { plane } = r {
            plane.walk_exprs_mut(f);
        }
    }
}

macro_rules! construct_geometry {
    () => {
        fn walk_geometry<'a>(&'a self, f: &mut dyn FnMut(GeoItem<'a>)) {
            for r in self.refs() {
                f(Geo::Construct(r));
            }
        }
        fn walk_geometry_mut<'a>(&'a mut self, f: &mut dyn FnMut(GeoItemMut<'a>)) {
            for r in self.refs_mut() {
                f(Geo::Construct(r));
            }
        }
    };
}

impl Walk for AxisDef {
    fn walk_exprs<'a>(&'a self, f: &mut dyn FnMut(ExprItem<'a>)) {
        plane_exprs(self.refs(), f)
    }
    fn walk_exprs_mut<'a>(&'a mut self, f: &mut dyn FnMut(ExprItemMut<'a>)) {
        plane_exprs_mut(self.refs_mut(), f)
    }
    construct_geometry!();
}

impl Walk for PointDef {
    fn walk_exprs<'a>(&'a self, f: &mut dyn FnMut(ExprItem<'a>)) {
        match self {
            PointDef::AlongPath { t, .. } => f(("Distance (0 to 1)", t, Kind::Unitless, ExprRole::Input)),
            other => plane_exprs(other.refs(), f),
        }
    }
    fn walk_exprs_mut<'a>(&'a mut self, f: &mut dyn FnMut(ExprItemMut<'a>)) {
        match self {
            PointDef::AlongPath { t, .. } => f(("Distance (0 to 1)", t, Kind::Unitless, ExprRole::Input)),
            other => plane_exprs_mut(other.refs_mut(), f),
        }
    }
    construct_geometry!();
}
