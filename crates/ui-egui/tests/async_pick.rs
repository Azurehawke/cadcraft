//! Hosts whose file pickers can't block a frame (Android): the app asks, keeps what the pick is
//! for, and finishes when the host answers.

use std::cell::RefCell;
use std::rc::Rc;

use cadcraft_engine::Session;
use cadcraft_ui_egui::{CadApp, PickRequest, Services};
use serde_json::Value;

fn app() -> (CadApp, Rc<RefCell<Vec<PickRequest>>>) {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&asked);
    let services = Services { pick_async: Some(Box::new(move |r| log.borrow_mut().push(r))), ..Services::default() };
    (CadApp::new(Session::new(), services), asked)
}

#[test]
fn open_asks_the_host_and_waits_for_its_answer() {
    let (mut app, asked) = app();
    let _ = app.run("ui.open", Value::Null);
    assert_eq!(asked.borrow().as_slice(), &[PickRequest::Open]);
    assert!(app.pick_pending.is_some(), "the open waits for the pick");
    app.finish_pick(None);
    assert!(app.pick_pending.is_none(), "cancelled: nothing left waiting");
}

#[test]
fn a_blocking_picker_is_used_when_there_is_one() {
    let (mut app, asked) = app();
    app.services.pick_open = Some(Box::new(|| None));
    let _ = app.run("ui.open", Value::Null);
    assert!(asked.borrow().is_empty());
    assert!(app.pick_pending.is_none());
}
