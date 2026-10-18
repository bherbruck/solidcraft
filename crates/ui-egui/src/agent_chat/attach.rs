//! Files a person adds to a message (the + button, a drop on the chat, a paste): pictures go to the
//! agent with it, text files go in as text, and CAD files are imported into the design, the agent
//! being told what arrived. Each file is checked and bounded before it is kept; a refused one says
//! why.

use std::path::Path;
use std::sync::Arc;

use super::backend::{self, PromptPart};
use crate::i18n::format as trf;

/// A refusal in the app's language: the file's name, then why.
fn refused(key: &str, name: &str, args: &[(&str, &str)]) -> String {
    let mut all = vec![("name", name)];
    all.extend_from_slice(args);
    trf(key, &all)
}

/// A refusal carrying the system's or the decoder's own words.
fn failed(name: &str, e: impl std::fmt::Display) -> String {
    refused("ui.agent_chat_file_error", name, &[("error", &e.to_string())])
}

/// Most files on one message.
pub const MAX_FILES: usize = 8;
/// Largest picture file read, before it is scaled down.
const MAX_PICTURE_BYTES: u64 = 40 * 1024 * 1024;
/// Largest text file sent in full.
const MAX_TEXT_BYTES: u64 = 256 * 1024;
/// A picture's longer side is scaled down to this before it is sent.
pub const MAX_SIDE: u32 = 2048;
/// Largest picture sent, as it came or re-encoded: its base64 (a third larger) stays under 5 MB,
/// so eight of them still fit in a prompt.
pub const MAX_SENT_BYTES: usize = 3_750_000;
/// A picture is not halved below this to fit; past it, it is refused.
const MIN_SIDE: u32 = 64;
/// Largest picture decoded (pixels on a side), so a hostile header can't ask for gigabytes.
const MAX_DECODE_SIDE: u32 = 20_000;
/// The chip's picture.
const THUMB: u32 = 64;

/// Pictures the agents read as they are; BMP and TIFF are sent as PNG.
const PICTURE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];
/// Text sent in full; SVG goes as its source, which the agents read well.
const TEXT_EXTS: &[&str] = &["svg", "txt", "md", "markdown", "json", "csv", "tsv", "yaml", "yml", "toml", "xml", "log", "ini", "cfg"];
/// CAD files and meshes, imported into the design (the file picker's list; [`kind`] asks the
/// importers themselves).
const CAD_EXTS: &[&str] = &["step", "stp", "igs", "iges", "FCStd", "3dm", "stl", "3mf", "obj", "glb", "gltf"];

/// Every extension that can be attached, pictures first (the file picker's list).
pub fn extensions() -> Vec<&'static str> {
    PICTURE_EXTS.iter().chain(TEXT_EXTS).chain(CAD_EXTS).copied().collect()
}

/// What a file is, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Picture,
    Text,
    Cad,
    Other,
}

pub fn kind(path: &Path) -> Kind {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    match ext.as_str() {
        e if PICTURE_EXTS.contains(&e) => Kind::Picture,
        e if TEXT_EXTS.contains(&e) => Kind::Text,
        _ => {
            let p = path.to_string_lossy();
            if solidcraft_engine::io::is_step_path(&p) || solidcraft_engine::io::is_mesh_path(&p) { Kind::Cad } else { Kind::Other }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    /// The picture's bytes as sent (PNG, JPEG, GIF or WebP), and its size in pixels.
    Picture {
        mime: &'static str,
        bytes: Arc<Vec<u8>>,
        size: [u32; 2],
    },
    Text(Arc<String>),
    /// A CAD file imported into the design as its own component.
    Imported,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    /// Its own on the message, never reused (set when it is added there; 0 until then): the
    /// chip's thumbnail is kept by it.
    pub id: u64,
    pub name: String,
    pub content: Content,
    /// A small copy of a picture for its chip.
    pub thumb: Option<Arc<egui::ColorImage>>,
}

impl Attachment {
    pub fn imported(name: String) -> Self {
        Attachment { id: 0, name, content: Content::Imported, thumb: None }
    }
    pub fn is_picture(&self) -> bool {
        matches!(self.content, Content::Picture { .. })
    }
}

pub(crate) fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// Read a picture or text file for a message. CAD files are imported by the caller, not read here.
pub fn read(path: &Path) -> Result<Attachment, String> {
    let name = name_of(path);
    let meta = std::fs::metadata(path).map_err(|e| failed(&name, e))?;
    if !meta.is_file() {
        return Err(refused("ui.agent_chat_file_not_a_file", &name, &[]));
    }
    match kind(path) {
        Kind::Picture => {
            if meta.len() > MAX_PICTURE_BYTES {
                return Err(refused("ui.agent_chat_file_picture_too_big", &name, &[("max", &(MAX_PICTURE_BYTES / 1024 / 1024).to_string())]));
            }
            let bytes = std::fs::read(path).map_err(|e| failed(&name, e))?;
            picture(name, &bytes)
        }
        Kind::Text => {
            if meta.len() > MAX_TEXT_BYTES {
                return Err(refused("ui.agent_chat_file_text_too_big", &name, &[("max", &(MAX_TEXT_BYTES / 1024).to_string())]));
            }
            let bytes = std::fs::read(path).map_err(|e| failed(&name, e))?;
            let text = String::from_utf8(bytes).map_err(|_| refused("ui.agent_chat_file_not_utf8", &name, &[]))?;
            Ok(Attachment { id: 0, name, content: Content::Text(Arc::new(text)), thumb: None })
        }
        Kind::Cad => Err(refused("ui.agent_chat_file_cad_not_read", &name, &[])),
        Kind::Other if matches!(path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("heic" | "heif")) => {
            Err(refused("ui.agent_chat_file_heic", &name, &[]))
        }
        Kind::Other => Err(refused("ui.agent_chat_file_kinds", &name, &[])),
    }
}

/// A picture from a file's bytes: decoded within limits, and sent as it came when the agents read
/// that format (PNG, JPEG, GIF, WebP), it is at most [`MAX_SIDE`] on a side and at most
/// [`MAX_SENT_BYTES`]; otherwise re-encoded (see [`encoded`]).
pub fn picture(name: String, bytes: &[u8]) -> Result<Attachment, String> {
    picture_within(name, bytes, MAX_SENT_BYTES)
}

fn picture_within(name: String, bytes: &[u8], limit: usize) -> Result<Attachment, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| failed(&name, e))?;
    let format = reader.format();
    let mime = match format {
        Some(image::ImageFormat::Png) => Some("image/png"),
        Some(image::ImageFormat::Jpeg) => Some("image/jpeg"),
        Some(image::ImageFormat::Gif) => Some("image/gif"),
        Some(image::ImageFormat::WebP) => Some("image/webp"),
        Some(image::ImageFormat::Bmp | image::ImageFormat::Tiff) => None,
        _ => return Err(refused("ui.agent_chat_file_unreadable_picture", &name, &[])),
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(1024 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| failed(&name, e))?;
    if img.width() == 0 || img.height() == 0 {
        return Err(refused("ui.agent_chat_file_empty_picture", &name, &[]));
    }
    if let Some(mime) = mime
        && img.width().max(img.height()) <= MAX_SIDE
        && bytes.len() <= limit
    {
        let thumb = thumbnail(&img);
        return Ok(Attachment {
            id: 0,
            name,
            content: Content::Picture { mime, bytes: Arc::new(bytes.to_vec()), size: [img.width(), img.height()] },
            thumb,
        });
    }
    encoded(name, img, limit)
}

/// A picture copied to the clipboard (RGBA rows from the system), sent as PNG.
pub fn from_rgba(name: String, width: usize, height: usize, rgba: &[u8]) -> Result<Attachment, String> {
    let (w, h) = (u32::try_from(width).ok(), u32::try_from(height).ok());
    let (Some(w), Some(h)) = (w, h) else { return Err(refused("ui.agent_chat_file_too_large", &name, &[])) };
    if w == 0 || h == 0 || w > MAX_DECODE_SIDE || h > MAX_DECODE_SIDE || width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(rgba.len())
    {
        return Err(refused("ui.agent_chat_file_not_a_picture", &name, &[]));
    }
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec()).ok_or_else(|| refused("ui.agent_chat_file_not_a_picture", &name, &[]))?;
    encoded(name, image::DynamicImage::ImageRgba8(img), MAX_SENT_BYTES)
}

/// Scale down to [`MAX_SIDE`] and encode within `limit` bytes: as PNG when that fits; else as JPEG
/// when the picture has no transparency; else halved and tried again. A GIF arrives here as its
/// first frame.
fn encoded(name: String, img: image::DynamicImage, limit: usize) -> Result<Attachment, String> {
    let mut img = if img.width().max(img.height()) > MAX_SIDE { img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle) } else { img };
    let opaque = opaque(&img);
    loop {
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| failed(&name, e))?;
        let mut sent = (png.len() <= limit).then_some(("image/png", png));
        if sent.is_none() && opaque {
            let mut jpeg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85).encode_image(&img.to_rgb8()).map_err(|e| failed(&name, e))?;
            sent = (jpeg.len() <= limit).then_some(("image/jpeg", jpeg));
        }
        if let Some((mime, bytes)) = sent {
            let thumb = thumbnail(&img);
            return Ok(Attachment {
                id: 0,
                name,
                content: Content::Picture { mime, bytes: Arc::new(bytes), size: [img.width(), img.height()] },
                thumb,
            });
        }
        if img.width().max(img.height()) <= MIN_SIDE {
            return Err(refused("ui.agent_chat_file_too_large_to_send", &name, &[]));
        }
        img = img.resize((img.width() / 2).max(1), (img.height() / 2).max(1), image::imageops::FilterType::Triangle);
    }
}

/// No pixel is see-through (a screenshot or a photo, even when stored with an alpha channel).
fn opaque(img: &image::DynamicImage) -> bool {
    !img.color().has_alpha() || img.to_rgba8().pixels().all(|p| p.0[3] == 255)
}

fn thumbnail(img: &image::DynamicImage) -> Option<Arc<egui::ColorImage>> {
    let t = img.thumbnail(THUMB, THUMB).to_rgba8();
    let size = [usize::try_from(t.width()).ok()?, usize::try_from(t.height()).ok()?];
    Some(Arc::new(egui::ColorImage::from_rgba_unmultiplied(size, t.as_raw())))
}

/// What the message says about its files, after the person's words: the text files' text, and the
/// pictures and imported parts by name (the agent sees the pictures themselves as images).
/// `pictures_sent`: whether this agent takes pictures. The text files share `budget` characters
/// in order: one that doesn't fit is cut and says how much is missing, and one past the budget
/// is named only, so the message stays within the prompt cap.
pub fn note(files: &[Attachment], pictures_sent: bool, budget: usize) -> String {
    let mut out = String::new();
    let pictures: Vec<&str> = files.iter().filter(|f| f.is_picture()).map(|f| f.name.as_str()).collect();
    if !pictures.is_empty() {
        let list = pictures.join(", ");
        out += &if pictures_sent {
            format!("\n\nAttached pictures: {list}.")
        } else {
            format!("\n\n(Pictures attached but not sent, this agent takes none: {list}.)")
        };
    }
    let mut left = budget;
    for f in files {
        match &f.content {
            Content::Text(t) => {
                let t = t.trim_end();
                let len = t.chars().count();
                if left == 0 {
                    out += &format!("\n\nAttached file `{}`: left out, the message is at its size limit.", f.name);
                } else if len <= left {
                    out += &format!("\n\nAttached file `{}`:\n```\n{t}\n```", f.name);
                    left -= len;
                } else {
                    let kept: String = t.chars().take(left).collect();
                    out += &format!("\n\nAttached file `{}` (cut: {} more characters left out):\n```\n{kept}\n```", f.name, len - left);
                    left = 0;
                }
            }
            Content::Imported => out += &format!("\n\n(I added `{}` to the design as a component of its own.)", f.name),
            Content::Picture { .. } => {}
        }
    }
    out
}

/// The pictures as image parts of the message.
pub fn parts(files: &[Attachment]) -> Vec<PromptPart> {
    files
        .iter()
        .filter_map(|f| match &f.content {
            Content::Picture { mime, bytes, .. } => Some(PromptPart::Image { mime: (*mime).to_string(), base64: backend::base64(bytes) }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("solidcraft-attach-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn png(path: &Path, w: u32, h: u32) {
        image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255])).save(path).unwrap();
    }

    #[test]
    fn pictures_are_kept_or_scaled_down_and_text_goes_in_whole() {
        let d = dir("read");
        let small = d.join("board.png");
        png(&small, 1600, 1120);
        let a = read(&small).unwrap();
        let Content::Picture { mime, size, bytes } = &a.content else { panic!("a picture") };
        assert_eq!((*mime, *size), ("image/png", [1600, 1120]));
        assert_eq!(bytes.as_slice(), std::fs::read(&small).unwrap().as_slice(), "small enough: sent as it came");
        assert!(a.thumb.as_ref().is_some_and(|t| t.size[0] <= 64 && t.size[1] <= 64));
        let big = d.join("photo.JPG");
        image::RgbImage::from_pixel(4000, 1000, image::Rgb([10, 10, 10])).save_with_format(&big, image::ImageFormat::Jpeg).unwrap();
        let b = read(&big).unwrap();
        let Content::Picture { mime, size, .. } = &b.content else { panic!("a picture") };
        assert_eq!((*mime, *size), ("image/png", [2048, 512]), "scaled to 2048 on the long side");
        let notes = d.join("brief.md");
        std::fs::write(&notes, "# Brief\nwall 2 mm\n").unwrap();
        let c = read(&notes).unwrap();
        assert!(matches!(&c.content, Content::Text(t) if t.contains("wall 2 mm")));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Pixels that don't compress, so a small picture can be made larger than a small limit.
    fn noise(w: u32, h: u32, alpha: bool) -> image::DynamicImage {
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed & 0xff) as u8
        };
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |_, _| {
            image::Rgba([next(), next(), next(), if alpha { next() } else { 255 }])
        }))
    }

    fn encode(img: &image::DynamicImage, format: image::ImageFormat) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), format).unwrap();
        out
    }

    /// Review of #1744: a picture up to 2048 px went byte for byte up to the 40 MB file cap, and
    /// eight of those don't fit in a prompt. Over the limit a picture is re-encoded: PNG when that
    /// fits, JPEG when it has no transparency, halved until it fits; a GIF goes as its first frame.
    #[test]
    fn pictures_over_the_send_limit_are_reencoded_to_fit() {
        let limit = 40_000;
        let photo = encode(&noise(300, 200, false), image::ImageFormat::Png);
        assert!(photo.len() > limit, "the test picture is over the limit");
        let a = picture_within("photo.png".into(), &photo, limit).unwrap();
        let Content::Picture { mime, bytes, size } = &a.content else { panic!("a picture") };
        assert!(bytes.len() <= limit, "{} bytes", bytes.len());
        assert_eq!(*mime, "image/jpeg", "an opaque picture goes as JPEG when PNG is too big");
        assert!(size[0] <= 300 && size[1] <= 200 && size[0] * 2 == size[1] * 3, "{size:?}: smaller, same shape");
        let clear = encode(&noise(300, 200, true), image::ImageFormat::Png);
        let b = picture_within("overlay.png".into(), &clear, limit).unwrap();
        let Content::Picture { mime, bytes, size } = &b.content else { panic!("a picture") };
        assert_eq!(*mime, "image/png", "transparency stays PNG");
        assert!(bytes.len() <= limit && size[0] < 300, "{} bytes, {size:?}: halved to fit", bytes.len());
        let gif = encode(
            &image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(400, 300, image::Rgba([40, 90, 200, 255]))),
            image::ImageFormat::Gif,
        );
        let c = picture_within("loop.gif".into(), &gif, gif.len() - 1).unwrap();
        let Content::Picture { mime, size, bytes } = &c.content else { panic!("a picture") };
        assert_eq!(*mime, "image/png", "a GIF over the limit goes as its first frame, as PNG");
        assert!(bytes.len() < gif.len() && size[0] * 3 == size[1] * 4, "{size:?}: within the limit, same shape");
        assert!(a.thumb.is_some() && b.thumb.is_some() && c.thumb.is_some());
        let under = encode(&noise(60, 40, false), image::ImageFormat::Png);
        let d = picture_within("small.png".into(), &under, limit).unwrap();
        assert!(matches!(&d.content, Content::Picture { bytes, .. } if bytes.as_slice() == under.as_slice()), "under the limit: as it came");
        assert_eq!(MAX_SENT_BYTES, 3_750_000, "its base64 stays under 5 MB");
    }

    #[test]
    fn bad_files_are_refused_with_a_reason() {
        let d = dir("bad");
        let fake = d.join("fake.png");
        std::fs::write(&fake, b"not a picture at all").unwrap();
        assert!(read(&fake).unwrap_err().contains("fake.png"));
        let text = d.join("big.txt");
        std::fs::write(&text, vec![b'a'; 300 * 1024]).unwrap();
        assert!(read(&text).unwrap_err().contains("at most"));
        let binary = d.join("bin.json");
        std::fs::write(&binary, [0xff, 0xfe, 0x00, 0x80]).unwrap();
        assert!(read(&binary).unwrap_err().contains("UTF-8"));
        let other = d.join("movie.mov");
        std::fs::write(&other, b"x").unwrap();
        assert!(read(&other).unwrap_err().contains("can be attached"));
        assert!(read(&d.join("missing.png")).is_err());
        assert_eq!(kind(Path::new("part.STEP")), Kind::Cad);
        assert_eq!(kind(Path::new("mount.stl")), Kind::Cad);
        assert_eq!(kind(Path::new("frame.3dm")), Kind::Cad);
        assert!(from_rgba("x".into(), 2, 2, &[0; 15]).is_err(), "wrong length");
        assert!(from_rgba("x".into(), 0, 2, &[]).is_err());
        assert!(from_rgba("x".into(), usize::MAX, 2, &[]).is_err());
        let ok = from_rgba("pasted.png".into(), 3, 2, &[255; 24]).unwrap();
        assert!(matches!(ok.content, Content::Picture { size: [3, 2], .. }));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Review of #1744: the refusals were English whatever the app's language.
    #[test]
    fn refusals_speak_the_apps_language() {
        let d = dir("lang");
        let binary = d.join("bin.json");
        std::fs::write(&binary, [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let other = d.join("movie.mov");
        std::fs::write(&other, b"x").unwrap();
        {
            let _ja = crate::i18n::scope("ja");
            assert!(read(&binary).unwrap_err().contains("UTF-8 のテキストではありません"));
            assert!(read(&other).unwrap_err().starts_with("movie.mov: 添付できるのは"));
        }
        {
            let _zh = crate::i18n::scope("zh");
            assert!(read(&binary).unwrap_err().contains("不是 UTF-8 文本"));
        }
        assert!(read(&binary).unwrap_err().contains("not UTF-8 text"), "English again outside the scope");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Every picture format the agents read goes as it is (PNG, JPEG, GIF, WebP); BMP and TIFF are
    /// sent as PNG; SVG goes as its source text; HEIC says why not.
    #[test]
    fn each_picture_format_goes_the_way_the_agent_reads_it() {
        let d = dir("formats");
        let img = image::RgbaImage::from_pixel(30, 20, image::Rgba([20, 120, 220, 255]));
        for (file, format, mime) in [
            ("a.gif", image::ImageFormat::Gif, "image/gif"),
            ("a.webp", image::ImageFormat::WebP, "image/webp"),
            ("a.bmp", image::ImageFormat::Bmp, "image/png"),
            ("a.tiff", image::ImageFormat::Tiff, "image/png"),
        ] {
            let path = d.join(file);
            image::DynamicImage::ImageRgba8(img.clone()).save_with_format(&path, format).unwrap();
            let a = read(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
            let Content::Picture { mime: m, size, bytes } = &a.content else { panic!("{file}: a picture") };
            assert_eq!((*m, *size), (mime, [30, 20]), "{file}");
            if mime != "image/png" {
                assert_eq!(bytes.as_slice(), std::fs::read(&path).unwrap().as_slice(), "{file}: sent as it came");
            }
            assert!(a.thumb.is_some(), "{file}: a thumbnail");
        }
        let svg = d.join("sketch.svg");
        std::fs::write(&svg, "<svg xmlns='http://www.w3.org/2000/svg'><rect width='10' height='5'/></svg>").unwrap();
        assert!(matches!(&read(&svg).unwrap().content, Content::Text(t) if t.contains("<rect")));
        let heic = d.join("photo.HEIC");
        std::fs::write(&heic, b"....ftypheic").unwrap();
        assert!(read(&heic).unwrap_err().contains("HEIC"));
        for ext in ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "svg", "md", "txt", "step", "FCStd", "3dm", "stl", "3mf", "obj", "glb"] {
            assert!(extensions().contains(&ext), "{ext} can be chosen");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Review of #1744: eight 256 KB text files took a message to about 2 MB, past the prompt cap
    /// that only trimmed the typed text. The files' text now gets a budget: a file that doesn't
    /// fit is cut and says so, and a file past it is named only.
    #[test]
    fn attached_text_is_kept_within_the_prompt_budget() {
        let file = |n: &str, len: usize| Attachment { id: 0, name: n.into(), content: Content::Text(Arc::new("x".repeat(len))), thumb: None };
        let files = vec![file("a.md", 600), file("b.md", 600), file("c.md", 600)];
        let n = note(&files, true, 1000);
        assert!(n.contains("`a.md`") && n.contains("`b.md`") && n.contains("`c.md`"), "every file is named: {n}");
        assert!(n.matches('x').count() <= 1000, "the files' text stays within the budget");
        assert!(n.contains("200 more characters"), "a cut file says how much is missing: {n}");
        assert!(n.contains("left out"), "a file past the budget is named only: {n}");
        assert_eq!(note(&files, true, usize::MAX).matches('x').count(), 1800, "with room, every file goes whole");
    }

    #[test]
    fn the_message_names_its_files_and_carries_the_pictures() {
        let pic = from_rgba("target.png".into(), 2, 2, &[9; 16]).unwrap();
        let text = Attachment { id: 0, name: "brief.md".into(), content: Content::Text(Arc::new("wall 2 mm\n".into())), thumb: None };
        let part = Attachment::imported("motor.step".into());
        let files = vec![pic, text, part];
        let n = note(&files, true, usize::MAX);
        assert!(n.contains("Attached pictures: target.png."), "{n}");
        assert!(n.contains("Attached file `brief.md`:\n```\nwall 2 mm\n```"), "{n}");
        assert!(n.contains("`motor.step` to the design"), "{n}");
        assert!(note(&files, false, usize::MAX).contains("not sent"));
        let p = parts(&files);
        assert_eq!(p.len(), 1);
        assert!(matches!(&p[0], PromptPart::Image { mime, base64 } if mime == "image/png" && base64.starts_with("iVBORw0KGgo")));
    }
}
