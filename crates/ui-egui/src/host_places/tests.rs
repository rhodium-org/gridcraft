use std::cell::RefCell;
use std::rc::Rc;

use gridcraft_engine::Session;
use serde_json::json;

use super::{
    DocumentPlace, HostPlace, HostPlaces, HostSave, MAX_PLACES, SaveOutcome, document_place_from, places_from, save_failed_from, ticket_from,
};
use crate::{Services, SheetApp};

fn github() -> HostPlace {
    HostPlace { id: "github".into(), label: "GitHub".into(), icon: None, open: true, save_as: true }
}

/// What a hosted app handed its host: saves as (name, what kind) and open requests as (place, key).
#[derive(Default)]
struct Seen {
    saves: Vec<(String, HostSave)>,
    exports: Vec<String>,
    opens: Vec<(String, Option<String>)>,
}

/// An app hosted by a page, as the web shell's bridge sets it up: workbook saves to the host,
/// exports through `download`, places and open requests through it.
fn hosted() -> (SheetApp, HostPlaces, Rc<RefCell<Seen>>) {
    let places = HostPlaces::default();
    let seen = Rc::new(RefCell::new(Seen::default()));
    let (saves, exports, opens) = (seen.clone(), seen.clone(), seen.clone());
    let services = Services {
        download: Some(Box::new(move |name: &str, _: &[u8]| exports.borrow_mut().exports.push(name.to_string()))),
        host_save: Some(Box::new(move |name: &str, _: &[u8], save: &HostSave| {
            saves.borrow_mut().saves.push((name.to_string(), save.clone()));
            Ok(())
        })),
        host_places: Some(places.clone()),
        host_open: Some(Box::new(move |place: &str, key: Option<&str>| {
            opens.borrow_mut().opens.push((place.to_string(), key.map(str::to_string)));
            Ok(())
        })),
        ..Default::default()
    };
    let mut session = Session::new();
    session.new_workbook();
    (SheetApp::new(session, services), places, seen)
}

/// The host sends `name`, as `gridcraft:open` does: the bridge notes it and queues it in the inbox.
fn host_opens(a: &mut SheetApp, places: &HostPlaces, name: &str) {
    let bytes = {
        let r = a.session.run("file.saveBytes", json!({"format": "xlsx"})).unwrap();
        gridcraft_engine::io::base64_decode(r["base64"].as_str().unwrap()).unwrap()
    };
    places.sent(name);
    let inbox = a.services.inbox.get_or_insert_with(Default::default).clone();
    inbox.lock().unwrap().push((name.to_string(), bytes));
    a.poll_async_opens();
}

fn edit(a: &mut SheetApp, value: &str) {
    a.run("cell.set", json!({"cell": "A1", "input": value})).unwrap();
}

fn dirty(a: &SheetApp) -> bool {
    a.session.active().unwrap().is_dirty()
}

#[test]
fn a_hosts_places_are_read_and_junk_is_skipped() {
    let message = json!({"places": [
        {"id": "github", "label": "GitHub", "open": true, "saveAs": true, "icon": "data:image/svg+xml;base64,AAAA"},
        {"id": "github", "label": "Again", "open": true},
        {"id": "drive", "label": " Drive\n ", "saveAs": true, "icon": "javascript:alert(1)"},
        {"id": "nowhere", "label": "Neither"},
        {"label": "No id", "open": true},
        {"id": "x", "open": true},
        {"id": 3, "label": "Number id", "open": true},
        "a string",
        null
    ]});
    let got = places_from(&message);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].id, "github");
    assert!(got[0].icon.is_some());
    assert_eq!(got[1], HostPlace { id: "drive".into(), label: "Drive".into(), icon: None, open: false, save_as: true });

    let many = json!({"places": (0..40).map(|i| json!({"id": format!("p{i}"), "label": "P", "open": true})).collect::<Vec<_>>()});
    assert_eq!(places_from(&many).len(), MAX_PLACES);
    for junk in [json!(null), json!({}), json!({"places": "github"}), json!([1, 2])] {
        assert!(places_from(&junk).is_empty());
    }
}

#[test]
fn a_document_place_needs_a_place_and_a_key_or_a_label() {
    let both = document_place_from(&json!({"place": "github", "key": "acme/books@main:Budget.xlsx", "label": "acme/books · main · Budget.xlsx"}));
    assert_eq!(both.map(|d| d.key), Some("acme/books@main:Budget.xlsx".into()));
    let label_only = document_place_from(&json!({"place": "github", "label": "Budget"})).map(|d| (d.key, d.label));
    assert_eq!(label_only, Some(("Budget".into(), "Budget".into())));
    for junk in [json!({"place": "github"}), json!({"key": "k", "label": "l"}), json!({"place": "", "key": "k"}), json!(null)] {
        assert_eq!(document_place_from(&junk), None, "{junk}");
    }
}

#[test]
fn save_numbers_and_failures_are_read_from_the_host() {
    assert_eq!(ticket_from(&json!("7")), Some(7));
    assert_eq!(ticket_from(&json!(7)), Some(7));
    assert_eq!(ticket_from(&json!("seven")), None);
    assert_eq!(save_failed_from(&json!({"id": "3", "message": "the branch moved"})), Some((3, SaveOutcome::Failed("the branch moved".into()))));
    assert!(matches!(save_failed_from(&json!({"id": 3})), Some((3, SaveOutcome::Failed(_)))));
    assert_eq!(save_failed_from(&json!({"message": "no id"})), None);
}

/// Save and Save As go to the host as an .xlsx workbook, Save As saying so; exports go through
/// `download`. Without confirmed saves a handed-over workbook counts as saved at once.
#[test]
fn saves_reach_the_host_as_save_or_save_as_and_count_at_once() {
    let (mut a, _, seen) = hosted();
    edit(&mut a, "1");
    let r = a.run("file.save", json!({})).unwrap();
    assert!(r.get("pendingSave").is_none(), "{r}");
    assert!(!dirty(&a), "handed over: saved");
    edit(&mut a, "2");
    a.run("ui.dialog", json!({"name": "saveAs"})).unwrap();
    edit(&mut a, "3");
    a.run("file.saveAs", json!({})).unwrap();
    let saves = seen.borrow().saves.iter().map(|(n, s)| (n.ends_with(".xlsx"), s.save_as, s.id, s.auto)).collect::<Vec<_>>();
    assert_eq!(saves, vec![(true, false, None, false), (true, true, None, false), (true, true, None, false)]);
    assert!(crate::backstage::export_as(&mut a, "csv"));
    assert_eq!(seen.borrow().exports.len(), 1, "an export is a copy, never a workbook save");
    assert_eq!(seen.borrow().saves.len(), 3);
}

/// After `confirm-saves`, a Save carries a number and the workbook stays unsaved (for the host
/// too) until the host answers; a refusal keeps it unsaved and says why.
#[test]
fn a_confirmed_save_counts_only_once_the_host_answers() {
    let (mut a, places, seen) = hosted();
    places.set_confirms(true);
    edit(&mut a, "1");
    let r = a.run("file.save", json!({})).unwrap();
    let first = seen.borrow().saves[0].1.id.unwrap();
    assert_eq!(r["pendingSave"], json!(first.to_string()));
    a.settle_host();
    assert!(dirty(&a) && a.host_unsaved() && a.shown_document().dirty, "waiting for the answer");

    places.finish(first, SaveOutcome::Failed("the branch has moved on".into()));
    a.settle_host();
    assert!(dirty(&a), "refused: still unsaved");
    assert!(a.message.as_ref().is_some_and(|(_, m)| m.contains("the branch has moved on")), "{:?}", a.message);
    a.message = None;

    a.run("file.save", json!({})).unwrap();
    let second = seen.borrow().saves[1].1.id.unwrap();
    assert_ne!(second, first);
    // Typed while the save was out: still unsaved after the answer.
    places.finish(second, SaveOutcome::Saved);
    edit(&mut a, "2");
    a.settle_host();
    assert!(dirty(&a), "the change made while the save was out isn't in it");
    a.run("file.save", json!({})).unwrap();
    let third = seen.borrow().saves[2].1.id.unwrap();
    places.finish(third, SaveOutcome::Saved);
    places.finish(999, SaveOutcome::Saved);
    a.settle_host();
    assert!(!dirty(&a) && !a.host_unsaved(), "confirmed");
}

/// A host with places to open from owns Open: no browser picker, backstage Open instead, and
/// choosing a place asks the host. A Save As into a place names it.
#[test]
fn open_and_save_as_name_a_place_the_host_offers() {
    let (mut a, places, seen) = hosted();
    let picked = Rc::new(std::cell::Cell::new(false));
    let p = picked.clone();
    a.services.open_async = Some(Box::new(move || p.set(true)));
    a.open_dialog("open", json!({}));
    assert!(picked.get(), "no places yet: the browser's picker");
    picked.set(false);
    places.set(vec![github(), HostPlace { id: "inbox".into(), label: "Inbox".into(), icon: None, open: true, save_as: false }]);
    a.open_dialog("open", json!({}));
    assert!(!picked.get(), "the host owns Open");
    assert_eq!(a.backstage.as_ref().map(|b| b.page), Some(crate::backstage::Page::Open));

    assert_eq!(a.run("ui.hostOpen", json!({"place": "inbox"})).unwrap(), json!({"asked": "inbox"}));
    assert!(a.run("ui.hostOpen", json!({"place": "nowhere"})).is_err());
    assert!(a.run("ui.hostOpen", json!({"place": "github", "key": 3})).is_err());
    assert_eq!(seen.borrow().opens, vec![("inbox".to_string(), None)]);

    a.run("ui.hostSaveAs", json!({"place": "github"})).unwrap();
    assert!(a.run("ui.hostSaveAs", json!({"place": "inbox"})).is_err(), "Inbox takes no saves");
    a.run("file.save", json!({})).unwrap();
    let saves: Vec<_> = seen.borrow().saves.iter().map(|(_, s)| (s.save_as, s.place.clone())).collect();
    assert_eq!(saves, vec![(true, Some("github".into())), (false, None)], "the place goes with one save only");
    let listed = a.run("ui.hostPlaces", json!({})).unwrap();
    assert_eq!(listed["places"].as_array().map(Vec::len), Some(2));
}

/// A workbook the host sent, and where it said it lives: File › Info, `ui.hostPlaces` and the
/// Recent entry, which reopens through the host.
#[test]
fn a_recent_workbook_from_a_place_reopens_through_the_host() {
    let (mut a, places, seen) = hosted();
    places.set(vec![github()]);
    host_opens(&mut a, &places, "Budget.xlsx");
    let place =
        DocumentPlace { place: "github".into(), key: "acme/books@main:Budget.xlsx".into(), label: "acme/books · main · Budget.xlsx".into() };
    places.arrived(place.clone());
    a.settle_host();
    assert_eq!(a.ui.recent.first().map(String::as_str), Some("Budget.xlsx"));
    assert_eq!(a.ui.recent_places.get("Budget.xlsx"), Some(&place));
    assert_eq!(a.run("ui.hostPlaces", json!({})).unwrap()["document"]["label"], json!(place.label));
    assert_eq!(a.shown_document().name.as_deref(), Some("Budget.xlsx"), "the host knows it by the name it sent");

    a.open_recent_from_host("Budget.xlsx").unwrap().unwrap();
    assert_eq!(seen.borrow().opens, vec![("github".to_string(), Some(place.key.clone()))]);
    assert!(a.open_recent_from_host("Elsewhere.xlsx").is_none());
}

/// AutoSave: refused in the browser until the host says it writes in place; then it saves the
/// workbook the host knows, marked `auto`, and waits for a numbered save's answer.
#[test]
fn autosave_runs_only_through_a_host_that_writes_in_place() {
    let (mut a, places, seen) = hosted();
    let refused = a.run("file.autosave", json!({"value": true}));
    assert!(refused.is_err_and(|e| e.contains("browser")));
    assert!(!a.ui.autosave);
    places.set_rewrites(true);
    places.set_confirms(true);
    assert_eq!(a.run("file.autosave", json!({"value": true})).unwrap(), json!({"value": true}));
    let ctx = egui::Context::default();

    // A new workbook the host has never had isn't AutoSaved.
    edit(&mut a, "1");
    a.grid.last_autosave = crate::monotonic_ms() - 3000.0;
    a.autosave(&ctx);
    assert!(seen.borrow().saves.is_empty());

    host_opens(&mut a, &places, "Budget.xlsx");
    edit(&mut a, "2");
    a.grid.last_autosave = crate::monotonic_ms() - 3000.0;
    a.autosave(&ctx);
    let (name, save) = seen.borrow().saves.last().cloned().unwrap();
    assert_eq!((name.as_str(), save.auto, save.save_as), ("Budget.xlsx", true, false));
    // Out and unanswered: the next AutoSave waits for it.
    edit(&mut a, "3");
    a.grid.last_autosave = crate::monotonic_ms() - 3000.0;
    a.autosave(&ctx);
    assert_eq!(seen.borrow().saves.len(), 1);
    // Refused: AutoSave stops for it until the person saves.
    places.finish(save.id.unwrap(), SaveOutcome::Failed("conflict".into()));
    a.settle_host();
    a.grid.last_autosave = crate::monotonic_ms() - 3000.0;
    a.autosave(&ctx);
    assert_eq!(seen.borrow().saves.len(), 1);
    a.run("file.save", json!({})).unwrap();
    assert!(!seen.borrow().saves[1].1.auto, "the person's Save");

    places.set_rewrites(false);
    assert!(a.run("file.autosave", json!({"value": true})).is_err(), "the host took AutoSave back");
}

/// The host hears which workbook is shown: a new one, and the first again.
#[test]
fn the_host_hears_which_workbook_is_shown() {
    let (mut a, places, _) = hosted();
    host_opens(&mut a, &places, "Budget.xlsx");
    let first = a.shown_document();
    assert_eq!((first.name.as_deref(), first.open), (Some("Budget.xlsx"), true));
    a.run("file.new", json!({})).unwrap();
    let new = a.shown_document();
    assert_ne!(new.id, first.id);
    assert_eq!(new.name, None, "no file yet");
    assert!(new.documents.contains(&first.id) && new.documents.contains(&new.id), "{new:?}");
    a.run("window.activate", json!({"doc": first.id})).unwrap();
    assert_eq!(a.shown_document().id, first.id);
}
