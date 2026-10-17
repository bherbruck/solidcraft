//! Renderer-independent product geometry. No modelling overlays or device resources.

use std::sync::Arc;

use solidcraft_geom::{Aabb3, Mesh};

#[derive(Clone, Debug, PartialEq)]
pub struct FaceAppearance {
    /// The existing mesh's B-rep face index, resolved by the document's appearance policy.
    pub face: u32,
    pub color: [u8; 3],
    pub opacity: f64,
    pub surface: Option<crate::surface::OpaqueSurface>,
    pub transmission: Option<crate::transmission::Transmission>,
}

#[derive(Clone, Debug)]
pub struct ProductMesh {
    /// Existing computed body name, including occurrence identity where applicable.
    pub name: String,
    /// Established world-space mesh. Adapters must not apply occurrence transforms again.
    pub mesh: Arc<Mesh>,
    pub color: [u8; 3],
    pub opacity: f64,
    pub surface: Option<crate::surface::OpaqueSurface>,
    pub transmission: Option<crate::transmission::Transmission>,
    pub faces: Vec<FaceAppearance>,
}

#[derive(Clone, Debug)]
pub struct ProductScene {
    pub meshes: Vec<ProductMesh>,
    pub bounds: Aabb3,
}

impl ProductScene {
    pub fn has_transmission(&self) -> bool {
        self.meshes.iter().any(|m| m.transmission.is_some() || m.faces.iter().any(|f| f.transmission.is_some()))
    }
    pub fn required_features(&self) -> Vec<crate::RenderFeature> {
        if self.meshes.iter().any(|m| m.surface.is_some() || m.faces.iter().any(|f| f.surface.is_some())) {
            vec![crate::RenderFeature::Pbr]
        } else {
            Vec::new()
        }
    }
}
