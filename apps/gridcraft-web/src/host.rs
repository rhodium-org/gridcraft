//! Host modes: a page that runs GridCraft (as an iframe, or in its own document) opens workbooks
//! in it and stores what it saves instead of the browser downloading. The messages and the two
//! transports are `craft-web-host`'s (storytold/craft-libs), the same contract as the other Craft
//! apps' bridges, with the `gridcraft:` prefix. This module is GridCraft's part of it.
//!
//! - `?host=parent` (an `<iframe>`, such as a Nextcloud app) talks to the same-origin parent over
//!   `postMessage`; `data-host="page"` on the canvas (the page's own document, no iframe) uses
//!   DOM `CustomEvent`s on the canvas instead. `?host=parent` wins when both are present. With no
//!   parent, or a parent on another origin, there is no bridge: the normal app with downloads.
//! - `gridcraft:ready` once the app is listening; send a file now. GridCraft reads bytes, so it
//!   says `file: false` and a host sends `bytes`.
//! - `gridcraft:open` (from the host): `{ name, bytes }`. It opens like a workbook picked with
//!   File › Open or dropped on the window; a file over [`MAX_OPEN_BYTES`] is refused before it is
//!   copied.
//! - `gridcraft:save` (to the host): `{ name, saveAs, bytes }` for every file GridCraft would
//!   otherwise download: Save and Save As (an .xlsx workbook) and the PDF, CSV and web page
//!   exports. The host stores it and reports failures itself.
//! - `gridcraft:dirty` whenever unsaved changes appear or go away, `gridcraft:failed` when the
//!   app couldn't start.
//!
//! - Beyond the crate's contract ([`crate::host_ext`], rhodium-org fork): `gridcraft:places`
//!   lists the host's places in backstage Open and Save As, choosing one sends
//!   `gridcraft:open-request`, and a Save As into one carries `place`; `gridcraft:document-place`
//!   says where a workbook lives, for File › Info and Recent; `gridcraft:autosave {available}`
//!   lets AutoSave run through a host that writes in place; after `gridcraft:confirm-saves
//!   {value: true}` each workbook Save carries an `id` and counts once the host answers
//!   `gridcraft:saved {id}`, while `gridcraft:save-failed {id, message}` leaves it unsaved and
//!   shows why. `gridcraft:save` says `saveAs` for Save As and `export` for anything but the .xlsx
//!   workbook, and `auto: true` for a save AutoSave sent. `gridcraft:document` says which workbook
//!   is shown, whenever that changes.
//!
//! In both host modes the page starts with an empty workbook instead of a sample, saves and PDF
//! exports are handed to the page rather than downloaded, and `?author=` names the user for new
//! notes and comments.

use std::cell::RefCell;
use std::rc::Rc;

use craft_web_host::web::{FileData, Host, Message};
use gridcraft_ui_egui::host_places::{self, HostPlaces, HostSave, SaveOutcome, ShownDocument};
use gridcraft_ui_egui::{Inbox, Services, SheetApp};

use crate::host_ext::{Ext, Save};
use crate::hostmode::open_name;

/// The app id in the host messages (`gridcraft:ready`).
const APP: &str = "gridcraft";

/// The largest workbook taken from the host page, refused before it is copied.
const MAX_OPEN_BYTES: u64 = 512 << 20;

/// The longest user name taken from `?author=`.
const MAX_AUTHOR_CHARS: usize = 200;

#[derive(Clone)]
pub struct Bridge {
    host: Rc<Host>,
    ext: Ext,
    /// The workbook the host was last told was shown: it hears again only when that changes.
    said: Rc<RefCell<Option<ShownDocument>>>,
}

impl Bridge {
    /// The bridge for this page, or `None` when GridCraft isn't hosted: no `?host=parent` and no
    /// `data-host="page"` on the canvas, an unframed `?host=parent` page (it stays the normal app
    /// with downloads), or a parent on another origin.
    pub fn detect(canvas: &web_sys::HtmlCanvasElement) -> Option<Self> {
        let host = Host::connect(APP, canvas)?;
        let ext = Ext::new(APP, host.mode(), canvas)?;
        Some(Self { host: Rc::new(host), ext, said: Rc::default() })
    }

    /// Send saved and exported files to the host instead of downloading them, and hear files
    /// from it. The host's places, AutoSave and confirmed saves ([`crate::host_ext`]) are filled
    /// in too; each stays as it was before until the host declares it.
    pub fn connect(&self, services: &mut Services, inbox: Inbox, ctx: egui::Context) {
        let places = HostPlaces::default();
        self.listen(inbox, ctx.clone(), places.clone());
        self.listen_ext(places.clone(), ctx);
        // A workbook Save or Save As: the app numbers it for a host that confirms saves.
        let bridge = self.clone();
        services.host_save = Some(Box::new(move |name: &str, bytes: &[u8], save: &HostSave| {
            let name = file_name(name);
            bridge.ext.post_save(&Save {
                name: &name,
                bytes,
                save_as: save.save_as,
                export: !host_places::is_workbook_name(&name),
                id: save.id,
                place: save.place.as_deref(),
                auto: save.auto,
            })
        }));
        // Anything else that would download goes to the host too: copies, never waited for.
        let bridge = self.clone();
        services.download = Some(Box::new(move |name: &str, bytes: &[u8]| {
            let name = file_name(name);
            let export =
                Save { name: &name, bytes, save_as: false, export: !host_places::is_workbook_name(&name), id: None, place: None, auto: false };
            if let Err(e) = bridge.ext.post_save(&export) {
                log::error!("couldn't hand {name} to the page: {e}");
            }
        }));
        services.host_places = Some(places);
        let ext = self.ext.clone();
        services.host_open = Some(Box::new(move |place: &str, key: Option<&str>| ext.post_open_request(place, key)));
    }

    /// `?author=`: the host's name for the user signs new notes and comments.
    pub fn set_author(app: &mut SheetApp, author: &str) {
        let author: String = author.trim().chars().take(MAX_AUTHOR_CHARS).collect();
        if !author.is_empty() {
            app.session.prefs.user_name = author;
        }
    }

    pub fn post_ready(&self) {
        let _ = self.host.post_ready(env!("CARGO_PKG_VERSION"), false);
    }

    pub fn post_failed(&self, error: &str) {
        let _ = self.host.post_failed(error, None);
    }

    /// After each frame: the host hears which workbook is shown when that changed, then whether
    /// there is unsaved work (a save still waiting for its answer counts). The crate does not
    /// repeat a dirty state it already reported.
    pub fn sync(&self, app: &SheetApp) {
        let shown = app.shown_document();
        if self.said.borrow().as_ref() != Some(&shown) && self.ext.post_document(&shown).is_ok() {
            *self.said.borrow_mut() = Some(shown);
        }
        self.host.post_dirty(app.host_unsaved());
    }

    /// Hear `gridcraft:open` and queue the file like a dropped one, and `gridcraft:saved` for a
    /// numbered save.
    fn listen(&self, inbox: Inbox, ctx: egui::Context, places: HostPlaces) {
        self.host.listen(move |message| match message {
            Message::Open { name, data } => deliver_open(&inbox, &ctx, &places, &name, &data),
            Message::Saved { id } => {
                if let Some(ticket) = host_places::ticket_from(&serde_json::Value::String(id)) {
                    places.finish(ticket, SaveOutcome::Saved);
                    ctx.request_repaint();
                }
            }
            _ => {}
        });
    }

    /// Hear the extension messages ([`crate::host_ext`]).
    fn listen_ext(&self, places: HostPlaces, ctx: egui::Context) {
        self.ext.listen(move |kind, message| {
            match kind {
                "places" => places.set(host_places::places_from(&message)),
                "document-place" => {
                    if let Some(place) = host_places::document_place_from(&message) {
                        places.arrived(place);
                    }
                }
                "autosave" => places.set_rewrites(host_places::flag(&message, "available")),
                "confirm-saves" => places.set_confirms(host_places::flag(&message, "value")),
                "save-failed" => {
                    if let Some((ticket, outcome)) = host_places::save_failed_from(&message) {
                        places.finish(ticket, outcome);
                    }
                }
                _ => return,
            }
            ctx.request_repaint();
        });
    }
}

/// The file an open message carries, pushed onto the inbox for the next frame, like a dropped
/// file. A host's file is untrusted input: an oversized one is refused before it is copied into
/// the app's memory (the data's own length, not the host's claim).
fn deliver_open(inbox: &Inbox, ctx: &egui::Context, places: &HostPlaces, name: &str, data: &FileData) {
    let FileData::Bytes(array) = data else {
        log::warn!("host file {name} came as a File; GridCraft reads bytes only; not opened");
        return;
    };
    if data.size() > MAX_OPEN_BYTES as f64 {
        log::warn!("host file is {} bytes, over the {} MiB host open limit; not opened", data.size(), MAX_OPEN_BYTES >> 20);
        return;
    }
    let name = open_name(name);
    // The app tells the host's files from dropped ones by name when it opens them.
    places.sent(&name);
    inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, array.to_vec()));
    ctx.request_repaint();
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}
