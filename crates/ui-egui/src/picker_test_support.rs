//! Controlled host futures shared by Render and library picker regressions.
use crate::{SolveApp, file_picker::FilePickerFuture};
use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};
pub type Slot = Rc<RefCell<Option<Option<String>>>>;
struct Manual(Slot);
impl Future for Manual {
    type Output = Option<String>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.borrow_mut().take().map_or(Poll::Pending, Poll::Ready)
    }
}
pub fn save(app: &mut SolveApp, slot: &Slot) {
    let slot = slot.clone();
    app.services.pick_save = Some(Box::new(move |_, _| Box::pin(Manual(slot.clone())) as FilePickerFuture));
}
pub fn open(app: &mut SolveApp, slot: &Slot) {
    let slot = slot.clone();
    app.services.pick_open = Some(Box::new(move |_| Box::pin(Manual(slot.clone())) as FilePickerFuture));
}
