//! Web, hosted: the places a host page keeps workbooks in, AutoSave through a host that writes in
//! place, and saves a host confirms or refuses (rhodium-org fork; the same extension points as
//! WordCraft's fork, with the `gridcraft:` prefix).
//!
//! A host declares its places once with `gridcraft:places`, and backstage Open and Save As list
//! them. Choosing one asks the host (`gridcraft:open-request`), or saves through it with the place
//! named (`gridcraft:save` with `place`). After an open or a save the host may say where the
//! workbook lives (`gridcraft:document-place`): File › Info shows it, and the Recent entry
//! remembers it, so a workbook from the host reopens through the host instead of failing to open
//! as a file of this computer. `gridcraft:autosave {available}` lets AutoSave run, and after
//! `gridcraft:confirm-saves {value: true}` each workbook Save carries an `id` and counts once the
//! host answers `gridcraft:saved {id}` (or `gridcraft:save-failed {id, message?}`).
//!
//! The web shell's bridge writes the host's messages into a [`HostPlaces`] and the app reads it
//! every frame ([`crate::SheetApp::settle_host`]). Plain `Rc<RefCell<…>>` state: everything runs
//! on the page's one thread. What only the app needs (numbered saves still out, the names the host
//! knows the workbooks by) is in [`HostDocs`].

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use gridcraft_engine::model::Workbook;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most places a host can declare; the rest are ignored.
pub const MAX_PLACES: usize = 16;
/// The longest place label, workbook label, key or message taken from a host, in characters.
pub const MAX_LABEL_CHARS: usize = 200;
/// The longest place id taken from a host, in characters.
pub const MAX_ID_CHARS: usize = 64;
/// The most file names the host sent that are remembered until they open.
const MAX_SENT: usize = 16;

/// A place the host keeps workbooks in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPlace {
    /// The host's own name for it, sent back in `open-request` and `save`.
    pub id: String,
    /// Shown as given.
    pub label: String,
    /// A data URI the host offered. Kept and reported, not drawn yet.
    pub icon: Option<String>,
    /// Backstage Open lists it.
    pub open: bool,
    /// Backstage Save As lists it.
    pub save_as: bool,
}

/// Where a workbook lives in a host's place: the place, the host's key for it there (sent back
/// to reopen it) and the label File › Info and Recent show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPlace {
    pub place: String,
    pub key: String,
    pub label: String,
}

/// What the app tells the host with a workbook Save ([`crate::Services::host_save`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostSave {
    /// Save As (backstage Save As, `file.saveAs`, `ui.dialog {name: "saveAs"}`), not Save.
    pub save_as: bool,
    /// The save's number, for a host that confirms saves: it counts once the host answers.
    pub id: Option<u64>,
    /// A Save As into one of the host's places.
    pub place: Option<String>,
    /// AutoSave sent it, not the person.
    pub auto: bool,
}

/// How a numbered save ended, as the host said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved,
    /// Not stored; why, in the host's words.
    Failed(String),
}

/// The workbook the window shows, for a host page (`gridcraft:document`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShownDocument {
    /// The workbook's [`gridcraft_engine::DocState::uid`]: the same while it is open; `0` when no
    /// workbook is open.
    pub id: u64,
    /// The file name the host knows it by (it sent it, or a save handed it over); `None` for a
    /// workbook with no file yet (File › New).
    pub name: Option<String>,
    /// What the title bar calls it (`Book1` for an untitled one).
    pub title: String,
    /// `false` when no workbook is open.
    pub open: bool,
    /// Unsaved changes, or a save still waiting for the host's answer.
    pub dirty: bool,
    /// Every open workbook's id, this one included: one the host kept that is gone was closed.
    pub documents: Vec<u64>,
}

#[derive(Default)]
struct State {
    places: Vec<HostPlace>,
    /// `document-place` messages not yet given to the workbook shown.
    arrived: Vec<DocumentPlace>,
    /// Where each open workbook lives, by uid.
    documents: BTreeMap<u64, DocumentPlace>,
    /// Names of files the host sent that the app hasn't opened yet ([`HostPlaces::sent`]).
    sent: Vec<String>,
    /// `autosave {available}`: the host writes Save back in place.
    rewrites: bool,
    /// `confirm-saves {value}`: each Save carries an id and waits for the host's answer.
    confirms: bool,
    /// Answers to numbered saves not yet taken by the app.
    finished: Vec<(u64, SaveOutcome)>,
}

/// The host's places and switches, shared between the bridge and the app (a cheap clone).
#[derive(Clone, Default)]
pub struct HostPlaces(Rc<RefCell<State>>);

impl HostPlaces {
    /// `gridcraft:places`: replace the list (an empty list removes them).
    pub fn set(&self, places: Vec<HostPlace>) {
        self.0.borrow_mut().places = places;
    }

    pub fn list(&self) -> Vec<HostPlace> {
        self.0.borrow().places.clone()
    }

    pub fn get(&self, id: &str) -> Option<HostPlace> {
        self.0.borrow().places.iter().find(|p| p.id == id).cloned()
    }

    /// Whether a declared place offers Open (`save_as: false`) or Save As (`save_as: true`). The
    /// host then owns that: GridCraft's own Browse… gives way to its places.
    pub fn offers(&self, save_as: bool) -> bool {
        self.0.borrow().places.iter().any(|p| if save_as { p.save_as } else { p.open })
    }

    /// `gridcraft:document-place`: the workbook just opened or saved lives here. It goes to the
    /// workbook shown at the next frame, after the file the host sent with it has opened.
    pub fn arrived(&self, place: DocumentPlace) {
        self.0.borrow_mut().arrived.push(place);
    }

    pub(crate) fn take_arrived(&self) -> Vec<DocumentPlace> {
        std::mem::take(&mut self.0.borrow_mut().arrived)
    }

    pub(crate) fn set_document(&self, uid: u64, place: DocumentPlace) {
        self.0.borrow_mut().documents.insert(uid, place);
    }

    /// Where the workbook lives, if the host said.
    pub fn document(&self, uid: u64) -> Option<DocumentPlace> {
        self.0.borrow().documents.get(&uid).cloned()
    }

    /// Forget the workbooks that are no longer open.
    pub(crate) fn keep_documents(&self, open: &[u64]) {
        self.0.borrow_mut().documents.retain(|uid, _| open.contains(uid));
    }

    /// The bridge: the host sent a file under this name (`gridcraft:open`). The app opens it from
    /// the inbox like a dropped file; this is how it tells the host's files from dropped ones.
    pub fn sent(&self, name: &str) {
        let mut s = self.0.borrow_mut();
        s.sent.push(name.to_string());
        // A host that sends files the app never opens (refused, unreadable) can't grow this.
        let extra = s.sent.len().saturating_sub(MAX_SENT);
        s.sent.drain(..extra);
    }

    /// Whether the host sent the file opened under `name` (claimed once).
    pub(crate) fn take_sent(&self, name: &str) -> bool {
        let mut s = self.0.borrow_mut();
        match s.sent.iter().position(|n| n == name) {
            Some(i) => {
                s.sent.remove(i);
                true
            }
            None => false,
        }
    }

    /// `gridcraft:autosave {available}`.
    pub fn set_rewrites(&self, on: bool) {
        self.0.borrow_mut().rewrites = on;
    }

    pub fn rewrites(&self) -> bool {
        self.0.borrow().rewrites
    }

    /// `gridcraft:confirm-saves {value}`.
    pub fn set_confirms(&self, on: bool) {
        self.0.borrow_mut().confirms = on;
    }

    pub fn confirms(&self) -> bool {
        self.0.borrow().confirms
    }

    /// `gridcraft:saved {id}` or `gridcraft:save-failed {id}`: how the numbered save ended. An id
    /// no save carries is ignored when the app takes it.
    pub fn finish(&self, ticket: u64, outcome: SaveOutcome) {
        let mut s = self.0.borrow_mut();
        // A host repeating answers can't grow this between frames without bound.
        if s.finished.len() < 1024 {
            s.finished.push((ticket, outcome));
        }
    }

    pub(crate) fn take_finished(&self) -> Vec<(u64, SaveOutcome)> {
        std::mem::take(&mut self.0.borrow_mut().finished)
    }
}

/// A workbook Save handed to a host that confirms saves, not yet answered.
#[derive(Clone, Debug)]
pub(crate) struct PendingSave {
    pub ticket: u64,
    /// The workbook it was made from ([`gridcraft_engine::DocState::uid`]).
    pub doc: u64,
    pub name: String,
    /// The workbook as saved: it is the saved state once the host confirms, so changes made
    /// while the save was out still count as unsaved.
    pub snapshot: Arc<Workbook>,
    pub auto: bool,
}

/// The app's side of hosting: numbered saves still out and the names the host knows workbooks by.
#[derive(Default)]
pub(crate) struct HostDocs {
    next_ticket: u64,
    pub pending: Vec<PendingSave>,
    /// The file name the host knows each workbook by: it sent it, or a save handed it over.
    pub names: BTreeMap<u64, String>,
    /// Workbooks whose AutoSave the host refused: it waits for a save the person asks for.
    pub autosave_stopped: BTreeSet<u64>,
    /// The place the next save goes to (a Save As into one of the host's places).
    pub save_place: Option<String>,
    /// The next save is AutoSave's.
    pub auto: bool,
}

impl HostDocs {
    /// A new save number (1, 2, …).
    pub fn ticket(&mut self) -> u64 {
        self.next_ticket = self.next_ticket.saturating_add(1);
        self.next_ticket
    }

    /// Whether a save of this workbook is still waiting for the host's answer.
    pub fn waiting(&self, doc: u64) -> bool {
        self.pending.iter().any(|p| p.doc == doc)
    }

    /// Whether AutoSave may write this workbook through a host that writes in place: the host
    /// knows it as an .xlsx workbook (another format would be replaced by an .xlsx of the same
    /// name), its AutoSave wasn't refused, and no save of it is still out.
    pub fn autosaves(&self, doc: u64) -> bool {
        self.names.get(&doc).is_some_and(|n| is_workbook_name(n)) && !self.autosave_stopped.contains(&doc) && !self.waiting(doc)
    }

    /// Forget the workbooks that are no longer open.
    pub fn keep(&mut self, open: &[u64]) {
        self.pending.retain(|p| open.contains(&p.doc));
        self.names.retain(|uid, _| open.contains(uid));
        self.autosave_stopped.retain(|uid| open.contains(uid));
    }
}

/// A file name GridCraft saves workbooks as (`.xlsx`); anything else handed to a host is an export.
pub fn is_workbook_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("xlsx"))
}

/// Text from a host: control characters dropped, surrounding space trimmed, cut to `max`
/// characters; `None` when nothing is left.
fn clean(raw: Option<&Value>, max: usize) -> Option<String> {
    let s: String = raw?.as_str()?.chars().filter(|c| !c.is_control()).collect();
    let s = s.trim();
    (!s.is_empty()).then(|| s.chars().take(max).collect())
}

/// The places a `gridcraft:places` message declares. A host's message is untrusted input: an
/// entry without an id or a label, or that offers neither Open nor Save As, is skipped, a second
/// entry with the same id too, and only the first [`MAX_PLACES`] count. An icon must be an image
/// data URI.
pub fn places_from(message: &Value) -> Vec<HostPlace> {
    let mut out: Vec<HostPlace> = Vec::new();
    let Some(entries) = message.get("places").and_then(Value::as_array) else { return out };
    for entry in entries {
        if out.len() == MAX_PLACES {
            break;
        }
        let (Some(id), Some(label)) = (clean(entry.get("id"), MAX_ID_CHARS), clean(entry.get("label"), MAX_LABEL_CHARS)) else { continue };
        let open = entry.get("open").and_then(Value::as_bool) == Some(true);
        let save_as = entry.get("saveAs").and_then(Value::as_bool) == Some(true);
        if !(open || save_as) || out.iter().any(|p| p.id == id) {
            continue;
        }
        let icon = entry.get("icon").and_then(Value::as_str).filter(|s| s.starts_with("data:image/")).map(str::to_string);
        out.push(HostPlace { id, label, icon, open, save_as });
    }
    out
}

/// The place a `gridcraft:document-place` message names, or `None` without a place. The key
/// defaults to the label, and the label to the key.
pub fn document_place_from(message: &Value) -> Option<DocumentPlace> {
    let place = clean(message.get("place"), MAX_ID_CHARS)?;
    let key = clean(message.get("key"), MAX_LABEL_CHARS);
    let label = clean(message.get("label"), MAX_LABEL_CHARS);
    let (key, label) = match (key, label) {
        (Some(k), Some(l)) => (k, l),
        (Some(k), None) => (k.clone(), k),
        (None, Some(l)) => (l.clone(), l),
        (None, None) => return None,
    };
    Some(DocumentPlace { place, key, label })
}

/// A host's on/off switch (`autosave`'s `available`, `confirm-saves`' `value`): on only for `true`.
pub fn flag(message: &Value, field: &str) -> bool {
    message.get(field).and_then(Value::as_bool) == Some(true)
}

/// A save number as the host sent it back: the string the app sent, or a number.
pub fn ticket_from(id: &Value) -> Option<u64> {
    id.as_u64().or_else(|| id.as_str().and_then(|s| s.trim().parse().ok()))
}

/// The save a `gridcraft:save-failed` message reports, as its ticket and outcome, or `None`
/// without the `id` of a numbered save. The host's message, cut to [`MAX_LABEL_CHARS`], says why.
pub fn save_failed_from(message: &Value) -> Option<(u64, SaveOutcome)> {
    let ticket = ticket_from(message.get("id")?)?;
    let why = clean(message.get("message"), MAX_LABEL_CHARS).unwrap_or_else(|| "the page didn't store it".to_string());
    Some((ticket, SaveOutcome::Failed(why)))
}

#[cfg(test)]
mod tests;
