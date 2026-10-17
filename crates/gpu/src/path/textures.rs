//! One read-only storage buffer: body lookup, planar records and linear RGBA pixels.
use solidcraft_geom::Vec3;
use solidcraft_render::job::{
    CancelToken, ErrorCode, JobError, Result,
    textures::{RuntimeTextures, TextureMap},
};

fn vector(a: [f64; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}
fn write_word(bytes: &mut [u8], word: usize, value: u32) -> Result<()> {
    bytes
        .get_mut(word * 4..word * 4 + 4)
        .ok_or_else(|| JobError::new(ErrorCode::Internal, "texture record outside allocation"))?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}
fn pixels(bytes: &mut Vec<u8>, map: Option<&TextureMap>, cancel: &CancelToken) -> Result<(u32, [u32; 2])> {
    let Some(map) = map else {
        return Ok((0, [0; 2]));
    };
    cancel.check()?;
    let rgba = map.decode(cancel)?;
    let offset = bytes.len() / 4;
    for (index, value) in rgba.into_iter().enumerate() {
        if index % 4096 == 0 {
            cancel.check()?;
        }
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    Ok((offset as u32, map.dimensions()))
}
pub(super) fn flatten(resources: Option<&RuntimeTextures>, body_count: usize, origin: Vec3, cancel: &CancelToken) -> Result<Vec<u8>> {
    let Some(resources) = resources else {
        return Ok(vec![0; 16]);
    };
    let header = 4 + body_count + 1;
    let record_words = resources.bindings().len() * 16;
    let mut bytes = super::scene::reserved((header + record_words) * 4 + resources.summary().linear_pixel_bytes as usize)?;
    bytes.resize((header + record_words) * 4, 0);
    write_word(&mut bytes, 0, (body_count + 1) as u32)?;
    for (index, binding) in resources.bindings().iter().enumerate() {
        cancel.check()?;
        let record = header + index * 16;
        write_word(&mut bytes, 4 + binding.body_index + 1, record as u32)?;
        let delta = vector(binding.frame.origin) - origin;
        if [delta.x, delta.y, delta.z].iter().any(|v| v.abs() > 1e7) {
            return Err(JobError::new(ErrorCode::InvalidScene, "texture frame must be within 1e7 mm of camera target"));
        }
        let u = vector(binding.frame.u);
        let v = vector(binding.frame.v);
        for (offset, p) in [(0, delta), (4, u / u.len2()), (8, v / v.len2())] {
            for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
                write_word(&mut bytes, record + offset + k, (c as f32).to_bits())?;
            }
        }
        let (base, base_size) = pixels(&mut bytes, binding.base_color.as_ref(), cancel)?;
        let (normal, normal_size) = pixels(&mut bytes, binding.normal.as_ref(), cancel)?;
        for (offset, value) in [
            (3, base),
            (7, base_size[0]),
            (11, base_size[1]),
            (12, normal),
            (13, normal_size[0]),
            (14, normal_size[1]),
            (15, (binding.normal_strength as f32).to_bits()),
        ] {
            write_word(&mut bytes, record + offset, value)?;
        }
    }
    cancel.check()?;
    Ok(bytes)
}
