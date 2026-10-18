//! Stable configuration style targets. Display indices and placement names are never saved.
use super::Column;
use crate::{DocError, Document, FeatureKind, ModelState, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solidcraft_geom::Vec3;

pub(crate) fn shell_plane_family(planes: impl IntoIterator<Item = Option<solidcraft_geom::Plane>>) -> bool {
    let mut planes = planes.into_iter();
    let Some(Some(first)) = planes.next() else { return false };
    first.origin.is_finite() && first.x.is_finite() && first.y.is_finite() && planes.all(|plane| plane == Some(first))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFaceTarget {
    pub body: String,
    pub origin_feature: u64,
    pub name: String,
    /// Definition-frame point for diagnostics and rendering; never a resolver fallback.
    pub point: Vec3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<super::FaceCertificate>,
}
impl ConfigFaceTarget {
    pub fn key(&self) -> String {
        // Length-prefix encoding keeps punctuation in body/name unambiguous.
        format!("{}:{}{}", self.body.len(), self.body, self.name)
    }
    pub fn matches(&self, face: &crate::appearance::FaceLook) -> bool {
        face.body == self.body && face.name.as_ref().is_some_and(|name| crate::naming::strip_piece(name) == self.name)
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(certificate) = &self.certificate {
            certificate.validate()?;
        }
        if self.body.is_empty()
            || self.body.len() > 128
            || self.name.is_empty()
            || self.name.len() > 512
            || self.origin_feature == 0
            || [self.point.x, self.point.y, self.point.z].iter().any(|v| !v.is_finite() || v.abs() > 1e9)
        {
            return Err(DocError::Invalid("invalid configuration face target".into()));
        }
        Ok(())
    }
}

pub fn validate_face_target_json(target: &Value) -> Result<()> {
    let fields = target.as_object().ok_or_else(|| DocError::Invalid("face target must be an object".into()))?;
    if !(4..=5).contains(&fields.len())
        || fields.keys().any(|key| !["body", "origin_feature", "name", "point", "certificate"].contains(&key.as_str()))
        || fields.get("body").and_then(Value::as_str).is_none_or(|body| body.is_empty() || body.len() > 128)
        || fields.get("name").and_then(Value::as_str).is_none_or(|name| name.is_empty() || name.len() > 512)
        || fields.get("origin_feature").and_then(Value::as_u64).is_none_or(|id| id == 0)
        || fields.get("point").and_then(Value::as_array).is_none_or(|point| {
            point.len() != 3 || point.iter().any(|value| value.as_f64().is_none_or(|value| !value.is_finite() || value.abs() > 1e9))
        })
    {
        return Err(DocError::Invalid("invalid or oversized configuration face target".into()));
    }
    if let Some(certificate) = fields.get("certificate") {
        super::validate_certificate_json(certificate)?;
    }
    Ok(())
}

/// Budget malformed style nodes before decoding their typed representation.
pub fn validate_styles_json(value: &Value) -> Result<()> {
    if let Some(columns) = value.get("configs").and_then(|v| v.get("columns")).and_then(Value::as_array) {
        if columns.len() > super::MAX_COLUMNS {
            return Err(DocError::Invalid("too many configuration columns".into()));
        }
        for column in columns {
            if let Some(target) = column.get("face_appearance") {
                validate_face_target_json(target)?;
            }
            if let Some(id) = column.get("component_appearance")
                && id.as_u64().is_none_or(|id| id == 0)
            {
                return Err(DocError::Invalid("configuration component ID must be nonzero".into()));
            }
        }
    }
    if let Some(faces) = value.get("appearances").and_then(|value| value.get("faces")).and_then(Value::as_array) {
        for face in faces {
            if let Some(certificate) = face.get("certificate") {
                super::validate_certificate_json(certificate)?;
                if !certificate.is_null()
                    && (face.get("strict_name").and_then(Value::as_bool) != Some(true)
                        || face.get("body").and_then(Value::as_str).is_none_or(|body| body.is_empty() || body.len() > 128)
                        || face.get("name").and_then(Value::as_str).is_none_or(|name| name.is_empty() || name.len() > 512)
                        || face.get("point").and_then(Value::as_array).is_none_or(|point| {
                            point.len() != 3 || point.iter().any(|value| value.as_f64().is_none_or(|value| !value.is_finite() || value.abs() > 1e9))
                        }))
                {
                    return Err(DocError::Invalid("invalid certified face appearance".into()));
                }
            }
        }
    }
    Ok(())
}

impl Document {
    pub(crate) fn flat_certificate_producer_matches(&self, producer: u64, certificate: &super::FaceCertificate) -> bool {
        use super::FaceCertificate;
        let Some(feature) = self.feature(producer) else { return false };
        match (certificate, &feature.kind) {
            (FaceCertificate::Box { .. }, FeatureKind::Box { operation: crate::Operation::NewBody | crate::Operation::Join, .. }) => true,
            (FaceCertificate::CylinderCap { .. }, FeatureKind::Cylinder { operation: crate::Operation::NewBody | crate::Operation::Join, .. }) => true,
            (FaceCertificate::ExtrudeCap { sketch, .. }, FeatureKind::Extrude { sketch: current, extent, operation: crate::Operation::NewBody | crate::Operation::Join, .. }) => sketch == current && extent.taper.is_none(),
            (FaceCertificate::ExtrudeSide { sketch, curve }, FeatureKind::Extrude { sketch: current, extent, operation: crate::Operation::NewBody | crate::Operation::Join, .. }) => {
                sketch == current && extent.taper.is_none()
                    && self.feature(*sketch).is_some_and(|feature| matches!(&feature.kind, FeatureKind::Sketch { sketch, .. } if sketch.curves.iter().any(|candidate| candidate.id == *curve && !candidate.construction && matches!(candidate.kind, solidcraft_sketch::CurveKind::Line { .. } | solidcraft_sketch::CurveKind::Circle { .. } | solidcraft_sketch::CurveKind::Arc { .. }))))
            }
            (FaceCertificate::Imported { .. }, FeatureKind::Import { .. }) => true,
            _ => false,
        }
    }
    /// Certify named generator lineage; generic index-derived naming is unsupported.
    pub fn certify_config_face(&self, target: &ConfigFaceTarget) -> Result<()> {
        target.validate()?;
        let prefix = format!("F{}:", target.origin_feature);
        let role = target.name.strip_prefix(&prefix).ok_or_else(|| DocError::Invalid("face lineage does not match its feature".into()))?;
        // An exact history name may name a piece by the faces around it (`F2:top#[F2:side:l4,…]`),
        // never by a number (an older design's piece, or a tie the faces around it left).
        let numbered =
            target.name.match_indices('#').any(|(i, _)| target.name.get(i + 1..).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())));
        let lineage_piece = matches!(target.certificate, Some(super::FaceCertificate::Lineage)) && !numbered;
        if crate::naming::strip_piece(&target.name) != target.name && !lineage_piece {
            return Err(DocError::Invalid("configure a face lineage, not a numbered split piece".into()));
        }
        let feature = self.feature(target.origin_feature).ok_or_else(|| DocError::Unknown("face origin feature is gone".into()))?;
        if let Some(certificate) = &target.certificate {
            let valid = match (certificate, &feature.kind) {
                (super::FaceCertificate::Box { axis, high }, FeatureKind::Box { operation: crate::Operation::NewBody | crate::Operation::Join, .. }) => {
                    ["x", "y", "z"].get(*axis as usize).is_some_and(|axis| role == format!("{}{axis}", if *high { "+" } else { "-" }))
                }
                (super::FaceCertificate::CylinderCap { high }, FeatureKind::Cylinder { operation: crate::Operation::NewBody | crate::Operation::Join, .. }) => {
                    role == super::cylinder_cap_role(*high)
                }
                (super::FaceCertificate::ExtrudeCap { sketch, high, outer, holes }, FeatureKind::Extrude { sketch: producer_sketch, extent, operation: crate::Operation::NewBody | crate::Operation::Join, .. }) => {
                    sketch == producer_sketch && extent.taper.is_none()
                        && role == format!("cap:{}:{}", if *high { "high" } else { "low" }, super::cap_key(outer, holes))
                }
                (super::FaceCertificate::ExtrudeSide { sketch, curve }, FeatureKind::Extrude { sketch: producer_sketch, extent, operation: crate::Operation::NewBody | crate::Operation::Join, .. }) => {
                    sketch == producer_sketch && extent.taper.is_none() && role == format!("side:{curve}")
                        && self.feature(*sketch).is_some_and(|feature| matches!(&feature.kind, FeatureKind::Sketch { sketch, .. } if sketch.curves.iter().any(|candidate| candidate.id == *curve && !candidate.construction && matches!(candidate.kind, solidcraft_sketch::CurveKind::Line { .. } | solidcraft_sketch::CurveKind::Circle { .. } | solidcraft_sketch::CurveKind::Arc { .. }))))
                }
                (super::FaceCertificate::Imported { .. }, FeatureKind::Import { .. }) => role.strip_prefix("face:").is_some_and(|index| index.parse::<usize>().is_ok()),
                (super::FaceCertificate::Lineage, _) => !role.is_empty(),
                (super::FaceCertificate::ShellPlanar { source_producer, source, inner }, FeatureKind::Shell { .. }) => {
                    *source_producer != target.origin_feature
                        && self.flat_certificate_producer_matches(*source_producer, source)
                        && super::shell_certificate_role(source, *inner).is_ok_and(|expected| role == expected)
                }
                _ => false,
            };
            return if valid { Ok(()) } else { Err(DocError::Invalid("face certificate no longer matches its producer".into())) };
        }
        let valid = match &feature.kind {
            FeatureKind::Box { .. } => ["+x", "-x", "+y", "-y", "+z", "-z"].contains(&role),
            FeatureKind::Cylinder { .. } => ["top", "bottom", "side"].contains(&role),
            // Curved side lineage currently uses centroid/nearest-curve heuristics;
            // certify caps only until the naming layer supplies proven curve provenance.
            FeatureKind::Extrude { extent, .. } => {
                extent.from.is_none()
                    && extent.start_offset.as_deref().is_none_or(|value| {
                        crate::expr::is_literal(value)
                            && crate::expr::eval_with(value, &|name| Err(DocError::Unknown(name.into())))
                                .ok()
                                .and_then(|value| value.to_kind(crate::expr::Kind::Length).ok())
                                .is_some_and(|value| value.abs() < 1e-12)
                    })
                    && extent.distance2.is_none()
                    && ["start", "top", "bottom"].contains(&role)
            }
            FeatureKind::Revolve { angle2, symmetric, .. } => angle2.is_none() && !symmetric && ["start", "end"].contains(&role),
            _ => false,
        };
        if !valid {
            return Err(DocError::Invalid("this face has no supported stable semantic lineage".into()));
        }
        Ok(())
    }
    /// Verify the current named pieces describe one surface, not ambiguous generated roles.
    pub fn configuration_face_indices(&self, body: &crate::ModelBody, target: &ConfigFaceTarget) -> Result<Vec<usize>> {
        self.certify_config_face(target)?;
        if body.body.is_mesh() || body.definition_name() != target.body {
            return Err(DocError::Invalid("configured face definition does not match".into()));
        }
        if let Some(super::FaceCertificate::Lineage) = &target.certificate {
            return self.lineage_face_indices(body, &target.name);
        }
        if let Some(super::FaceCertificate::ShellPlanar { source_producer, source, inner }) = &target.certificate {
            let indices: Vec<_> = body.body.face_sources().into_iter().flatten().enumerate().filter_map(|(index, current)| {
                matches!(current, Some(solidcraft_kernel::FaceSource::ShellPlanar { producer: Some(producer), source: current_source, inner: current_inner, .. }) if *producer == target.origin_feature && inner == current_inner && super::flat_source_matches(source, current_source, *source_producer)).then_some(index)
            }).collect();
            // A shell can split one original support into several pieces. All pieces
            // must retain the same canonical frame, not merely lie on a common plane.
            if !shell_plane_family(indices.iter().map(|index| body.body.face_plane(*index))) {
                return Err(DocError::Unknown("shell planar face certificate is missing or ambiguous".into()));
            }
            return Ok(indices);
        }
        if matches!(
            target.certificate,
            Some(super::FaceCertificate::Box { .. } | super::FaceCertificate::CylinderCap { .. } | super::FaceCertificate::ExtrudeCap { .. })
        ) {
            let indices: Vec<_> = body
                .body
                .face_sources()
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(|(index, source)| {
                    let matched = match (&target.certificate, source) {
                        (
                            Some(super::FaceCertificate::Box { axis, high }),
                            Some(solidcraft_kernel::FaceSource::Box { producer: Some(producer), axis: current_axis, high: current_high, .. }),
                        ) => *producer == target.origin_feature && axis == current_axis && high == current_high,
                        (
                            Some(super::FaceCertificate::CylinderCap { high }),
                            Some(solidcraft_kernel::FaceSource::CylinderCap { producer, high: current_high, .. }),
                        ) => *producer == target.origin_feature && high == current_high,
                        (
                            Some(super::FaceCertificate::ExtrudeCap { sketch, high, outer, holes }),
                            Some(solidcraft_kernel::FaceSource::CertifiedExtrudeCap {
                                producer,
                                sketch: current_sketch,
                                high: current_high,
                                outer: current_outer,
                                holes: current_holes,
                                ..
                            }),
                        ) => {
                            *producer == target.origin_feature
                                && sketch == current_sketch
                                && high == current_high
                                && outer == current_outer
                                && holes == current_holes
                        }
                        _ => false,
                    };
                    matched.then_some(index)
                })
                .collect();
            if indices.is_empty() {
                return Err(DocError::Unknown("attached planar face certificate is missing or ambiguous".into()));
            }
            return self.certified_pieces(body, indices, target.origin_feature);
        }
        if let Some(super::FaceCertificate::ExtrudeSide { sketch, curve }) = &target.certificate {
            let side = |index: usize, exact: bool| matches!(body.body.face_sources().and_then(|sources| sources.get(index)), Some(Some(solidcraft_kernel::FaceSource::CertifiedExtrudeSide { producer, sketch: current_sketch, curve: current_curve, .. })) if *producer == target.origin_feature && current_sketch == sketch && (!exact || current_curve == curve));
            let count = body.body.face_sources().map_or(0, |sources| sources.len());
            let mut indices: Vec<_> = (0..count).filter(|index| side(*index, true)).collect();
            // A side drawn in collinear pieces is one face over all of them (#1788): a certificate
            // made when the pieces were faces of their own finds it by the piece's name, which
            // the joined face answers to.
            if indices.is_empty() {
                let names = crate::naming::names(body);
                indices = (0..count)
                    .filter(|index| {
                        side(*index, false) && crate::history_names::matches(&names, *index, &target.name) == Some(crate::history_names::Match::Alias)
                    })
                    .collect();
            }
            if indices.is_empty() {
                return Err(DocError::Unknown("constructed side certificate no longer exists".into()));
            }
            return Ok(self.without_copies(body, indices));
        }
        if let Some(super::FaceCertificate::Imported { payload_sha256, item, face }) = &target.certificate {
            let indices: Vec<_> = body.body.face_sources().into_iter().flatten().enumerate().filter_map(|(index, source)| {
                matches!(source, Some(solidcraft_kernel::FaceSource::Imported { item: current_item, face: current_face, payload_sha256: Some(digest), producer: Some(producer) }) if current_item == item && current_face == face && digest == payload_sha256 && *producer == target.origin_feature).then_some(index)
            }).collect();
            if indices.is_empty() {
                return Err(DocError::Unknown("immutable import face certificate no longer exists".into()));
            }
            return Ok(self.without_copies(body, indices));
        }
        let names = crate::naming::face_names(body);
        let indices: Vec<usize> =
            names.iter().enumerate().filter(|(_, name)| crate::naming::strip_piece(name) == target.name).map(|(index, _)| index).collect();
        if indices.is_empty() {
            return Err(DocError::Unknown(format!("configured face `{}` on `{}` no longer exists", target.name, target.body)));
        }
        if let Some(certificate) = &target.certificate {
            for index in &indices {
                if self.configuration_face_certificate(body, *index)?.as_ref() != Some(certificate) {
                    return Err(DocError::Invalid("face pieces do not share the exact certified source".into()));
                }
            }
            return Ok(indices);
        }
        if target.name.ends_with(":side") {
            if indices.len() != 1 {
                return Err(DocError::Invalid("split curved face lineage is not yet certified".into()));
            }
            return Ok(indices);
        }
        // Caps and box faces must share an oriented plane. This rejects generic cap-role
        // collisions while permitting the coplanar pieces of a split supported face.
        let mesh = body.mesh();
        let mut plane: Option<(Vec3, Vec3)> = None;
        for (triangle, face) in mesh.triangles.iter().zip(&mesh.tri_face) {
            if indices.binary_search(&(*face as usize)).is_err() {
                continue;
            }
            let Some([a, b, c]) = mesh.tri(triangle) else { continue };
            let Some(normal) = (b - a).cross(c - a).normalized() else { continue };
            if let Some((point, base)) = plane {
                let magnitude = [point.x, point.y, point.z, a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z]
                    .iter()
                    .fold(body.body.size(), |scale, value| scale.max(value.abs()));
                let tolerance = (magnitude * f64::EPSILON * 128.0).max(1e-7);
                if normal.dot(base) < 1.0 - 1e-6 || [a, b, c].iter().any(|vertex| (*vertex - point).dot(base).abs() > tolerance) {
                    return Err(DocError::Invalid("face lineage names more than one oriented surface".into()));
                }
            } else {
                plane = Some((a, normal));
            }
        }
        if plane.is_none() {
            return Err(DocError::Invalid("configured face has no valid surface".into()));
        }
        Ok(indices)
    }
    pub fn config_face_look(&self, target: &ConfigFaceTarget) -> Option<crate::appearance::Look> {
        self.appearances.faces.iter().rev().find(|face| target.matches(face)).map(|face| face.look.clone())
    }
    /// Validate against the final rebuilt definition state. Missing names never pick nearby faces.
    pub fn configuration_style_status(&self, state: &ModelState) -> Result<Vec<serde_json::Value>> {
        let mut result = Vec::new();
        for column in &self.configs.columns {
            match column {
                Column::ComponentAppearance(id) => {
                    let component =
                        self.components.iter().find(|c| c.id == *id).ok_or_else(|| DocError::Unknown(format!("component {id} is gone")))?;
                    result.push(serde_json::json!({"key":self.config_key(column),"target":component.name,"shared":true,"dormant":false}));
                }
                Column::FaceAppearance(target) => {
                    self.certify_config_face(target)?;
                    let dormant = match state.body(&target.body) {
                        None if self.feature(target.origin_feature).is_some_and(|feature| feature.suppressed) => true,
                        None => return Err(DocError::Unknown(format!("configured face body `{}` is gone", target.body))),
                        Some(body) => {
                            self.configuration_face_indices(body, target)?;
                            false
                        }
                    };
                    result.push(serde_json::json!({"key":self.config_key(column),"target":format!("{} / {}",target.body,target.name),"shared":true,"dormant":dormant}));
                }
                _ => {}
            }
        }
        Ok(result)
    }
    /// Maintain columns and row cell alignment when component definitions are deleted.
    pub fn remove_configuration_components(&mut self, ids: &[u64]) {
        let keep: Vec<bool> = self.configs.columns.iter().map(|c| !matches!(c,Column::ComponentAppearance(id) if ids.contains(id))).collect();
        let mut mask = keep.iter();
        self.configs.columns.retain(|_| mask.next().copied().unwrap_or(false));
        for row in &mut self.configs.rows {
            let mut mask = keep.iter();
            row.cells.retain(|_| mask.next().copied().unwrap_or(false));
        }
        self.appearances.components.retain(|id, _| !ids.contains(id));
    }
}
