//! Embedded offline message catalogs. UI messages use stable IDs; engine display labels
//! pass through an adapter without changing engine or scenario identifiers.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::OnceLock;

type Messages = BTreeMap<String, String>;

/// Register locales here after adding their catalogs. Missing translations use English.
pub struct Catalog {
    pub id: &'static str,
    pub name: &'static str,
    source: &'static str,
    messages: OnceLock<Messages>,
}

impl Catalog {
    const fn new(id: &'static str, name: &'static str, source: &'static str) -> Self {
        Self { id, name, source, messages: OnceLock::new() }
    }

    fn messages(&'static self) -> &'static Messages {
        self.messages.get_or_init(|| match serde_json::from_str(self.source) {
            Ok(messages) => messages,
            Err(error) => {
                log::error!("invalid embedded {} message catalog: {error}", self.id);
                Messages::new()
            }
        })
    }
}

static ENGLISH: Catalog = Catalog::new("en", "English", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/en.json")));
static CZECH: Catalog = Catalog::new("cs", "Čeština", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/cs.json")));
static GERMAN: Catalog = Catalog::new("de", "Deutsch", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/de.json")));
static SPANISH: Catalog = Catalog::new("es", "Español (España)", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/es.json")));
static FRENCH: Catalog = Catalog::new("fr", "Français", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/fr.json")));
static GALICIAN: Catalog = Catalog::new("gl", "Galego", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/gl.json")));
static JAPANESE: Catalog = Catalog::new("ja", "日本語", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/ja.json")));
static KOREAN: Catalog = Catalog::new("ko", "한국어", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/ko.json")));
static PORTUGUESE_BR: Catalog =
    Catalog::new("pt-BR", "Português (Brasil)", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/pt-BR.json")));
static RUSSIAN: Catalog = Catalog::new("ru", "Русский", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/ru.json")));
static TURKISH: Catalog = Catalog::new("tr", "Türkçe", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/tr.json")));
static UKRAINIAN: Catalog = Catalog::new("uk", "Українська", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/uk.json")));
static CHINESE: Catalog = Catalog::new("zh", "中文", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/zh.json")));
/// English first (the fallback), then by language id, one per line so that adding a language
/// doesn't edit a line another addition edits.
#[rustfmt::skip]
pub const LOCALES: &[&Catalog] = &[
    &ENGLISH,
    &CZECH,
    &GERMAN,
    &SPANISH,
    &FRENCH,
    &GALICIAN,
    &JAPANESE,
    &KOREAN,
    &PORTUGUESE_BR,
    &RUSSIAN,
    &TURKISH,
    &UKRAINIAN,
    &CHINESE,
];

pub fn is_supported(id: &str) -> bool {
    LOCALES.iter().any(|catalog| catalog.id == id)
}

/// Immutable locale reference. Catalogs parse once; lookups are logarithmic.
#[derive(Clone, Copy)]
pub struct Locale(&'static Catalog);

impl Locale {
    pub fn for_language(id: &str) -> Self {
        Self(LOCALES.iter().copied().find(|catalog| catalog.id == id).unwrap_or(&ENGLISH))
    }

    pub fn name(self) -> &'static str {
        self.0.name
    }

    /// Stable message ID → translation → English → ID.
    pub fn text(self, id: &str) -> &str {
        self.0.messages().get(id).or_else(|| ENGLISH.messages().get(id)).map_or(id, String::as_str)
    }

    /// Translate source labels from engine/UI tables, preserving unknown labels.
    /// Also accepts stable IDs while table helpers migrate away from source text.
    pub fn label(self, label: &str) -> &str {
        static LABEL_IDS: OnceLock<BTreeMap<&'static str, &'static str>> = OnceLock::new();
        let ids = LABEL_IDS.get_or_init(|| {
            ENGLISH.messages().iter().filter(|(_, text)| !text.contains('{')).map(|(id, text)| (text.as_str(), id.as_str())).collect()
        });
        ids.get(label).map_or_else(|| self.text(label), |id| self.text(id))
    }

    pub fn format(self, id: &str, args: &[(&str, &str)]) -> String {
        interpolate(self.text(id), args)
    }

    /// The default names SolidCraft gives (`Sketch1`, `Body2`, `Component3`), translated.
    fn default_name(self, text: &str) -> Option<String> {
        for (prefix, id) in [("Sketch", "browser.sketch_name"), ("Body", "browser.body_name"), ("Component", "browser.component_name")] {
            if let Some(number) = text.strip_prefix(prefix)
                && !number.is_empty()
                && number.chars().all(|c| c.is_ascii_digit())
            {
                return Some(self.format(id, &[("number", number)]));
            }
        }
        None
    }

    /// A name someone gave (a body, sketch, component, …): shown as typed, even when it matches
    /// a UI label ("Fillet", "Boss"); only the default names are translated.
    pub fn given_name(self, text: &str) -> String {
        self.default_name(text).unwrap_or_else(|| text.to_string())
    }

    /// Translate generated browser names and labels, preserving custom names otherwise.
    pub fn dynamic(self, text: &str) -> String {
        if let Some(name) = self.default_name(text) {
            return name;
        }
        for (prefix, id, arg) in [("Units: ", "browser.units", "units"), ("Precision: ", "browser.precision", "precision")] {
            if let Some(value) = text.strip_prefix(prefix) {
                return self.format(id, &[(arg, value)]);
            }
        }
        if let Some(count) = text.strip_prefix("Parameters (").and_then(|s| s.strip_suffix(')')) {
            return self.format("browser.parameters", &[("count", count)]);
        }
        if let Some(count) = text.strip_suffix(" degrees of freedom")
            && !count.is_empty()
            && count.chars().all(|c| c.is_ascii_digit())
        {
            return self.format("sketch.degrees_of_freedom", &[("count", count)]);
        }
        self.label(text).to_string()
    }
}

/// One-pass substitution: argument values are literal, even when they contain braces.
/// Missing arguments remain visible rather than crashing or silently removing text.
fn interpolate(template: &str, args: &[(&str, &str)]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some((before, after)) = remaining.split_once('{') {
        output.push_str(before);
        let Some((name, rest)) = after.split_once('}') else {
            output.push('{');
            output.push_str(after);
            return output;
        };
        if let Some((_, value)) = args.iter().find(|(key, _)| *key == name) {
            output.push_str(value);
        } else {
            output.push('{');
            output.push_str(name);
            output.push('}');
        }
        remaining = rest;
    }
    output.push_str(remaining);
    output
}

thread_local! {
    // Helpers without an app reference share a render scope. Nested renders restore
    // their caller's locale; no owned language strings or persistent global setting.
    static CURRENT: Cell<Locale> = const { Cell::new(Locale(&ENGLISH)) };
}

#[must_use]
pub struct LanguageScope {
    previous: Locale,
    // Drop on the thread where the scope was entered.
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

pub fn scope(id: &str) -> LanguageScope {
    let previous = CURRENT.with(|current| current.replace(Locale::for_language(id)));
    LanguageScope { previous, _thread: std::marker::PhantomData }
}

impl Drop for LanguageScope {
    fn drop(&mut self) {
        CURRENT.with(|current| current.set(self.previous));
    }
}

pub fn tr(id: &str) -> &str {
    CURRENT.with(|current| current.get().text(id))
}
pub fn tr_label(label: &str) -> &str {
    CURRENT.with(|current| current.get().label(label))
}
pub fn format(id: &str, args: &[(&str, &str)]) -> String {
    CURRENT.with(|current| current.get().format(id, args))
}
pub fn tr_dyn(text: &str) -> String {
    CURRENT.with(|current| current.get().dynamic(text))
}
pub fn tr_name(text: &str) -> String {
    CURRENT.with(|current| current.get().given_name(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The languages are listed English first, then by id, so adding one doesn't edit the
    /// line another addition edits.
    #[test]
    fn locales_are_listed_english_first_then_by_id() {
        let ids: Vec<&str> = LOCALES.iter().map(|c| c.id).collect();
        assert_eq!(ids.first(), Some(&"en"));
        assert!(ids[1..].is_sorted(), "{ids:?}");
    }
    use std::collections::BTreeSet;

    fn placeholders(text: &str) -> BTreeSet<&str> {
        let mut result = BTreeSet::new();
        let mut rest = text;
        while let Some((_, after)) = rest.split_once('{') {
            let (name, remaining) = after.split_once('}').expect("unclosed placeholder");
            assert!(!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            result.insert(name);
            rest = remaining;
        }
        result
    }

    /// Every English text `label()` can be asked to translate, read straight from the catalogs.
    fn english_texts() -> BTreeSet<&'static str> {
        ENGLISH.messages().values().map(String::as_str).collect()
    }

    /// Catalogs delivered complete. English is the source catalog; Japanese was completed
    /// upstream, Chinese and Czech land complete here. German, Korean, Russian, French and Ukrainian
    /// are partial (German and Korean predate the coverage guards; Russian, French and Ukrainian
    /// are kept partial so a new label doesn't need more translations), so their gaps fall back
    /// to English and are reported by `cargo xtask i18n` without failing the build, while
    /// nothing in the complete catalogs may regress.
    const COMPLETE: &[&str] = &["ja", "zh"];

    /// `text()` falls back to English silently, so a message a complete locale never received is
    /// shown in English inside an otherwise translated UI, with nothing in the build to say so.
    /// This is the guard against that: adding a key to `en.json` and forgetting a complete
    /// catalog fails here.
    #[test]
    fn every_complete_catalog_carries_every_english_message() {
        let english: Messages = serde_json::from_str(ENGLISH.source).unwrap();
        for catalog in LOCALES.iter().filter(|c| COMPLETE.contains(&c.id)) {
            let messages: Messages = serde_json::from_str(catalog.source).unwrap();
            let missing: Vec<&str> = english.keys().filter(|id| !messages.contains_key(*id)).map(String::as_str).collect();
            assert!(missing.is_empty(), "{} is missing {} English message(s): {}", catalog.id, missing.len(), missing.join(", "));
        }
    }

    /// A toolbar tab or panel reaches the UI as a bare source string (`toolbar::TABS`,
    /// `CommandSpec::tab` / `.panel`) and is resolved only by `label()`'s English-text lookup, so
    /// a heading missing from `en.json` can never translate in any locale. There are a dozen of
    /// them, so all of them are required rather than ratcheted.
    #[test]
    fn every_toolbar_tab_and_panel_is_in_the_english_catalog() {
        let texts = english_texts();
        let mut wanted: Vec<&str> = crate::toolbar::TABS.to_vec();
        wanted.push(crate::toolbar_custom::WORKSPACE);
        for spec in solidcraft_engine::command_specs() {
            wanted.push(spec.tab);
            wanted.push(spec.panel);
        }
        let mut missing: Vec<&str> = wanted.into_iter().filter(|t| !t.is_empty() && !texts.contains(t)).collect();
        missing.sort_unstable();
        missing.dedup();
        assert!(missing.is_empty(), "toolbar tabs/panels absent from en.json: {missing:?}");
    }

    /// Every command label the engine can show must exist as an English catalog message.
    ///
    /// `label()` resolves source text coming from the engine by looking that exact English string
    /// up in `en.json`, so a label the catalog does not carry stays English in *every* locale —
    /// nothing fails, the row just looks untranslated. The whole registry now has entries (their
    /// ids are `cmd.<command id>`), so this is a hard requirement rather than a budget: a new
    /// command added without a catalog entry fails here.
    #[test]
    fn every_command_label_is_in_the_english_catalog() {
        let texts = english_texts();
        let mut untranslated: Vec<&str> = solidcraft_engine::command_specs().iter().map(|c| c.label).filter(|label| !texts.contains(label)).collect();
        untranslated.sort_unstable();
        untranslated.dedup();
        assert!(
            untranslated.is_empty(),
            "{} command label(s) have no English catalog entry, so they would stay English in every locale: {untranslated:?}",
            untranslated.len()
        );
    }

    /// The start page shows each built-in sample's name and description through `label()`, so a
    /// sample added to the engine without English catalog entries would stay English everywhere.
    #[test]
    fn every_sample_name_and_description_is_in_the_english_catalog() {
        let texts = english_texts();
        let samples = solidcraft_engine::sample::samples();
        let missing: Vec<&str> = samples.iter().flat_map(|s| [s.name, s.about]).filter(|text| !texts.contains(text)).collect();
        assert!(missing.is_empty(), "sample texts absent from en.json: {missing:?}");
        assert_eq!(Locale::for_language("ja").label("Sample Plate"), "サンプル プレート");
    }

    /// The whole engine registry now reaches the catalogs. Sample command labels from areas that
    /// had no entry at all before (sheet metal, inspect, joints, motion) must resolve to their
    /// translation rather than echo the English source text — `label()` returns its argument when
    /// it finds no English message, so a dropped entry looks like a working English UI.
    #[test]
    fn newly_catalogued_command_labels_translate() {
        let cases = [
            ("cs", "Příruba", "Zebra analýza", "Studie pohybu"),
            ("zh", "法兰", "斑马纹分析", "运动研究"),
            ("ja", "フランジ", "ゼブラ解析", "モーション スタディ"),
            ("de", "Flansch", "Zebra-Analyse", "Bewegungsuntersuchung"),
            ("ko", "플랜지", "얼룩말줄무늬 분석", "운동 연구"),
            ("ru", "Фланец", "Зебра-анализ", "Исследование движения"),
            ("fr", "Bord tombé", "Analyse zébrée", "Étude de mouvement"),
            ("uk", "Фланець", "Зебра-аналіз", "Дослідження руху"),
        ];
        for (lang, flange, zebra, motion) in cases {
            let locale = Locale::for_language(lang);
            for (source, expected) in [("Flange", flange), ("Zebra Analysis", zebra), ("Motion Study", motion)] {
                assert_eq!(locale.label(source), expected, "{lang}: {source}");
            }
        }
    }

    #[test]
    fn catalogs_have_valid_ids_and_matching_placeholders() {
        let english: Messages = serde_json::from_str(ENGLISH.source).unwrap();
        let mut ids = BTreeSet::new();
        for catalog in LOCALES {
            assert!(ids.insert(catalog.id), "duplicate locale");
            let messages: Messages = serde_json::from_str(catalog.source).unwrap();
            assert!(!messages.is_empty());
            for (id, text) in messages {
                let original = english.get(&id).expect("unknown message ID");
                assert_eq!(placeholders(&text), placeholders(original), "{}: {id}", catalog.id);
            }
        }
    }

    #[test]
    fn direct_ui_message_ids_exist_in_english_catalog() {
        // Catch a misspelled key before it is shown verbatim in the UI. Engine-table labels
        // deliberately use the separate label adapter and fall back to their source text.
        for source in [
            include_str!("toolbar.rs"),
            include_str!("prefs.rs"),
            include_str!("documents.rs"),
            include_str!("dialogs.rs"),
            include_str!("browser.rs"),
            include_str!("sketch_palette.rs"),
            include_str!("design_properties.rs"),
            include_str!("home.rs"),
        ] {
            for call in [".tr(\"", "i18n::tr(\"", "i18n::format(\""] {
                for tail in source.split(call).skip(1) {
                    let id = tail.split('"').next().unwrap();
                    assert!(ENGLISH.messages().contains_key(id), "missing English message: {id}");
                }
            }
        }
    }

    #[test]
    fn keys_and_engine_labels_translate_with_english_fallback() {
        let ja = Locale::for_language("ja");
        assert_eq!(ja.text("ui.preferences"), "環境設定");
        assert_eq!(ja.label("Extrude"), "押し出し");
        assert_eq!(ja.label("CREATE SKETCH"), "スケッチを作成");
        assert_eq!(ja.label("New Component"), "新規コンポーネント");
        assert_eq!(ja.label("UTILITIES"), "ユーティリティ");
        assert_eq!(ja.label("CONFIGURE"), "構成");
        assert_eq!(ja.label("Drive Joints"), "ジョイントを駆動");
        assert_eq!(ja.label("Axis Through Cylinder/Cone/Torus"), "円柱/円錐/トーラスを通る軸");
        assert_eq!(ja.label("Filter commands or keys"), "コマンドまたはキーを検索");
        assert_eq!(ja.label("Context menu"), "コンテキスト メニュー");
        assert_eq!(ja.label("NonExistentKey"), "NonExistentKey");
        assert_eq!(Locale::for_language("unknown").text("ui.preferences"), "Preferences");
        static PARTIAL: Catalog = Catalog::new("test", "Test", r#"{"ui.preferences":"Prefs"}"#);
        assert_eq!(Locale(&PARTIAL).text("ui.preferences"), "Prefs");
        assert_eq!(Locale(&PARTIAL).text("ui.extrude"), "Extrude");
    }

    /// `label()`'s id fallback (an argument that matches no other entry's English text is
    /// looked up as an id directly) is how the Sketch Palette reaches `ui.palette_profile` and
    /// `ui.palette_points` without colliding with "Profile" / "Points" elsewhere (see the
    /// comment on `sketch_palette::TOGGLES`). Prove it resolves to each locale's own
    /// translation, not merely to something: a broken id would fall back to the English catalog
    /// silently (`Locale::text`'s own fallback chain), so a test that only checks "resolves to a
    /// non-empty string" would not catch that.
    #[test]
    fn id_labels_resolve_to_their_own_translation_not_an_english_fallback() {
        for (lang, profile, points) in [
            ("de", "Profil", "Punkte"),
            ("ja", "プロファイル", "点"),
            ("ko", "프로파일", "점"),
            ("ru", "Профиль", "Точки"),
            ("fr", "Profil", "Points"),
            ("uk", "Профіль", "Точки"),
        ] {
            let locale = Locale::for_language(lang);
            assert_eq!(locale.label("ui.palette_profile"), profile, "{lang}");
            assert_eq!(locale.label("ui.palette_points"), points, "{lang}");
        }
        // The literal English text "Profile" / "Points" must keep naming the pre-existing,
        // unrelated entries (the SelInput row labels in dialogs.rs, and "Show Profile" /
        // "Hide Profile" in context_menu.rs), not the palette's own ids: that separation is the
        // entire reason these two rows pass an id instead of their English text.
        assert_eq!(Locale::for_language("de").label("Profile"), Locale::for_language("de").text("ui.profile"));
        assert_eq!(Locale::for_language("de").label("Points"), Locale::for_language("de").text("ui.points"));
    }

    /// The Construction Geometries row passes its English text, like the other seven text
    /// rows. "Construction" on its own is already the line-type entry `ui.construction`, so
    /// the row's two-word text must reach its own entry rather than that one.
    #[test]
    fn the_construction_geometries_row_reaches_its_own_entry_not_the_line_type() {
        let rows = [
            ("de", "Konstruktionsgeometrien"),
            ("ja", "構築ジオメトリ"),
            ("ko", "구성 형상"),
            ("ru", "Вспомогательная геометрия"),
            ("fr", "Géométries de construction"),
            ("uk", "Конструкційна геометрія"),
        ];
        for (lang, want) in rows {
            let locale = Locale::for_language(lang);
            assert_eq!(locale.label("Construction Geometries"), want, "{lang}");
            assert_ne!(locale.label("Construction Geometries"), locale.text("ui.construction"), "{lang}");
        }
        assert_eq!(Locale::for_language("en").label("Construction Geometries"), "Construction Geometries");
    }

    #[test]
    fn templates_allow_reordering_and_literal_braces_in_values() {
        assert_eq!(interpolate("{second} then {first}", &[("first", "{second}"), ("second", "B")]), "B then {second}");
        assert_eq!(interpolate("{missing} / {", &[]), "{missing} / {");
        assert_eq!(Locale::for_language("ja").format("document.save_changes", &[("name", "My {design}")]), "My {design} への変更を保存しますか？");
        assert_eq!(Locale::for_language("en").format("dialog.selected_count", &[("count", "2")]), "↖ 2 selected");
    }

    #[test]
    fn browser_patterns_preserve_numbers_and_custom_names() {
        let ja = Locale::for_language("ja");
        assert_eq!(ja.dynamic("Sketch1"), "スケッチ1");
        assert_eq!(ja.dynamic("Body2"), "ボディ2");
        assert_eq!(ja.dynamic("Component12"), "コンポーネント12");
        assert_eq!(ja.dynamic("Units: mm"), "単位: mm");
        assert_eq!(ja.dynamic("Precision: 3"), "精度: 3");
        assert_eq!(ja.dynamic("Parameters (5)"), "パラメータ (5)");
        assert_eq!(ja.dynamic("4 degrees of freedom"), "自由度 4");
        assert_eq!(ja.dynamic("Sketch of bracket"), "Sketch of bracket");
        assert_eq!(Locale::for_language("en").dynamic("Sketch1"), "Sketch1");
    }

    #[test]
    fn given_names_are_not_translated_as_labels() {
        let de = Locale::for_language("de");
        // "Boss" and "Fillet" are also UI labels, which `dynamic` translates.
        assert_ne!(de.dynamic("Boss"), "Boss");
        assert_ne!(de.dynamic("Fillet"), "Fillet");
        assert_eq!(de.given_name("Boss"), "Boss");
        assert_eq!(de.given_name("Fillet"), "Fillet");
        assert_eq!(de.given_name("Sketch3"), de.dynamic("Sketch3"));
        assert_ne!(de.given_name("Sketch3"), "Sketch3");
    }

    #[test]
    fn spanish_catalog_is_registered_and_translates_cad_labels() {
        let es = Locale::for_language("es");
        assert_eq!(es.name(), "Español (España)");
        assert!(is_supported("es"));
        assert_eq!(es.text("ui.preferences"), "Preferencias");
        assert_eq!(es.label("Extrude"), "Extruir");
        assert_eq!(es.label("Fillet"), "Redondeo");
        assert_eq!(es.dynamic("Sketch1"), "Boceto1");
        assert_eq!(es.format("document.save_changes", &[("name", "Pieza")]), "¿Guardar los cambios de Pieza?");
    }

    #[test]
    fn korean_catalog_translates_keys_labels_and_browser_patterns() {
        let ko = Locale::for_language("ko");
        assert_eq!(ko.name(), "한국어");
        assert!(is_supported("ko"));
        assert_eq!(ko.text("ui.preferences"), "환경 설정");
        assert_eq!(ko.label("Extrude"), "돌출");
        assert_eq!(ko.label("CREATE SKETCH"), "스케치 만들기");
        assert_eq!(ko.label("NonExistentKey"), "NonExistentKey");
        assert_eq!(ko.format("document.save_changes", &[("name", "My {design}")]), "My {design}의 변경 사항을 저장하시겠습니까?");
        assert_eq!(ko.format("dialog.selected_count", &[("count", "2")]), "↖ 2개 선택됨");
        assert_eq!(ko.dynamic("Sketch1"), "스케치1");
        assert_eq!(ko.dynamic("Body2"), "바디2");
        assert_eq!(ko.dynamic("Component12"), "컴포넌트12");
        assert_eq!(ko.dynamic("Units: mm"), "단위: mm");
        assert_eq!(ko.dynamic("Precision: 3"), "정밀도: 3");
        assert_eq!(ko.dynamic("Parameters (5)"), "매개변수 (5)");
        assert_eq!(ko.dynamic("4 degrees of freedom"), "자유도 4개");
        assert_eq!(ko.dynamic("Sketch of bracket"), "Sketch of bracket");
    }

    #[test]
    fn russian_catalog_translates_keys_labels_and_browser_patterns() {
        let ru = Locale::for_language("ru");
        assert_eq!(ru.name(), "Русский");
        assert!(is_supported("ru"));
        assert_eq!(ru.text("ui.preferences"), "Настройки");
        assert_eq!(ru.label("Extrude"), "Выдавливание");
        assert_eq!(ru.label("CREATE SKETCH"), "СОЗДАТЬ ЭСКИЗ");
        assert_eq!(ru.label("NonExistentKey"), "NonExistentKey");
        assert_eq!(ru.format("document.save_changes", &[("name", "My {design}")]), "Сохранить изменения в My {design}?");
        assert_eq!(ru.format("dialog.selected_count", &[("count", "2")]), "↖ Выбрано: 2");
        assert_eq!(ru.dynamic("Sketch1"), "Эскиз1");
        assert_eq!(ru.dynamic("Body2"), "Тело2");
        assert_eq!(ru.dynamic("Component12"), "Компонент12");
        assert_eq!(ru.dynamic("Units: mm"), "Единицы: mm");
        assert_eq!(ru.dynamic("Precision: 3"), "Точность: 3");
        assert_eq!(ru.dynamic("Parameters (5)"), "Параметры (5)");
        assert_eq!(ru.dynamic("4 degrees of freedom"), "Степеней свободы: 4");
        assert_eq!(ru.dynamic("Sketch of bracket"), "Sketch of bracket");
    }

    #[test]
    fn french_catalog_translates_keys_labels_and_browser_patterns() {
        let fr = Locale::for_language("fr");
        assert_eq!(fr.name(), "Français");
        assert!(is_supported("fr"));
        assert_eq!(fr.text("ui.preferences"), "Préférences");
        assert_eq!(fr.label("Extrude"), "Extrusion");
        assert_eq!(fr.label("Fillet"), "Congé");
        assert_eq!(fr.label("CREATE SKETCH"), "CRÉER UNE ESQUISSE");
        assert_eq!(fr.label("NonExistentKey"), "NonExistentKey");
        // French puts a no-break space before the two-part punctuation marks.
        assert_eq!(fr.format("document.save_changes", &[("name", "My {design}")]), "Enregistrer les modifications de My {design}\u{a0}?");
        assert_eq!(fr.format("dialog.selected_count", &[("count", "2")]), "↖ 2 sélectionné(s)");
        assert_eq!(fr.dynamic("Sketch1"), "Esquisse1");
        assert_eq!(fr.dynamic("Body2"), "Corps2");
        assert_eq!(fr.dynamic("Component12"), "Composant12");
        assert_eq!(fr.dynamic("Units: mm"), "Unités\u{a0}: mm");
        assert_eq!(fr.dynamic("Precision: 3"), "Précision\u{a0}: 3");
        assert_eq!(fr.dynamic("Parameters (5)"), "Paramètres (5)");
        assert_eq!(fr.dynamic("4 degrees of freedom"), "4 degrés de liberté");
        assert_eq!(fr.dynamic("Sketch of bracket"), "Sketch of bracket");
    }

    #[test]
    fn turkish_catalog_is_registered_and_translates_keys() {
        let tr = Locale::for_language("tr");
        assert_eq!(tr.name(), "Türkçe");
        assert!(is_supported("tr"));
        assert_eq!(tr.text("ui.preferences"), "Ayarlar");
        assert_eq!(tr.format("document.save_changes", &[("name", "My {design}")]), "My {design} içindeki değişiklikler kaydedilsin mi?");
        assert_eq!(tr.format("dialog.selected_count", &[("count", "2")]), "↖ 2 seçildi");
        assert_eq!(tr.label("NonExistentKey"), "NonExistentKey");
    }

    #[test]
    fn brazilian_portuguese_catalog_is_registered_and_translates_keys() {
        let pt = Locale::for_language("pt-BR");
        assert_eq!(pt.name(), "Português (Brasil)");
        assert!(is_supported("pt-BR"));
        assert_eq!(pt.text("ui.preferences"), "Preferências");
        assert_eq!(pt.format("document.save_changes", &[("name", "My {design}")]), "Salvar as alterações em My {design}?");
        assert_eq!(pt.format("dialog.selected_count", &[("count", "2")]), "↖ 2 selecionados");
        assert_eq!(pt.label("NonExistentKey"), "NonExistentKey");
    }

    #[test]
    fn galician_catalog_is_registered_and_translates_keys() {
        let gl = Locale::for_language("gl");
        assert_eq!(gl.name(), "Galego");
        assert!(is_supported("gl"));
        assert_eq!(gl.text("ui.preferences"), "Preferencias");
        assert_eq!(gl.format("document.save_changes", &[("name", "My {design}")]), "Gardar os cambios en My {design}?");
        assert_eq!(gl.format("dialog.selected_count", &[("count", "2")]), "↖ 2 seleccionados");
        assert_eq!(gl.label("Fillet"), "Arredondamento");
        assert_eq!(gl.label("NonExistentKey"), "NonExistentKey");
    }

    #[test]
    fn ukrainian_catalog_translates_keys_labels_and_browser_patterns() {
        let uk = Locale::for_language("uk");
        assert_eq!(uk.name(), "Українська");
        assert!(is_supported("uk"));
        assert_eq!(uk.text("ui.preferences"), "Налаштування");
        assert_eq!(uk.label("Extrude"), "Видавлювання");
        assert_eq!(uk.label("CREATE SKETCH"), "СТВОРИТИ ЕСКІЗ");
        assert_eq!(uk.label("NonExistentKey"), "NonExistentKey");
        assert_eq!(uk.format("document.save_changes", &[("name", "My {design}")]), "Зберегти зміни в My {design}?");
        assert_eq!(uk.format("dialog.selected_count", &[("count", "2")]), "↖ Вибрано: 2");
        assert_eq!(uk.dynamic("Sketch1"), "Ескіз1");
        assert_eq!(uk.dynamic("Body2"), "Тіло2");
        assert_eq!(uk.dynamic("Component12"), "Компонент12");
        assert_eq!(uk.dynamic("Units: mm"), "Одиниці: mm");
        assert_eq!(uk.dynamic("Precision: 3"), "Точність: 3");
        assert_eq!(uk.dynamic("Parameters (5)"), "Параметри (5)");
        assert_eq!(uk.dynamic("4 degrees of freedom"), "4 ступенів вільності");
        assert_eq!(uk.dynamic("Sketch of bracket"), "Sketch of bracket");
    }

    #[test]
    fn render_scopes_restore_the_previous_language() {
        let _english = scope("en");
        {
            let _japanese = scope("ja");
            assert_eq!(tr("ui.preferences"), "環境設定");
            {
                let _nested = scope("en");
                assert_eq!(tr("ui.preferences"), "Preferences");
            }
            assert_eq!(tr("ui.preferences"), "環境設定");
        }
        assert_eq!(tr("ui.preferences"), "Preferences");
    }
}
