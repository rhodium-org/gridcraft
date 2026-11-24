//! Web, hosted: the app's side of the host page's places, AutoSave through it and the saves it
//! confirms ([`crate::host_places`]). Nothing here runs without [`Services::host_save`]: the
//! desktop app and the plain web app are unchanged.
//!
//! [`Services::host_save`]: crate::Services::host_save

use crate::host_places::{DocumentPlace, HostSave, PendingSave, SaveOutcome, ShownDocument};

use super::{Json, SheetApp, json};

impl SheetApp {
    /// Web, hosted: workbook saves go to the host page ([`crate::Services::host_save`]).
    pub(crate) fn hosted(&self) -> bool {
        self.services.host_save.is_some()
    }

    /// Web, hosted: whether the host page says it writes Save back in place, so AutoSave may run.
    pub(crate) fn host_rewrites(&self) -> bool {
        self.hosted() && self.services.host_places.as_ref().is_some_and(crate::host_places::HostPlaces::rewrites)
    }

    /// Web, hosted: whether the host page declared a place for Open (`save_as: false`) or Save As.
    /// It then owns that: GridCraft's own Browse… gives way to its places.
    pub(crate) fn host_owns(&self, save_as: bool) -> bool {
        self.hosted() && self.services.host_places.as_ref().is_some_and(|p| p.offers(save_as))
    }

    /// Why AutoSave can't be turned on, or `None` when it can: in the browser a page can't write
    /// back to the person's files, unless a host page says it writes Save back in place.
    pub(crate) fn autosave_refused(&self) -> Option<String> {
        let browser = self.services.pick_save.is_none() && self.services.download.is_some();
        (browser && !self.host_rewrites())
            .then(|| "AutoSave can't run in the browser: a web page can't write back to your files, so Save downloads a copy instead.".to_string())
    }

    /// Whether the host should guard leaving the page: a workbook has unsaved changes, or a save
    /// is still waiting for the host's answer.
    pub fn host_unsaved(&self) -> bool {
        self.session.documents().iter().any(gridcraft_engine::DocState::is_dirty) || !self.host_docs.pending.is_empty()
    }

    /// The workbook the window shows, for the host page (`gridcraft:document`).
    pub fn shown_document(&self) -> ShownDocument {
        let documents: Vec<u64> = self.session.documents().iter().map(|d| d.uid).collect();
        match self.session.active() {
            Some(d) => ShownDocument {
                id: d.uid,
                name: self.host_docs.names.get(&d.uid).cloned().or_else(|| d.path.as_ref().map(|_| d.display_title())),
                title: d.display_title(),
                open: true,
                dirty: d.is_dirty() || self.host_docs.waiting(d.uid),
                documents,
            },
            None => ShownDocument { id: 0, name: None, title: String::new(), open: false, dirty: false, documents },
        }
    }

    /// Save or Save As (`save_as`) the active workbook through the host page, as an .xlsx
    /// workbook. Without confirmed saves it counts as saved once handed over; with them it carries
    /// a number and counts once the host answers ([`Self::settle_host`]).
    pub(crate) fn save_to_host(&mut self, save_as: bool) -> Result<Json, String> {
        self.store_view_state();
        let r = self.session.run("file.saveBytes", json!({"format": "xlsx"})).map_err(|e| crate::clean_error(&e.to_string()))?;
        let Some(bytes) = r.get("base64").and_then(Json::as_str).and_then(gridcraft_engine::io::base64_decode) else {
            return Err("the workbook couldn't be encoded".into());
        };
        let Some((uid, snapshot)) = self.session.active().map(|d| (d.uid, d.wb.clone())) else {
            return Err("no workbook is open".into());
        };
        // The name the host knows the workbook by, as an .xlsx workbook.
        let name = r.get("name").and_then(Json::as_str).unwrap_or("Book.xlsx").to_string();
        let confirms = self.services.host_places.as_ref().is_some_and(crate::host_places::HostPlaces::confirms);
        let id = confirms.then(|| self.host_docs.ticket());
        let save = HostSave { save_as, id, place: self.host_docs.save_place.take(), auto: self.host_docs.auto };
        let Some(hand) = &self.services.host_save else { return Err("no host page keeps workbooks here".into()) };
        hand(&name, &bytes, &save).map_err(|e| format!("Couldn't hand {name} to the page: {e}"))?;
        self.host_docs.names.insert(uid, name.clone());
        if !save.auto {
            // A save the person asked for turns AutoSave back on for a workbook it failed on.
            self.host_docs.autosave_stopped.remove(&uid);
        }
        match id {
            Some(ticket) => {
                self.host_docs.pending.push(PendingSave { ticket, doc: uid, name: name.clone(), snapshot, auto: save.auto });
                Ok(json!({"name": name, "pendingSave": ticket.to_string()}))
            }
            None => {
                self.mark_host_saved(uid, snapshot, &name);
                Ok(json!({"name": name}))
            }
        }
    }

    /// The workbook `uid` is saved as `snapshot` under `name`: later changes still count as unsaved.
    fn mark_host_saved(&mut self, uid: u64, snapshot: std::sync::Arc<gridcraft_engine::model::Workbook>, name: &str) {
        if let Some(doc) = self.session.document_mut(uid) {
            doc.saved = snapshot;
            if doc.path.is_none() {
                doc.title = name.to_string();
            }
        }
    }

    /// A file just opened from the inbox under `name`: if the host sent it, the host knows the
    /// workbook by that name (AutoSave may write it back when it is an .xlsx workbook).
    pub(crate) fn note_host_open(&mut self, name: &str) {
        let Some(places) = &self.services.host_places else { return };
        if places.take_sent(name)
            && let Some(uid) = self.session.active().map(|d| d.uid)
        {
            self.host_docs.names.insert(uid, name.to_string());
        }
    }

    /// Every frame, after files from the inbox opened: the host's answers to numbered saves, and
    /// where the workbooks it just opened or saved live. The Recent entry for one remembers its
    /// place, so it reopens through the host.
    pub(crate) fn settle_host(&mut self) {
        let Some(places) = self.services.host_places.clone() else { return };
        for (ticket, outcome) in places.take_finished() {
            let Some(i) = self.host_docs.pending.iter().position(|p| p.ticket == ticket) else { continue };
            let save = self.host_docs.pending.swap_remove(i);
            match outcome {
                SaveOutcome::Saved => self.mark_host_saved(save.doc, save.snapshot, &save.name),
                SaveOutcome::Failed(why) => {
                    log::warn!("the page didn't store {}: {why}", save.name);
                    if save.auto {
                        // AutoSave waits for a save the person asks for, instead of failing again.
                        self.host_docs.autosave_stopped.insert(save.doc);
                    }
                    self.message = Some(("GridCraft".into(), format!("Couldn't save {}: {why}", save.name)));
                }
            }
        }
        let open: Vec<u64> = self.session.documents().iter().map(|d| d.uid).collect();
        self.host_docs.keep(&open);
        places.keep_documents(&open);
        let Some(place) = places.take_arrived().into_iter().last() else { return };
        let Some((uid, title)) = self.session.active().map(|d| (d.uid, d.display_title())) else { return };
        places.set_document(uid, place.clone());
        self.remember_recent_place(title, place);
    }

    /// The Recent entry `entry` lives in a host's place.
    fn remember_recent_place(&mut self, entry: String, place: DocumentPlace) {
        self.ui.recent.retain(|r| *r != entry);
        self.ui.recent.insert(0, entry.clone());
        self.ui.recent.truncate(20);
        self.ui.recent_places.insert(entry, place);
        let recent = &self.ui.recent;
        self.ui.recent_places.retain(|k, _| recent.contains(k));
    }

    /// A Recent entry from a host's place reopens through the host, which sends it back; `None`
    /// for any other entry.
    pub(crate) fn open_recent_from_host(&mut self, entry: &str) -> Option<Result<Json, String>> {
        let place = self.ui.recent_places.get(entry).cloned()?;
        self.services.host_open.as_ref()?;
        Some(self.run("ui.hostOpen", json!({"place": place.place, "key": place.key})))
    }

    /// AutoSave through a host page that writes in place: the active workbook, shortly after it
    /// changes, as GridCraft's own AutoSave does for a file on disk. The save is AutoSave's
    /// (`auto: true`), so the host can store it without asking anything.
    pub(crate) fn host_autosave(&mut self, ctx: &egui::Context) {
        let Some((uid, dirty)) = self.session.active().map(|d| (d.uid, d.is_dirty())) else { return };
        if !(self.ui.autosave && dirty && self.host_docs.autosaves(uid)) {
            return;
        }
        let now = crate::monotonic_ms();
        if self.editor.is_some() || self.text_box_editor.is_some() || now - self.grid.last_autosave <= 2000.0 {
            // Keep checking even when no input arrives.
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
            return;
        }
        self.grid.last_autosave = now;
        self.host_docs.auto = true;
        let saved = self.run("file.save", json!({}));
        self.host_docs.auto = false;
        if let Err(e) = saved {
            log::warn!("AutoSave failed: {e}");
            self.host_docs.autosave_stopped.insert(uid);
            self.message = Some(("GridCraft".into(), format!("AutoSave failed: {e}. Save the workbook to turn AutoSave back on.")));
        }
    }

    /// The commands of hosting: Save and Save As through the host, `file.autosave`, and the host's
    /// places. `None` for any other command, or for Save with a `path` (a file of this computer).
    pub(super) fn host_command(&mut self, id: &str, p: &Json) -> Option<Result<Json, String>> {
        Some(match id {
            "file.save" | "file.saveAs" if self.hosted() && p.get("path").and_then(Json::as_str).is_none() => self.save_to_host(id == "file.saveAs"),
            "file.autosave" => {
                let on = p.get("value").and_then(Json::as_bool).unwrap_or(!self.ui.autosave);
                if on && let Some(why) = self.autosave_refused() {
                    return Some(Err(why));
                }
                self.ui.autosave = on;
                Ok(json!({"value": on}))
            }
            "ui.hostPlaces" => {
                let Some(places) = self.services.host_places.clone() else {
                    return Some(Ok(json!({"places": [], "document": null})));
                };
                let document = self.session.active().and_then(|d| places.document(d.uid));
                Ok(json!({"places": places.list(), "document": document}))
            }
            "ui.hostOpen" => self.host_open(p),
            "ui.hostSaveAs" => self.host_save_as(p),
            _ => return None,
        })
    }

    /// `ui.hostOpen {place, key?}`: backstage Open's place, or a Recent workbook from one (`key`).
    /// The host asks its own questions and sends the workbook back as any host file.
    fn host_open(&mut self, p: &Json) -> Result<Json, String> {
        let Some(place) = p.get("place").and_then(Json::as_str) else {
            return Err("`place` must name a place the host page offers".into());
        };
        let key = match p.get("key") {
            None | Some(Json::Null) => None,
            Some(Json::String(k)) => Some(k.as_str()),
            Some(_) => return Err("`key` must be a string".into()),
        };
        let (Some(places), Some(open)) = (self.services.host_places.as_ref(), self.services.host_open.as_ref()) else {
            return Err("no host page keeps workbooks here".into());
        };
        let Some(found) = places.get(place) else { return Err(format!("the host page offers no place `{place}`")) };
        // A Recent entry names its place even when the host no longer lists it for Open.
        if !found.open && key.is_none() {
            return Err(format!("{} does not open workbooks", found.label));
        }
        open(place, key).map_err(|e| format!("Couldn't ask {} for the workbook: {e}", found.label))?;
        Ok(json!({"asked": place}))
    }

    /// `ui.hostSaveAs {place}`: backstage Save As into one of the host's places; the host asks
    /// where in it.
    fn host_save_as(&mut self, p: &Json) -> Result<Json, String> {
        let Some(place) = p.get("place").and_then(Json::as_str) else {
            return Err("`place` must name a place the host page offers".into());
        };
        let Some(places) = self.services.host_places.as_ref().filter(|_| self.hosted()) else {
            return Err("no host page keeps workbooks here".into());
        };
        match places.get(place) {
            Some(found) if found.save_as => {}
            Some(found) => return Err(format!("{} does not take saves", found.label)),
            None => return Err(format!("the host page offers no place `{place}`")),
        }
        self.host_docs.save_place = Some(place.to_string());
        let r = self.run("file.saveAs", json!({}));
        // Claimed by the save when it went out; never left for a later one.
        self.host_docs.save_place = None;
        if r.is_ok() {
            self.backstage = None;
        }
        r
    }
}
