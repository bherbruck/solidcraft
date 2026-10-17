//! System font loading and fallback setup for multi-language (CJK: Japanese, Chinese, Korean;
//! Thai) support.
//!
//! The UI's Latin font always comes first; a CJK font (then a Hangul font, then a Thai font) is only
//! ever appended as a fallback, so English text renders exactly as before and only glyphs the Latin
//! font lacks use the CJK face. The Hangul fallback covers Korean where the CJK font found (Meiryo,
//! Hiragino) lacks Hangul; the Thai fallback covers Thai typed in a name or the agent chat (#1902). The order of the CJK and Chinese faces follows the UI language
//! (see [`fallback_order`]): one face drawing a whole word keeps its glyphs on one baseline.

use std::sync::Arc;

/// Name of the CJK fallback entry in [`egui::FontDefinitions`].
pub const CJK_FALLBACK: &str = "cjk_fallback";

/// Name of the Chinese fallback entry in [`egui::FontDefinitions`], after [`CJK_FALLBACK`].
/// A Japanese-only CJK font (Meiryo, Hiragino Gothic) lacks simplified characters, so
/// Chinese falls through to this one per glyph.
pub const CHINESE_FALLBACK: &str = "chinese_fallback";

/// Name of the chosen interface font in [`egui::FontDefinitions`], first in the Proportional
/// family when one is set (#1290).
pub const UI_FONT: &str = "ui_font";

/// The interface font a preference names (#1290): an installed family, a font file, or the
/// sketch text's built-in family. `None` for the default (empty) or a font that can't be read.
pub fn ui_font(name: &str) -> Option<egui::FontData> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let data = solidcraft_engine::sketch::font_data(Some(name)).ok()?;
    ttf_parser::Face::parse(data.bytes(), data.index).ok()?;
    let mut font = egui::FontData::from_owned(data.bytes().to_vec());
    font.index = data.index;
    Some(font)
}

/// `fonts` with `font` first in the Proportional family (menus, dialogs, labels); the default
/// face and the fallbacks stay after it for the glyphs it lacks.
pub fn with_ui_font(mut fonts: egui::FontDefinitions, font: egui::FontData) -> egui::FontDefinitions {
    fonts.font_data.insert(UI_FONT.to_owned(), Arc::new(font));
    let list = fonts.families.entry(egui::FontFamily::Proportional).or_default();
    list.retain(|entry| entry != UI_FONT);
    list.insert(0, UI_FONT.to_owned());
    fonts
}

/// Name of the Hangul fallback entry in [`egui::FontDefinitions`], after the CJK ones.
pub const HANGUL_FALLBACK: &str = "hangul_fallback";

/// Name of the Thai fallback entry in [`egui::FontDefinitions`], last (#1902).
pub const THAI_FALLBACK: &str = "thai_fallback";

/// UI languages whose text needs the CJK font. The web app's bundled font covers Japanese only;
/// add Chinese or Korean here together with a font that covers them.
pub const CJK_LANGUAGES: &[&str] = &["ja", "zh", "ko"];

/// Whether the UI language needs a CJK font (the web app fetches one only then).
pub fn needs_cjk_font(language: &str) -> bool {
    CJK_LANGUAGES.contains(&language)
}

/// Whether `text` has characters only the CJK fallback draws: kana, CJK ideographs and their
/// punctuation, and full-width forms. Hangul is left out: the web app's font has none.
pub fn has_cjk(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(u32::from(c),
            0x3000..=0x30FF // CJK punctuation, Hiragana, Katakana
            | 0x31F0..=0x31FF // Katakana phonetic extensions
            | 0x3400..=0x4DBF // CJK Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0xFF00..=0xFFEF // Half-width and full-width forms
        )
    })
}

/// Whether any text painted so far this frame has CJK characters (see [`has_cjk`]). The web app
/// calls it after drawing the UI, until its CJK font is loaded, so a Japanese design name, file
/// name or typed text fetches the font whatever the UI language (#720).
pub fn painted_cjk(ctx: &egui::Context) -> bool {
    let mut layers: Vec<egui::LayerId> = ctx.memory(|m| m.layer_ids().collect());
    layers.push(egui::LayerId::background());
    ctx.graphics(|g| layers.iter().filter_map(|l| g.get(*l)).any(|list| list.all_entries().any(|c| shape_has_cjk(&c.shape))))
}

fn shape_has_cjk(shape: &egui::Shape) -> bool {
    match shape {
        egui::Shape::Text(t) => has_cjk(&t.galley.job.text),
        egui::Shape::Vec(shapes) => shapes.iter().any(shape_has_cjk),
        _ => false,
    }
}

/// `fonts` with `bytes` appended as the last fallback of the Proportional and Monospace
/// families, after the Latin UI font. `None` when the bytes are not a font.
pub fn with_cjk_fallback(fonts: egui::FontDefinitions, bytes: Vec<u8>) -> Option<egui::FontDefinitions> {
    with_fallback(fonts, CJK_FALLBACK, bytes)
}

/// `fonts` with `bytes` appended under `name` as the last fallback of the Proportional and
/// Monospace families. `None` when the bytes are not a font.
fn with_fallback(mut fonts: egui::FontDefinitions, name: &str, bytes: Vec<u8>) -> Option<egui::FontDefinitions> {
    ttf_parser::Face::parse(&bytes, 0).ok()?;
    fonts.font_data.insert(name.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        list.retain(|entry| entry != name);
        list.push(name.to_owned());
    }
    Some(fonts)
}

/// Install font bytes as fallback for Proportional and Monospace font families.
/// Returns true if the font was parsed and installed successfully.
pub fn install_cjk_font(ctx: &egui::Context, bytes: Vec<u8>) -> bool {
    match with_cjk_fallback(egui::FontDefinitions::default(), bytes) {
        Some(fonts) => {
            ctx.set_fonts(fonts);
            true
        }
        None => false,
    }
}

/// Lower-case file-name fragments that identify CJK fonts. Deliberately specific: generic
/// words such as "noto" or "gothic" also match Latin-only fonts (NotoSans-Regular, Century Gothic).
pub const CJK_FONT_KEYWORDS: &[&str] = &[
    "cjk",
    "notosansjp",
    "notoserifjp",
    "bizud",
    "biz-ud",
    "yugoth",
    "meiryo",
    "msgothic",
    "hiragino",
    "pingfang",
    "songti",
    "arial unicode",
    "ipag",
    "ipaexg",
    "takao",
    "vlgothic",
    "droidsansfallback",
];

/// Whether a font file name looks like a CJK font.
pub fn is_cjk_font_name(file_name: &str) -> bool {
    let name = file_name.to_ascii_lowercase();
    CJK_FONT_KEYWORDS.iter().any(|k| name.contains(k))
}

/// Lower-case file-name fragments that identify fonts covering simplified Chinese. Deliberately
/// separate from [`CJK_FONT_KEYWORDS`]: many Japanese fonts (Hiragino Gothic, Meiryo, Yu Gothic)
/// match those but lack simplified characters, so a Chinese fallback searches these instead.
pub const CHINESE_FONT_KEYWORDS: &[&str] = &[
    "pingfang",
    "hiragino sans gb",
    "stheiti",
    "songti",
    "arial unicode",
    "msyh",
    "yahei",
    "simhei",
    "simsun",
    "wqy",
    "wenquanyi",
    "notosanscjk",
    "notosanssc",
    "droidsansfallback",
    "sourcehanss",
    "siyuan",
];

/// Whether a font file name looks like a font covering simplified Chinese.
pub fn is_chinese_font_name(file_name: &str) -> bool {
    let name = file_name.to_ascii_lowercase();
    CHINESE_FONT_KEYWORDS.iter().any(|k| name.contains(k))
}

/// Session state of the web app's on-demand CJK font. Pure state so it can be tested off the web:
/// the shell does the fetch when [`CjkFontLoader::tick`] asks for it.
#[derive(Debug, Default)]
pub struct CjkFontLoader {
    status: CjkFontStatus,
    /// The saved language while this session falls back to English after a failed load.
    saved_language: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CjkFontStatus {
    #[default]
    Unloaded,
    Loading,
    Installed,
    Failed,
}

impl CjkFontLoader {
    pub fn status(&self) -> CjkFontStatus {
        self.status
    }

    /// Called every frame with the live language. Returns true when the font fetch should start
    /// now (only for a CJK language, only once). After a failed load a CJK language falls back to
    /// English for this session; the saved preference is kept (see [`Self::persisted_language`]).
    pub fn tick(&mut self, language: &mut String) -> bool {
        if self.saved_language.is_some() && language != "en" {
            // The user picked a language themselves since the fallback: that is the preference now.
            self.saved_language = None;
        }
        if !needs_cjk_font(language) {
            return false;
        }
        match self.status {
            CjkFontStatus::Unloaded => {
                self.status = CjkFontStatus::Loading;
                true
            }
            CjkFontStatus::Failed => {
                self.fall_back(language);
                false
            }
            CjkFontStatus::Loading | CjkFontStatus::Installed => false,
        }
    }

    /// CJK text is on screen (whatever the UI language). Returns true when the font fetch should
    /// start now: once, and never again after a failed load (the UI language stays as it is).
    pub fn text_needs_font(&mut self) -> bool {
        if self.status == CjkFontStatus::Unloaded {
            self.status = CjkFontStatus::Loading;
            true
        } else {
            false
        }
    }

    /// Whether the session still watches painted text for CJK characters.
    pub fn watching_text(&self) -> bool {
        self.status == CjkFontStatus::Unloaded
    }

    /// Record the outcome of the fetch and install.
    pub fn finished(&mut self, installed: bool, language: &mut String) {
        if installed {
            self.status = CjkFontStatus::Installed;
        } else {
            self.status = CjkFontStatus::Failed;
            if needs_cjk_font(language) {
                self.fall_back(language);
            }
        }
    }

    fn fall_back(&mut self, language: &mut String) {
        self.saved_language = Some(std::mem::replace(language, "en".to_owned()));
    }

    /// The language to write to storage: the user's own choice, not this session's fallback.
    pub fn persisted_language<'a>(&'a self, live: &'a str) -> &'a str {
        self.saved_language.as_deref().unwrap_or(live)
    }
}

/// Candidate font paths to look for on different operating systems.
#[cfg(not(target_arch = "wasm32"))]
const FONT_CANDIDATES: &[&str] = &[
    // Windows
    "C:\\Windows\\Fonts\\meiryo.ttc",
    "C:\\Windows\\Fonts\\YuGothM.ttc",
    "C:\\Windows\\Fonts\\YuGothR.ttc",
    "C:\\Windows\\Fonts\\msgothic.ttc",
    "C:\\Windows\\Fonts\\BIZ-UDGothicR.ttc",
    "C:\\Windows\\Fonts\\BIZ-UDMinchoM.ttc",
    "C:\\Windows\\Fonts\\yumin.ttf",
    // macOS
    "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
    "/System/Library/Fonts/Hiragino Sans W3.ttc",
    "/System/Library/Fonts/Hiragino Sans W4.ttc",
    "/System/Library/Fonts/Hiragino Sans W6.ttc",
    "/System/Library/Fonts/Supplemental/Hiragino Sans W3.ttc",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/Supplemental/Songti.ttc",
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/Library/Fonts/Arial Unicode.ttf",
    "/Library/Fonts/Hiragino Sans W3.ttc",
    // Linux
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJKjp-Regular.otf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/opentype/ipafont-gothic/ipag.ttf",
    "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
    "/usr/share/fonts/truetype/takao-gothic/TakaoPGothic.ttf",
    "/usr/share/fonts/truetype/takao-gothic/TakaoGothic.ttf",
    "/usr/share/fonts/truetype/vlgothic/VL-PGothic-Regular.ttf",
    "/usr/share/fonts/truetype/ipafont/ipag.ttf",
    "/usr/share/fonts/truetype/ipaexfont/ipaexg.ttf",
    "/usr/share/fonts/opentype/ipafont/ipag.ttf",
    "/usr/share/fonts/opentype/ipaexfont/ipaexg.ttf",
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
    "/usr/share/fonts/cjkuni-uming/uming.ttc",
];

/// Hangul fonts, appended after the CJK fallback so Korean renders even when the CJK font found
/// is a Japanese one without Hangul (Meiryo, Hiragino).
#[cfg(not(target_arch = "wasm32"))]
const HANGUL_FONT_CANDIDATES: &[&str] = &[
    // Windows: Malgun Gothic
    "C:\\Windows\\Fonts\\malgun.ttf",
    // macOS: Apple SD Gothic Neo
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
    // Linux: Nanum Gothic (Noto Sans CJK already covers Hangul)
    "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
    "/usr/share/fonts/nanum/NanumGothic.ttf",
];

/// Thai fonts, appended last: none of the Latin or CJK faces draw Thai, so without one Thai text
/// shows as empty boxes (#1902).
#[cfg(not(target_arch = "wasm32"))]
const THAI_FONT_CANDIDATES: &[&str] = &[
    // Windows: Leelawadee UI (Windows 8 and later), Leelawadee, Tahoma
    "C:\\Windows\\Fonts\\LeelawUI.ttf",
    "C:\\Windows\\Fonts\\leelawad.ttf",
    "C:\\Windows\\Fonts\\tahoma.ttf",
    // macOS: Thonburi, Ayuthaya
    "/System/Library/Fonts/Thonburi.ttc",
    "/System/Library/Fonts/Supplemental/Thonburi.ttc",
    "/System/Library/Fonts/Supplemental/Ayuthaya.ttf",
    "/Library/Fonts/Thonburi.ttf",
    // Linux: Noto Sans Thai, TLWG Loma and Garuda
    "/usr/share/fonts/noto/NotoSansThai-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansThai-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSansThai-Regular.ttf",
    "/usr/share/fonts/google-noto-vf/NotoSansThai[wght].ttf",
    "/usr/share/fonts/opentype/noto/NotoSansThai-Regular.ttf",
    "/usr/share/fonts/truetype/tlwg/Loma.ttf",
    "/usr/share/fonts/opentype/tlwg/Loma.otf",
    "/usr/share/fonts/truetype/tlwg/Garuda.ttf",
    "/usr/share/fonts/opentype/tlwg/Garuda.otf",
    "/usr/share/fonts/tlwg/Loma.otf",
];

/// Fonts covering simplified Chinese, appended after the CJK fallback so Chinese renders even
/// when the CJK font found is a Japanese one without simplified characters (Hiragino Gothic,
/// Meiryo, Yu Gothic).
#[cfg(not(target_arch = "wasm32"))]
const CHINESE_FONT_CANDIDATES: &[&str] = &[
    // Windows: Microsoft YaHei
    "C:\\Windows\\Fonts\\msyh.ttc",
    "C:\\Windows\\Fonts\\msyh.ttf",
    // macOS: PingFang SC, Hiragino Sans GB, Songti SC, STHeiti, Arial Unicode MS
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/System/Library/Fonts/Supplemental/Songti.ttc",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    // Linux: Noto Sans CJK SC
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
];

#[cfg(not(target_arch = "wasm32"))]
fn try_read_font(path: &std::path::Path) -> Option<Vec<u8>> {
    if let Ok(bytes) = std::fs::read(path)
        && ttf_parser::Face::parse(&bytes, 0).is_ok()
    {
        Some(bytes)
    } else {
        None
    }
}

/// Recursively search for matching font files up to a bounded depth, testing names with `matches`.
#[cfg(not(target_arch = "wasm32"))]
fn scan_dir_for_font(dir: &std::path::Path, depth_remaining: usize, matches: fn(&str) -> bool) -> Option<Vec<u8>> {
    if depth_remaining == 0 {
        return None;
    }
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();

    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_file() {
            let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let is_font = name.ends_with(".ttc") || name.ends_with(".ttf") || name.ends_with(".otf");
            if is_font
                && matches(&name)
                && let Some(bytes) = try_read_font(&p)
            {
                return Some(bytes);
            }
        } else if p.is_dir() {
            subdirs.push(p);
        }
    }

    for sub in subdirs {
        if let Some(bytes) = scan_dir_for_font(&sub, depth_remaining.saturating_sub(1), matches) {
            return Some(bytes);
        }
    }

    None
}

/// Find a CJK fallback font on the local system or in CRAFT_FONTS_DIR.
#[cfg(not(target_arch = "wasm32"))]
fn find_system_cjk_font() -> Option<Vec<u8>> {
    // 1. Check optional craft-fonts pack directory if specified.
    if let Ok(craft_dir) = std::env::var("CRAFT_FONTS_DIR") {
        let dir = std::path::Path::new(&craft_dir);
        let names = ["BIZUDPGothic-Regular.ttf", "BIZ-UDGothicR.ttc", "NotoSansCJK-Regular.ttc", "NotoSansJP-Regular.ttf"];
        for name in names {
            if let Some(bytes) = try_read_font(&dir.join(name)) {
                return Some(bytes);
            }
        }
    }

    // 2. Check well-known candidate paths.
    for path in FONT_CANDIDATES {
        if let Some(bytes) = try_read_font(std::path::Path::new(path)) {
            return Some(bytes);
        }
    }

    // 3. Platform-specific dynamic directory search.
    #[cfg(target_os = "windows")]
    {
        if let Ok(windir) = std::env::var("WINDIR").or_else(|_| std::env::var("SystemRoot")) {
            let fonts_dir = std::path::PathBuf::from(windir).join("Fonts");
            let names = ["meiryo.ttc", "YuGothM.ttc", "YuGothR.ttc", "msgothic.ttc", "BIZ-UDGothicR.ttc"];
            for name in names {
                if let Some(bytes) = try_read_font(&fonts_dir.join(name)) {
                    return Some(bytes);
                }
            }
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let user_fonts = std::path::PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts");
            if let Some(bytes) = scan_dir_for_font(&user_fonts, 2, is_cjk_font_name) {
                return Some(bytes);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let dirs = ["/System/Library/Fonts", "/System/Library/Fonts/Supplemental", "/Library/Fonts"];
        for d in dirs {
            if let Some(bytes) = scan_dir_for_font(std::path::Path::new(d), 2, is_cjk_font_name) {
                return Some(bytes);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let search_dirs = [std::path::PathBuf::from("/usr/share/fonts"), std::path::PathBuf::from("/usr/local/share/fonts")];
        for dir in &search_dirs {
            if let Some(bytes) = scan_dir_for_font(dir, 3, is_cjk_font_name) {
                return Some(bytes);
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            let user_fonts = std::path::PathBuf::from(home).join(".local/share/fonts");
            if let Some(bytes) = scan_dir_for_font(&user_fonts, 2, is_cjk_font_name) {
                return Some(bytes);
            }
        }
    }

    None
}

/// The fallback entries in the order they join each family for a UI language.
///
/// Chinese goes first for `zh`: a Japanese font (Meiryo, Yu Gothic) lacks the simplified
/// characters, so the rest of the word falls through to the Chinese font per glyph — and
/// epaint centres each face's own line height in the row, so glyphs from two faces of very
/// different height (Meiryo ≈ 2.0 em, Microsoft YaHei ≈ 1.4 em) sit at different heights
/// inside one word. One face for the whole word keeps them on one baseline.
/// Thai comes last whatever the language: no other face draws it.
pub fn fallback_order(language: &str) -> [&'static str; 4] {
    if language == "zh" {
        [CHINESE_FALLBACK, CJK_FALLBACK, HANGUL_FALLBACK, THAI_FALLBACK]
    } else {
        [CJK_FALLBACK, CHINESE_FALLBACK, HANGUL_FALLBACK, THAI_FALLBACK]
    }
}

/// Configure font definitions with system CJK, Chinese, Hangul and Thai fallbacks where available,
/// ordered for the UI language (see [`fallback_order`]), after the interface font `ui_font`
/// when one is chosen (see [`ui_font`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn setup_fonts_for(ctx: &egui::Context, language: &str, ui_font: &str) {
    let cjk = find_system_cjk_font();
    let chinese = find_system_chinese_font().filter(|bytes| cjk.as_ref() != Some(bytes));
    let hangul = HANGUL_FONT_CANDIDATES.iter().find_map(|path| try_read_font(std::path::Path::new(path)));
    let thai = THAI_FONT_CANDIDATES.iter().find_map(|path| try_read_font(std::path::Path::new(path)));
    let mut fonts = egui::FontDefinitions::default();
    // Set the fonts also when only the interface font changes, so going back to the default
    // takes the chosen one away.
    let mut found = !ui_font.trim().is_empty();
    if let Some(font) = self::ui_font(ui_font) {
        fonts = with_ui_font(fonts, font);
    }
    for name in fallback_order(language) {
        let bytes = match name {
            CJK_FALLBACK => cjk.clone(),
            CHINESE_FALLBACK => chinese.clone(),
            HANGUL_FALLBACK => hangul.clone(),
            _ => thai.clone(),
        };
        if let Some(bytes) = bytes
            && let Some(with_fallback) = with_fallback(fonts.clone(), name, bytes)
        {
            fonts = with_fallback;
            found = true;
        }
    }
    if found {
        ctx.set_fonts(fonts);
    }
}

/// Configure font definitions with system fallbacks for an English UI (all fallbacks are
/// installed whatever the language, so CJK text in a design still renders; only their order
/// follows the language, see [`setup_fonts_for`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn setup_fonts(ctx: &egui::Context) {
    setup_fonts_for(ctx, "en", "");
}

/// Find a font covering simplified Chinese in the known candidate paths or by scanning the
/// system font directories for [`CHINESE_FONT_KEYWORDS`] names.
#[cfg(not(target_arch = "wasm32"))]
fn find_system_chinese_font() -> Option<Vec<u8>> {
    for path in CHINESE_FONT_CANDIDATES {
        if let Some(bytes) = try_read_font(std::path::Path::new(path)) {
            return Some(bytes);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let dirs = ["/System/Library/Fonts", "/System/Library/Fonts/Supplemental", "/Library/Fonts"];
        for d in dirs {
            if let Some(bytes) = scan_dir_for_font(std::path::Path::new(d), 2, is_chinese_font_name) {
                return Some(bytes);
            }
        }
    }
    None
}

/// On web, setup_fonts does not perform synchronous disk reads.
/// Use asynchronous loading through [`solidcraft_web`] or host scripts.
#[cfg(target_arch = "wasm32")]
pub fn setup_fonts_for(_ctx: &egui::Context, _language: &str, _ui_font: &str) {}

#[cfg(target_arch = "wasm32")]
pub fn setup_fonts(_ctx: &egui::Context) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interface_font_comes_first_and_keeps_the_fallbacks() {
        // #1290: the chosen font leads the Proportional family; the default face stays after it.
        assert!(ui_font("").is_none());
        assert!(ui_font("No Such Font 1290").is_none());
        let font = ui_font(solidcraft_engine::sketch::DEFAULT_FONT_FAMILY).expect("the built-in sketch font reads");
        let defaults = egui::FontDefinitions::default();
        let first_default = defaults.families[&egui::FontFamily::Proportional][0].clone();
        let fonts = with_ui_font(defaults, font.clone());
        let list = &fonts.families[&egui::FontFamily::Proportional];
        assert_eq!(list[0], UI_FONT);
        assert!(list.contains(&first_default), "{list:?}");
        assert!(!fonts.families[&egui::FontFamily::Monospace].contains(&UI_FONT.to_string()));
        // Choosing again replaces it rather than adding a second entry.
        let again = with_ui_font(fonts, font);
        assert_eq!(again.families[&egui::FontFamily::Proportional].iter().filter(|n| *n == UI_FONT).count(), 1);
    }

    fn latin_font_bytes() -> Vec<u8> {
        let defaults = egui::FontDefinitions::default();
        let name = &defaults.families[&egui::FontFamily::Proportional][0];
        defaults.font_data[name].font.to_vec()
    }

    #[test]
    fn default_ui_language_is_english() {
        assert_eq!(crate::prefs::Prefs::default().language, "en");
        assert!(!needs_cjk_font("en"));
    }

    #[test]
    fn cjk_font_is_a_fallback_after_the_latin_ui_font() {
        let defaults = egui::FontDefinitions::default();
        let fonts = with_cjk_fallback(defaults.clone(), latin_font_bytes()).unwrap();
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let list = &fonts.families[&family];
            assert_eq!(list.first(), defaults.families[&family].first(), "the Latin UI font stays first");
            assert_eq!(list.last().map(String::as_str), Some(CJK_FALLBACK), "CJK is the last fallback");
        }
        assert!(with_cjk_fallback(defaults, b"not a font".to_vec()).is_none());
    }

    #[test]
    fn a_chinese_ui_puts_the_chinese_face_before_the_japanese_one() {
        assert_eq!(fallback_order("zh"), [CHINESE_FALLBACK, CJK_FALLBACK, HANGUL_FALLBACK, THAI_FALLBACK]);
        for lang in ["en", "ja", "ko", "de"] {
            assert_eq!(
                fallback_order(lang),
                [CJK_FALLBACK, CHINESE_FALLBACK, HANGUL_FALLBACK, THAI_FALLBACK],
                "{lang} keeps the Japanese font first"
            );
        }
        let defaults = egui::FontDefinitions::default();
        let mut fonts = defaults.clone();
        for name in fallback_order("zh") {
            fonts = with_fallback(fonts, name, latin_font_bytes()).unwrap();
        }
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let list = &fonts.families[&family];
            assert_eq!(list.first(), defaults.families[&family].first(), "the Latin UI font stays first");
            assert_eq!(&list[list.len() - 4..], [CHINESE_FALLBACK, CJK_FALLBACK, HANGUL_FALLBACK, THAI_FALLBACK]);
        }
    }

    /// Thai typed into the agent chat drew as empty boxes (#1902): every Thai candidate this
    /// machine has must parse and cover Thai, and the fonts set up must then draw it.
    #[test]
    fn a_thai_fallback_draws_thai() {
        let found: Vec<Vec<u8>> = THAI_FONT_CANDIDATES.iter().filter_map(|path| try_read_font(std::path::Path::new(path))).collect();
        for bytes in &found {
            let face = ttf_parser::Face::parse(bytes, 0).unwrap();
            for c in "สวัสดี".chars() {
                assert!(face.glyph_index(c).is_some(), "{c:?} is in the Thai font");
            }
        }
        let thai = |fonts: egui::FontDefinitions| {
            let ctx = egui::Context::default();
            ctx.set_fonts(fonts);
            ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
            ctx.fonts_mut(|f| f.has_glyphs(&egui::FontId::proportional(14.0), "สวัสดี"))
        };
        assert!(!thai(egui::FontDefinitions::default()), "the built-in faces have no Thai");
        let Some(bytes) = found.into_iter().next() else { return };
        assert!(thai(with_fallback(egui::FontDefinitions::default(), THAI_FALLBACK, bytes).unwrap()), "Thai draws with the fallback");
    }

    #[test]
    fn hangul_font_falls_back_after_the_cjk_font() {
        let defaults = egui::FontDefinitions::default();
        let cjk = with_cjk_fallback(defaults.clone(), latin_font_bytes()).unwrap();
        let fonts = with_fallback(cjk, HANGUL_FALLBACK, latin_font_bytes()).unwrap();
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let list = &fonts.families[&family];
            assert_eq!(list.first(), defaults.families[&family].first(), "the Latin UI font stays first");
            assert_eq!(&list[list.len() - 2..], [CJK_FALLBACK, HANGUL_FALLBACK], "Hangul follows the CJK fallback");
        }
    }

    #[test]
    fn directory_scan_matches_cjk_fonts_only() {
        for cjk in [
            "NotoSansCJK-Regular.ttc",
            "NotoSansJP-Regular.ttf",
            "BIZUDPGothic-Regular.ttf",
            "YuGothM.ttc",
            "meiryo.ttc",
            "msgothic.ttc",
            "Hiragino Sans W3.ttc",
            "ipag.ttf",
            "TakaoPGothic.ttf",
            "DroidSansFallbackFull.ttf",
        ] {
            assert!(is_cjk_font_name(cjk), "{cjk} is a CJK font");
        }
        for latin in ["NotoSans-Regular.ttf", "NotoSerif-Bold.ttf", "NotoMono-Regular.ttf", "GOTHIC.TTF", "DejaVuSans.ttf", "DroidSans.ttf"] {
            assert!(!is_cjk_font_name(latin), "{latin} is not a CJK font");
        }
    }

    #[test]
    fn web_font_is_fetched_only_for_a_cjk_language() {
        let mut loader = CjkFontLoader::default();
        for lang in ["en", "de"] {
            let mut language = lang.to_owned();
            assert!(!loader.tick(&mut language), "no CJK font fetch for {lang}");
        }
        let mut language = "ja".to_owned();
        assert!(loader.tick(&mut language), "Japanese starts the fetch");
        assert!(!loader.tick(&mut language), "the fetch starts once");
        loader.finished(true, &mut language);
        assert_eq!(loader.status(), CjkFontStatus::Installed);
        assert_eq!(language, "ja");
    }

    #[test]
    fn cjk_text_is_recognised() {
        for cjk in ["テスト", "ひらがな", "部品", "ﾃｽﾄ", "Bracket（改）", "Plate 板"] {
            assert!(has_cjk(cjk), "{cjk} has CJK characters");
        }
        for other in ["", "Bracket", "Ø12 × 3 mm", "Größe", "Привет", "한국어", "émoji ✓"] {
            assert!(!has_cjk(other), "{other} has no CJK characters the fallback draws");
        }
    }

    /// An English UI fetches the font once CJK text is painted (#720), and a failed fetch leaves
    /// the language alone.
    #[test]
    fn cjk_text_fetches_the_font_with_an_english_ui() {
        let mut loader = CjkFontLoader::default();
        let mut language = "en".to_owned();
        assert!(!loader.tick(&mut language));
        assert!(loader.watching_text());
        assert!(loader.text_needs_font(), "CJK text starts the fetch");
        assert!(!loader.text_needs_font(), "the fetch starts once");
        assert!(!loader.tick(&mut language), "no second fetch from the language");
        assert!(!loader.watching_text());
        loader.finished(false, &mut language);
        assert_eq!(loader.status(), CjkFontStatus::Failed);
        assert_eq!(language, "en");
        assert!(!loader.text_needs_font(), "no retry loop after a failure");

        let mut loader = CjkFontLoader::default();
        assert!(loader.text_needs_font());
        loader.finished(true, &mut language);
        assert_eq!(loader.status(), CjkFontStatus::Installed);
        // Switching to Japanese afterwards needs no second fetch.
        let mut ja = "ja".to_owned();
        assert!(!loader.tick(&mut ja));
    }

    /// The painted-text check finds CJK in the frame's labels and nothing else.
    #[test]
    fn painted_cjk_sees_labels_in_the_frame() {
        let ctx = egui::Context::default();
        let run = |text: &'static str| {
            let mut seen = false;
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.label(text);
                seen = painted_cjk(ui.ctx());
            });
            seen
        };
        assert!(!run("Untitled"));
        assert!(run("テスト"));
    }

    #[test]
    fn failed_font_fetch_falls_back_for_the_session_without_changing_the_saved_language() {
        let mut loader = CjkFontLoader::default();
        let mut language = "ja".to_owned();
        assert!(loader.tick(&mut language));
        loader.finished(false, &mut language);
        assert_eq!(language, "en", "this session shows English instead of tofu");
        assert_eq!(loader.persisted_language(&language), "ja", "the saved preference stays Japanese");
        assert!(!loader.tick(&mut language));
        assert_eq!(loader.persisted_language(&language), "ja");

        // Choosing a language afterwards makes that the saved preference.
        language = "de".to_owned();
        assert!(!loader.tick(&mut language));
        assert_eq!(loader.persisted_language(&language), "de");
    }
}
