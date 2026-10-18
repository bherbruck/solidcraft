//! Native pickers must return to the event loop while the user chooses a file (#1088).
//! The host opens an async sheet; its continuation runs on the UI thread once it answers.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use serde_json::{Value, json};

use crate::SolveApp;

/// A host picker answer: a selected path, or `None` when cancelled.
pub type FilePickerFuture = Pin<Box<dyn Future<Output = Option<String>>>>;
type Completion = Box<dyn FnOnce(&mut SolveApp, Option<String>)>;

pub(crate) struct Pending {
    future: FilePickerFuture,
    complete: Completion,
}

struct Repaint(egui::Context);

impl Wake for Repaint {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint();
    }
}

impl SolveApp {
    /// Keep the target design and editors unchanged until the picker answers.
    pub fn file_picker_busy(&self) -> bool {
        self.file_picker.is_some()
    }

    pub(crate) fn pick_open(&mut self, complete: impl FnOnce(&mut SolveApp, Option<String>) + 'static) {
        self.pick_open_for(&[], complete);
    }

    /// Open a file of this type: the picker offers these extensions first (#1801).
    pub(crate) fn pick_open_for(&mut self, extensions: &[&str], complete: impl FnOnce(&mut SolveApp, Option<String>) + 'static) {
        if self.file_picker_busy() {
            return;
        }
        let future = self.services.pick_open.as_ref().map(|pick| pick(extensions));
        self.begin_picker(future, Box::new(complete));
    }

    /// Ask for one file of a kind (`label`, `extensions`), with the host's dialog for it, or the
    /// Open dialog where the host has none. (The agent chat asks for its files; the web build has
    /// no chat.)
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn pick_file(&mut self, label: &str, extensions: &[&str], complete: impl FnOnce(&mut SolveApp, Option<String>) + 'static) {
        if self.file_picker_busy() {
            return;
        }
        let future = match self.services.pick_file.as_ref() {
            Some(pick) => Some(pick(label, extensions)),
            None => self.services.pick_open.as_ref().map(|pick| pick(extensions)),
        };
        self.begin_picker(future, Box::new(complete));
    }

    pub(crate) fn pick_save(&mut self, name: &str, extensions: &[&str], complete: impl FnOnce(&mut SolveApp, Option<String>) + 'static) {
        if self.file_picker_busy() {
            return;
        }
        let future = self.services.pick_save.as_ref().map(|pick| pick(name, extensions));
        self.begin_picker(future, Box::new(complete));
    }

    /// Ask for a program (a slicer). Without a host dialog, the answer is a cancel.
    pub(crate) fn pick_program(&mut self, complete: impl FnOnce(&mut SolveApp, Option<String>) + 'static) {
        if self.file_picker_busy() {
            return;
        }
        let future = self.services.pick_program.as_ref().map(|pick| pick());
        self.begin_picker(future, Box::new(complete));
    }

    fn begin_picker(&mut self, future: Option<FilePickerFuture>, complete: Completion) {
        let Some(future) = future else {
            complete(self, None);
            return;
        };
        self.file_picker = Some(Pending { future, complete });
    }

    pub(crate) fn pick_open_command(&mut self, command: &str, params: Value, extensions: &[&str]) {
        let command = command.to_string();
        self.pick_open_for(extensions, move |app, path| {
            if let Some(path) = path {
                let mut params = params;
                params["path"] = json!(path);
                let result = app.run(&command, params);
                if command == "parameters.import"
                    && let Some(dialog) = app.params.as_mut()
                {
                    dialog.error = result.err();
                }
            }
        });
    }

    pub(crate) fn pick_save_command(&mut self, name: &str, extensions: &[&str], command: &str, params: Value) {
        let command = command.to_string();
        self.pick_save(name, extensions, move |app, path| {
            if let Some(path) = path {
                let mut params = params;
                params["path"] = json!(path);
                let result = app.run(&command, params);
                if command == "parameters.export"
                    && let Some(dialog) = app.params.as_mut()
                {
                    dialog.error = result.err();
                }
            }
        });
    }
}

pub(crate) fn tick(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(pending) = app.file_picker.as_mut() else { return };
    let waker = Waker::from(Arc::new(Repaint(ctx.clone())));
    let Poll::Ready(path) = pending.future.as_mut().poll(&mut Context::from_waker(&waker)) else {
        // A fallback for native backends that do not wake reliably. Resizing still draws
        // the ordinary UI in every frame.
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        return;
    };
    if let Some(pending) = app.file_picker.take() {
        (pending.complete)(app, path);
        // Hosts can flush writes or restore native input on the following frame.
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[derive(Default)]
    struct Answer {
        path: RefCell<Option<Option<String>>>,
        polls: Cell<usize>,
        waker: RefCell<Option<Waker>>,
    }

    struct Delayed(Rc<Answer>);
    impl Future for Delayed {
        type Output = Option<String>;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.0.polls.set(self.0.polls.get() + 1);
            *self.0.waker.borrow_mut() = Some(cx.waker().clone());
            self.0.path.borrow_mut().take().map_or(Poll::Pending, Poll::Ready)
        }
    }

    fn picker(app: &mut SolveApp, save: bool) -> Rc<Answer> {
        let answer = Rc::new(Answer::default());
        let captured = answer.clone();
        if save {
            app.services.pick_save = Some(Box::new(move |_, _| Box::pin(Delayed(captured.clone()))));
        } else {
            app.services.pick_open = Some(Box::new(move |_| Box::pin(Delayed(captured.clone()))));
        }
        answer
    }

    fn finish(app: &mut SolveApp, answer: &Answer, path: Option<String>) {
        *answer.path.borrow_mut() = Some(path);
        if let Some(waker) = answer.waker.borrow_mut().take() {
            waker.wake();
        }
        tick(app, &egui::Context::default());
        assert!(!app.file_picker_busy());
    }

    /// #1088: Open and Save stay pending through real UI frames at changing window sizes.
    /// Model input and tab changes wait, while the event loop keeps painting.
    #[test]
    fn pending_open_and_save_allow_resize_frames_without_changing_the_design() {
        for save in [false, true] {
            let mut h = crate::scenario::Harness::new();
            h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
            let before = h.app.session.doc.to_json();
            let answer = picker(&mut h.app, save);
            let completed = Rc::new(Cell::new(0));
            let count = completed.clone();
            let callback = move |_: &mut SolveApp, path: Option<String>| {
                assert!(path.is_none());
                count.set(count.get() + 1);
            };
            if save {
                h.app.pick_save("part.solidcraft", &["solidcraft"], callback);
            } else {
                h.app.pick_open(callback);
            }
            assert_eq!(answer.polls.get(), 0, "the UI action returns before polling");
            for (width, height) in [(1200, 800), (1600, 1000), (960, 600), (1400, 900)] {
                h.call("ui.resize", json!({"width":width,"height":height}));
                h.frames(2);
                assert!(h.app.file_picker_busy());
                assert_eq!(completed.get(), 0);
                assert_eq!(h.ctx.content_rect().width(), width as f32);
            }
            assert!(answer.polls.get() >= 8);
            crate::documents::new_design(&mut h.app);
            crate::documents::close(&mut h.app, 0, true);
            crate::documents::request_quit(&mut h.app);
            h.app.start("solid.box");
            assert!(h.app.run("solid.box", json!({})).is_err());
            h.step(&json!({"key":"N", "cmd":true})).unwrap();
            assert_eq!(h.app.docs.count(), 1);
            assert!(!h.app.quit_requested);
            assert_eq!(h.app.session.doc.to_json(), before);
            // Duplicate requests cannot replace the pending operation.
            h.app.pick_open(|_, _| panic!("duplicate picker"));
            finish(&mut h.app, &answer, None);
            tick(&mut h.app, &h.ctx);
            assert_eq!(completed.get(), 1);
            assert_eq!(h.app.session.doc.to_json(), before);
        }
    }

    #[test]
    fn save_as_waits_for_its_path_and_saves_only_on_completion() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
        let answer = picker(&mut h.app, true);
        let path = h.dir.join("chosen.solidcraft");
        crate::toolbar::save(&mut h.app);
        h.frames(3);
        assert!(h.app.session.is_dirty() && !path.exists());
        finish(&mut h.app, &answer, Some(path.to_string_lossy().into_owned()));
        assert!(path.is_file() && !h.app.session.is_dirty());
        assert_eq!(h.app.session.path.as_deref(), path.to_str());
    }

    #[test]
    fn close_and_quit_wait_for_save_as_and_cancel_or_failure_keeps_changes() {
        for quit in [false, true] {
            for outcome in ["save", "cancel", "fail"] {
                let mut h = crate::scenario::Harness::new();
                h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
                let before = h.app.session.doc.to_json();
                let answer = picker(&mut h.app, true);
                if quit {
                    crate::documents::request_quit(&mut h.app);
                } else {
                    crate::documents::close(&mut h.app, 0, false);
                }
                crate::documents::answer_prompt(&mut h.app, crate::documents::Answer::Save);
                h.frames(3);
                assert!(h.app.file_picker_busy() && !h.app.quit_requested);
                assert_eq!(h.app.docs.closing, Some(0));
                let path = match outcome {
                    "save" => Some(h.dir.join("chosen.solidcraft")),
                    "fail" => Some(h.dir.join("missing/chosen.solidcraft")),
                    _ => None,
                };
                finish(&mut h.app, &answer, path.map(|p| p.to_string_lossy().into_owned()));
                if outcome == "save" {
                    assert_eq!(h.app.quit_requested, quit);
                    assert!(!h.app.session.is_dirty());
                    if !quit {
                        assert!(h.app.home.open && h.app.session.doc.features.is_empty());
                    }
                } else {
                    assert!(!h.app.quit_requested && h.app.session.is_dirty());
                    assert!(h.app.docs.quitting.is_none() && h.app.docs.closing.is_none());
                    assert_eq!(h.app.session.doc.to_json(), before);
                }
            }
        }
    }

    #[test]
    fn quit_saves_each_untitled_tab_after_its_own_dialog_answers() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
        crate::documents::new_design(&mut h.app);
        h.app.run("solid.box", json!({"length":20,"width":20,"height":20})).unwrap();
        crate::documents::request_quit(&mut h.app);
        for i in 0..2 {
            let answer = picker(&mut h.app, true);
            crate::documents::answer_prompt(&mut h.app, crate::documents::Answer::Save);
            h.frames(2);
            assert!(!h.app.quit_requested);
            finish(&mut h.app, &answer, Some(h.dir.join(format!("tab{i}.solidcraft")).to_string_lossy().into_owned()));
            assert_eq!(h.app.quit_requested, i == 1);
        }
        assert!(!crate::documents::any_dirty(&h.app));
    }

    /// #1801: Insert SVG and Insert DXF ask the host for their own file type first; other
    /// opens keep the default list.
    #[test]
    fn sketch_inserts_ask_for_their_file_type() {
        let mut h = crate::scenario::Harness::new();
        let asked: Rc<RefCell<Vec<Vec<String>>>> = Rc::default();
        let log = asked.clone();
        h.app.services.pick_open = Some(Box::new(move |extensions| {
            log.borrow_mut().push(extensions.iter().map(|e| e.to_string()).collect());
            Box::pin(std::future::ready(None))
        }));
        h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
        for (command, want) in [("sketch.insert_svg", vec!["svg"]), ("sketch.insert_dxf", vec!["dxf"]), ("file.insert_step", vec![])] {
            h.app.start(command);
            tick(&mut h.app, &h.ctx);
            assert_eq!(asked.borrow().last().cloned(), Some(want.iter().map(|e| e.to_string()).collect::<Vec<_>>()), "{command}");
        }
    }

    #[test]
    fn picker_holds_engine_requests_but_answers_window_resize_requests() {
        let mut h = crate::scenario::Harness::new();
        let answer = picker(&mut h.app, false);
        h.app.pick_open(|_, _| {});
        let (tx, rx) = std::sync::mpsc::channel();
        h.app.control_rx = Some(rx);
        let (command, reply) =
            crate::control::ControlRequest::new("engine.execute", json!({"command":"solid.box","params":{"length":10,"width":10,"height":10}}));
        let (resize, resized) = crate::control::ControlRequest::new("ui.resize", json!({"width":1200,"height":800}));
        let (home, shown) = crate::control::ControlRequest::new("ui.home", json!({"open":true}));
        tx.send(command).unwrap();
        tx.send(resize).unwrap();
        tx.send(home).unwrap();
        h.frames(2);
        assert!(reply.try_recv().is_err());
        assert_eq!(resized.try_recv().unwrap()["ok"], true);
        assert!(shown.try_recv().is_err());
        assert_eq!(h.call("ui.inspect", json!({}))["result"]["file_picker_busy"], true);
        assert!(h.app.session.doc.features.is_empty());
        finish(&mut h.app, &answer, None);
        h.frames(2);
        let result = reply.try_recv().unwrap();
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(shown.try_recv().unwrap()["ok"], true);
        assert_eq!(h.app.session.doc.features.len(), 1);
    }

    #[test]
    fn canvas_picker_starts_placement_only_after_a_file_is_selected() {
        let mut h = crate::scenario::Harness::new();
        let answer = picker(&mut h.app, false);
        h.app.start("canvas.insert");
        h.frames(2);
        assert!(h.app.tool.is_none());
        finish(&mut h.app, &answer, Some("selected-image.png".into()));
        assert_eq!(h.app.tool.as_ref().map(|tool| tool.cmd), Some("canvas.insert"));
        let answer = picker(&mut h.app, false);
        h.app.start("canvas.insert");
        finish(&mut h.app, &answer, None);
        assert!(h.app.tool.is_none(), "cancelling does not resume the old placement");
    }
}
