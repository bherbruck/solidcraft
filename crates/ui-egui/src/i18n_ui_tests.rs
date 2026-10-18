//! Localization must not change scenario handles or leave the next app in another locale.

use crate::scenario::{self, Harness};
use serde_json::json;

#[test]
fn japanese_dialog_keeps_english_scenario_handles() {
    let mut h = Harness::new();
    h.app.preferences.language = "ja".into();
    h.app.home.open = false;
    h.call("ui.start", json!({"command":"sketch.create"}));
    h.frames(3);
    assert!(h.widget("平面").is_some(), "the input label must be translated");
    assert!(h.widget("キャンセル").is_some(), "the close button must be translated");
    assert!(!scenario::handles("dialog:Plane").is_empty(), "scenario handles must retain source labels");
    assert_eq!(crate::i18n::tr("ui.preferences"), "Preferences", "the render scope must restore English");

    h.app.preferences.language = "en".into();
    h.frames(3);
    assert!(h.widget("Plane").is_some());
    assert!(h.widget("Cancel").is_some());
}

#[test]
fn italian_dialog_keeps_english_scenario_handles() {
    let mut h = Harness::new();
    h.app.preferences.language = "it".into();
    h.app.home.open = false;
    h.call("ui.start", json!({"command":"sketch.create"}));
    h.frames(3);
    assert!(h.widget("Piano").is_some());
    assert!(h.widget("Annulla").is_some());
    assert!(!scenario::handles("dialog:Plane").is_empty());
    assert_eq!(crate::i18n::tr("ui.preferences"), "Preferences");

    h.app.preferences.language = "en".into();
    h.frames(3);
    assert!(h.widget("Plane").is_some());
    assert!(h.widget("Cancel").is_some());
}

#[test]
fn sketch_palette_profile_and_points_rows_show_their_own_translation() {
    // ui.palette_profile / ui.palette_points are reached by id, not by the usual English-text
    // lookup (see the comment on sketch_palette::TOGGLES, and
    // i18n::tests::id_labels_resolve_to_their_own_translation_not_an_english_fallback for the
    // lookup itself). A broken id falls back to English silently rather than failing loudly, so
    // check the word actually on screen in each locale, not just that something renders.
    for (lang, profile, points) in [("de", "Profil", "Punkte"), ("ja", "プロファイル", "点"), ("ko", "프로파일", "점"), ("uk", "Профіль", "Точки")]
    {
        let mut h = Harness::new();
        h.app.preferences.language = lang.into();
        h.app.home.open = false;
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.frames(3);
        assert!(h.widget(profile).is_some(), "{lang}: the Profile row must show {profile:?}");
        assert!(h.widget(points).is_some(), "{lang}: the Points row must show {points:?}");
        assert!(h.widget("Profile").is_none(), "{lang}: must not silently fall back to English");
        assert!(h.widget("Points").is_none(), "{lang}: must not silently fall back to English");
    }
}

#[test]
fn language_preference_cancel_restores_previous_locale() {
    let mut h = Harness::new();
    h.app.home.open = false;
    h.call("ui.prefs", json!({"open":"General"}));
    h.call("ui.prefs", json!({"language":"ja"}));
    assert!(h.widget("言語").is_some());
    assert_eq!(h.app.preferences.language, "ja");
    h.call("ui.prefs", json!({"button":"cancel"}));
    assert_eq!(h.app.preferences.language, "en");
    assert_eq!(h.app.tr("ui.preferences"), "Preferences");
}

/// The chrome a user sees before touching anything: the browser heading and the SELECT panel.
/// Both were English literals in the drawing code, and a broken message id falls back to
/// English silently, so check the word actually rendered in each language.
#[test]
fn window_chrome_renders_in_the_active_language() {
    for (lang, browser, filter, faces) in [
        ("zh", "浏览器", "选择筛选", "面"),
        ("ja", "ブラウザ", "選択フィルター", "面"),
        ("de", "BROWSER", "Auswahlfilter", "Flächen"),
        ("ko", "브라우저", "선택 필터", "면"),
        ("ru", "БРАУЗЕР", "Фильтр выделения", "Грани"),
        ("fr", "NAVIGATEUR", "Filtre de sélection", "Faces"),
        ("it", "STRUTTURA", "Filtro di selezione", "Facce"),
        ("uk", "БРАУЗЕР", "Фільтр виділення", "Грані"),
    ] {
        let mut h = Harness::new();
        h.app.preferences.language = lang.into();
        h.app.home.open = false;
        h.frames(2);
        // The browser heading is on screen from the start; the selection filter lives in the
        // SELECT panel, which is not the one shown by default.
        assert!(h.widget(browser).is_some(), "{lang}: the browser heading must show {browser:?}");
        let select = scenario::handle_at("panel:SOLID:SELECT").expect("the SELECT panel is on the SOLID tab");
        h.step(&json!({"click": [select.x, select.y]})).unwrap();
        h.frames(2);
        assert!(h.widget(filter).is_some(), "{lang}: the selection filter must show {filter:?}");
        assert!(h.widget(faces).is_some(), "{lang}: the Faces checkbox must show {faces:?}");
    }
}

/// The start page's two big buttons had a fixed width, so a translation longer than the English
/// label ran past the edge ("Nouvelle conception"). Every registered language's label must stay
/// inside its button.
#[test]
fn home_page_buttons_fit_their_translated_labels() {
    fn collect(shape: &egui::Shape, rects: &mut Vec<egui::Rect>, texts: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Rect(r) => rects.push(r.rect),
            egui::Shape::Text(t) => texts.push((t.galley.job.text.clone(), egui::Rect::from_min_size(t.pos, t.galley.size()))),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, rects, texts)),
            _ => {}
        }
    }
    for catalog in crate::i18n::LOCALES {
        let mut h = Harness::new();
        h.app.preferences.language = catalog.id.into();
        let rev = h.app.session.revision;
        h.app.home.show(rev);
        h.frames(2);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
        let mut out = h.ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| h.app.ui(ui));
        out.textures_delta.clear();
        let (mut rects, mut texts) = (Vec::new(), Vec::new());
        out.shapes.iter().for_each(|shape| collect(&shape.shape, &mut rects, &mut texts));
        let locale = crate::i18n::Locale::for_language(catalog.id);
        for id in ["ui.new_design", "home.open"] {
            let label = locale.text(id);
            let (_, text) = texts.iter().find(|(t, _)| t == label).unwrap_or_else(|| panic!("{}: {label:?} is not painted", catalog.id));
            let button = rects
                .iter()
                .filter(|r| (r.height() - 64.0).abs() < 1.0 && r.contains(text.left_center()))
                .min_by(|a, b| a.area().total_cmp(&b.area()))
                .unwrap_or_else(|| panic!("{}: no button around {label:?}", catalog.id));
            assert!(text.right() <= button.right(), "{}: {label:?} ends at {} past its button's edge {}", catalog.id, text.right(), button.right());
        }
    }
}

/// The start page is the first thing a user sees. Its buttons, links, headings and cards were
/// English literals, mostly painted rather than laid out as widgets, so check the text actually
/// painted in each complete language: a broken message id falls back to English silently.
#[test]
fn home_page_renders_in_the_active_language() {
    fn painted(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| painted(s, out)),
            _ => {}
        }
    }
    let cases = [
        (
            "it",
            [
                "Nuovo progetto",
                "Apri…",
                "Documentazione",
                "Segnala un problema",
                "Novità",
                "Scorciatoie da tastiera",
                "Preferenze",
                "Recenti",
                "Esempi",
                "Piastra di esempio",
            ],
            "non presente\nAperto 2 h fa",
        ),
        (
            "ja",
            [
                "新規デザイン",
                "開く…",
                "ドキュメント",
                "問題を報告",
                "新機能",
                "キーボードショートカット",
                "環境設定",
                "最近使用した項目",
                "サンプル",
                "サンプル プレート",
            ],
            "見つかりません\n2 時間前に開きました",
        ),
        ("zh", ["新建设计", "打开…", "文档", "报告问题", "新功能", "键盘快捷键", "首选项", "最近使用", "示例", "示例板"], "文件不存在\n2 小时前打开"),
    ];
    for (lang, words, missing_recent) in cases {
        let mut h = Harness::new();
        h.app.preferences.language = lang.into();
        let gone = h.dir.join("gone.solidcraft").to_string_lossy().into_owned();
        h.app.home.add_recent(&gone);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        h.app.home.opened_at.insert(gone, now - 2 * 3600);
        let rev = h.app.session.revision;
        h.app.home.show(rev);
        h.frames(2);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
        let mut out = h.ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| h.app.ui(ui));
        out.textures_delta.clear();
        let mut texts = Vec::new();
        out.shapes.iter().for_each(|shape| painted(&shape.shape, &mut texts));
        for word in words.into_iter().chain([missing_recent]) {
            assert!(texts.iter().any(|t| t == word), "{lang}: {word:?} must be painted, got {texts:?}");
        }
        for english in ["New Design", "Open…", "Documentation", "Report an Issue", "What's new", "Recent", "Samples", "Sample Plate"] {
            assert!(!texts.iter().any(|t| t == english), "{lang}: {english:?} is still painted in English");
        }
    }
}

/// The command palette's hint embeds a JSON example, so its braces must not be mistaken for a
/// placeholder name by the catalog's parser, and the example must survive interpolation whole.
#[test]
fn command_palette_hint_keeps_its_json_example_intact() {
    let mut h = Harness::new();
    h.app.preferences.language = "zh".into();
    h.app.home.open = false;
    h.frames(2);
    let hint = crate::i18n::format("ui.command_palette_hint", &[("example", "Extrude {\"distance\": 20}")]);
    assert!(hint.contains("Extrude {\"distance\": 20}"), "the example is inserted literally: {hint:?}");
    assert!(!hint.contains("{example}"), "the placeholder is substituted: {hint:?}");
}

#[test]
fn translated_dialog_buttons_do_not_wrap() {
    fn cancel_rows(shape: &egui::Shape) -> Option<usize> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == "キャンセル" => Some(text.galley.rows.len()),
            egui::Shape::Vec(shapes) => shapes.iter().find_map(cancel_rows),
            _ => None,
        }
    }
    let mut h = Harness::new();
    crate::fonts::setup_fonts(&h.ctx);
    h.app.preferences.language = "ja".into();
    h.app.home.open = false;
    h.call("ui.start", json!({"command":"solid.extrude"}));
    let mut out = h.ctx.run_ui(
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() },
        |ui| h.app.ui(ui),
    );
    assert_eq!(out.shapes.iter().find_map(|shape| cancel_rows(&shape.shape)), Some(1));
    out.textures_delta.clear();
}

#[test]
fn long_panel_label_wraps_centred_and_its_button_is_centred() {
    fn label_rows(shape: &egui::Shape) -> Option<Vec<egui::Rect>> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == "SKIZZE BEENDEN" => {
                Some(text.galley.rows.iter().map(|row| row.rect().translate(text.pos.to_vec2())).collect())
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(label_rows),
            _ => None,
        }
    }
    fn hover_rect(shape: &egui::Shape, hover: egui::Color32, at: egui::Pos2) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Rect(r) if r.fill == hover && r.rect.contains(at) => Some(r.rect),
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| hover_rect(s, hover, at)),
            _ => None,
        }
    }
    let mut h = Harness::new();
    h.app.preferences.language = "de".into();
    h.app.home.open = false;
    h.app.run("sketch.create", json!({"plane":"xy"})).unwrap();
    h.frames(2);
    let label = scenario::handles("panel:SKETCH:FINISH SKETCH")[0].1;
    let button = scenario::handles("button:sketch.finish")[0].1;
    assert!((label.x - button.x).abs() < 1.0, "the only button sits centred over its label: {label:?} {button:?}");

    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
    let mut out =
        h.ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events: vec![egui::Event::PointerMoved(label)], ..Default::default() }, |ui| {
            h.app.ui(ui)
        });
    out.textures_delta.clear();
    let mut out = h.ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| h.app.ui(ui));
    let rows = out.shapes.iter().find_map(|shape| label_rows(&shape.shape)).expect("the Finish Sketch panel label is drawn");
    assert_eq!(rows.len(), 2, "the label wider than its one-button panel wraps: {rows:?}");
    assert!(rows[0].width() != rows[1].width(), "the lines differ in width: {rows:?}");
    assert!((rows[0].center().x - rows[1].center().x).abs() < 1.0, "each line is centred: {rows:?}");
    let hover = crate::theme::Tokens::get().hover;
    let lr = out.shapes.iter().find_map(|shape| hover_rect(&shape.shape, hover, label)).expect("the hovered label is highlighted");
    assert!(rows.iter().all(|row| lr.contains_rect(*row)), "the highlight holds both lines: {lr:?} {rows:?}");
    out.textures_delta.clear();
}
