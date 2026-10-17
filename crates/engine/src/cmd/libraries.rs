//! Scalar local libraries: inspect/export files and copy exact revisions into a design.
use super::CommandSpec;
use crate::{Result, Session, params::bad};
use serde::Deserialize;
use serde_json::{Value, json};
use solidcraft_io::catalog::{Catalog, Definition, Provenance};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("library.inspect", "Inspect Local Library", inspect).icon("inspect").noundo().doc_only()
        .params("path: scalar library JSON; returns validated catalog and exact entry revisions, without changing the design"),
    CommandSpec::new("library.export", "Export Design Library", export).icon("export").noundo().doc_only()
        .params("path: .json; id? (default user.local), name?, provenance?, overwrite? false. Copies assigned looks and design-local physical definitions; never changes the design"),
    CommandSpec::new("library.apply", "Apply Local Library Entry", apply).icon("import").enabled(super::not_in_sketch)
        .params("path, entry: qualified ID, revision: exact content SHA-256; targets?: {bodies?, faces?: [{body,point}], components?}; name?: physical material name on import. Copies values in one undo step; material conflicts are explicit"),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspect {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    path: String,
    #[serde(default = "default_id")]
    id: String,
    #[serde(default = "default_name")]
    name: String,
    #[serde(default)]
    provenance: Provenance,
    #[serde(default)]
    overwrite: bool,
}
fn default_id() -> String {
    "user.local".into()
}
fn default_name() -> String {
    "My library".into()
}
#[derive(Default, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Targets {
    #[serde(default)]
    bodies: Vec<String>,
    #[serde(default)]
    faces: Vec<Face>,
    #[serde(default)]
    components: Vec<String>,
}
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Face {
    body: String,
    point: [f64; 3],
}
impl Targets {
    fn validate(&self) -> Result<()> {
        let cmd = "library.apply";
        if self.bodies.len().saturating_add(self.faces.len()).saturating_add(self.components.len()) > 1024 {
            return Err(bad(cmd, "at most 1024 library application targets"));
        }
        for name in self.bodies.iter().chain(&self.components).chain(self.faces.iter().map(|f| &f.body)) {
            if name.is_empty() || name.len() > 1024 || name.chars().any(char::is_control) {
                return Err(bad(cmd, "invalid library target name"));
            }
        }
        if self.faces.iter().any(|f| f.point.iter().any(|x| !x.is_finite() || x.abs() > 1e9)) {
            return Err(bad(cmd, "invalid library target point"));
        }
        Ok(())
    }
    fn empty(&self) -> bool {
        self.bodies.is_empty() && self.faces.is_empty() && self.components.is_empty()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    path: String,
    entry: String,
    revision: String,
    #[serde(default)]
    targets: Targets,
    name: Option<String>,
}
fn path(cmd: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(bad(cmd, "invalid library path"));
    }
    solidcraft_io::vfs::check(value).map_err(|e| bad(cmd, e.to_string()))
}
fn nested(s: &mut Session, id: &str, p: &Value) -> Result<Value> {
    crate::find_command(id).ok_or_else(|| bad("library.apply", "required assignment command is unavailable"))?.run_nested(s, p)
}
fn inspect(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "library.inspect";
    let request: Inspect = serde_json::from_value(p.clone()).map_err(|e| bad(cmd, e.to_string()))?;
    path(cmd, &request.path)?;
    let catalog = Catalog::read(&request.path).map_err(|e| bad(cmd, e.to_string()))?;
    Ok(json!({"catalog":catalog}))
}
fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "library.export";
    let request: Export = serde_json::from_value(p.clone()).map_err(|e| bad(cmd, e.to_string()))?;
    path(cmd, &request.path)?;
    if !request.path.to_ascii_lowercase().ends_with(".json") {
        return Err(bad(cmd, "library export path must end in .json"));
    }
    let catalog = Catalog::from_document(&s.doc, &request.id, &request.name, request.provenance).map_err(|e| bad(cmd, e.to_string()))?;
    let bytes = catalog.encode().map_err(|e| bad(cmd, e.to_string()))?;
    #[cfg(not(target_arch = "wasm32"))]
    let result = if request.overwrite {
        solidcraft_io::write_atomic(std::path::Path::new(&request.path), &bytes, false)
    } else {
        solidcraft_io::safe_write::create_atomic(std::path::Path::new(&request.path), &bytes)
    };
    #[cfg(target_arch = "wasm32")]
    let result = if !request.overwrite && solidcraft_io::vfs::exists(&request.path) {
        Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "library file already exists"))
    } else {
        solidcraft_io::vfs::write(&request.path, &bytes)
    };
    result.map_err(|e| bad(cmd, format!("could not export library: {e}")))?;
    Ok(json!({"path":request.path,"entries":catalog.entries.len(),"bytes":bytes.len(),"catalog":catalog}))
}
fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "library.apply";
    let request: Apply = serde_json::from_value(p.clone()).map_err(|e| bad(cmd, e.to_string()))?;
    path(cmd, &request.path)?;
    request.targets.validate()?;
    if request.entry.len() > 129 || request.revision.len() != 64 {
        return Err(bad(cmd, "invalid entry ID or revision"));
    }
    let catalog = Catalog::read(&request.path).map_err(|e| bad(cmd, e.to_string()))?;
    let entry = catalog.entries.iter().find(|entry| entry.id == request.entry).ok_or_else(|| bad(cmd, "library entry is missing"))?;
    if entry.revision != request.revision {
        return Err(bad(cmd, "library entry changed; reload and explicitly select its new revision"));
    }
    let result = match &entry.definition {
        Definition::Appearance(look) => {
            if request.name.is_some() || request.targets.empty() {
                return Err(bad(cmd, "appearance entries need targets and do not take a material name"));
            }
            let mut target = serde_json::to_value(&request.targets).map_err(|e| bad(cmd, e.to_string()))?;
            target["appearance"] = json!(look.name);
            target["color"] = json!(look.color);
            target["opacity"] = json!(look.opacity);
            nested(s, "appearance.assign", &target)?
        }
        Definition::Material(material) => {
            if !request.targets.faces.is_empty() || !request.targets.components.is_empty() {
                return Err(bad(cmd, "physical material assignment takes body targets only"));
            }
            if request.targets.bodies.iter().any(|body| s.model.state().body(body).is_none()) {
                return Err(bad(cmd, "physical material targets must name existing bodies"));
            }
            let wanted = request.name.as_deref().unwrap_or(&material.name).trim();
            if wanted.is_empty() || wanted.len() > 128 || wanted.chars().any(char::is_control) {
                return Err(bad(cmd, "invalid physical material name"));
            }
            let existing = s.doc.user_materials.iter().find(|m| m.name.eq_ignore_ascii_case(wanted)).cloned();
            let name = existing.as_ref().map_or(wanted, |m| m.name.as_str()).to_string();
            let copy = material.to_material(name.clone());
            let reused = if let Some(existing) = existing {
                if existing != copy {
                    return Err(bad(cmd, "physical material name conflicts; choose a new name explicitly"));
                }
                true
            } else {
                let mut params = json!({"name":name,"density_g_cm3":material.density_g_cm3});
                if let Some(look) = &material.appearance {
                    params["color"] = json!(look.color);
                    params["opacity"] = json!(look.opacity);
                }
                nested(s, "material.create", &params)?;
                // The existing colour API labels a custom look after its material.
                // Preserve the library's exact look label inside this same transaction.
                let imported = s
                    .doc_mut()
                    .user_materials
                    .iter_mut()
                    .find(|m| m.name == name)
                    .ok_or_else(|| bad(cmd, "imported physical material is missing"))?;
                imported.appearance = copy.appearance.clone();
                false
            };
            if !request.targets.bodies.is_empty() {
                nested(s, "material.assign", &json!({"bodies":request.targets.bodies,"material":name}))?;
            }
            json!({"material":name,"reused":reused,"bodies":request.targets.bodies})
        }
    };
    Ok(json!({"entry":entry.id,"revision":entry.revision,"copied":true,"result":result}))
}

#[cfg(test)]
#[path = "libraries_tests.rs"]
mod tests;
