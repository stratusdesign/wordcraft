//! WordCraft's file dialogs on Android: one folder browser for Open and Save As (and Export,
//! Insert Picture…), drawn over the app. ChromeOS shows its own picker to Android apps only for
//! opening files, so the app browses the user's folders itself and every dialog looks and works the
//! same: locations and recent folders on the left, the folder's contents on the right, and when
//! saving a name box. The app asks through its file dialog hook ([`Request`]) and gets back a
//! local path, as from a desktop dialog ([`crate::files`] does the rest).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use wordcraft_ui_egui::{FileDialogAnswer, FileDialogRequest, open_filters};

use crate::files::{Files, Item};
use crate::page;

/// WordCraft's colour for the page's left column (GridCraft's is green); see [`page::View::accent`].
const ACCENT: [u32; 3] = [0x2B579A, 0x1E3A5F, 0xDCE6F4];

/// The dialog the app asked for and where its answer goes; shared with the hook.
pub type Pending = Rc<RefCell<Option<(FileDialogRequest, Sender<FileDialogAnswer>)>>>;

/// The `file_dialog` hook: queue the request, answer later from [`Browser::show`].
pub fn hook(pending: Pending, ctx: egui::Context) -> Box<dyn Fn(FileDialogRequest) -> Receiver<FileDialogAnswer>> {
    Box::new(move |req| {
        let (tx, rx) = channel();
        // A new dialog replaces one still open; dropping its sender answers it as cancelled.
        *pending.borrow_mut() = Some((req, tx));
        ctx.request_repaint();
        rx
    })
}

/// Where the browser is; it starts afresh for every dialog.
#[derive(Default)]
pub struct Browser {
    /// The dialog being shown (to notice a new one).
    shown: Option<FileDialogRequest>,
    /// The folders from the starting location down to the one shown: (id, name).
    trail: Vec<(String, String)>,
    listing: Option<Result<Vec<Item>, String>>,
    name: String,
    /// The existing file the user has been asked about replacing.
    replace: Option<String>,
    /// Something to tell the user inside the dialog.
    note: Option<String>,
}


/// The file types a dialog lists: `None` lists every file.
fn wanted(req: &FileDialogRequest) -> Option<Vec<String>> {
    match req {
        FileDialogRequest::Open { purpose } | FileDialogRequest::OpenMany { purpose } => {
            let exts: Vec<String> = open_filters(purpose).first().map(|(_, e)| e.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
            if exts.iter().any(|e| e == "*") { None } else { Some(exts) }
        }
        // Saving lists every file, so a name already taken asks before it is replaced.
        FileDialogRequest::Save { .. } => None,
    }
}

fn shown(item: &Item, exts: &Option<Vec<String>>) -> bool {
    item.folder
        || exts.as_ref().is_none_or(|exts| {
            std::path::Path::new(&item.name).extension().and_then(std::ffi::OsStr::to_str).is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
        })
}

impl Browser {
    fn go(&mut self, trail: Vec<(String, String)>) {
        self.trail = trail;
        self.listing = None;
        self.replace = None;
    }

    /// Draws the dialog the app is waiting on, if any, and answers it.
    pub fn show(&mut self, ctx: &egui::Context, pending: &Pending, files: &Rc<RefCell<Files>>) {
        let Some(req) = pending.borrow().as_ref().map(|(r, _)| r.clone()) else {
            self.shown = None;
            return;
        };
        if self.shown.as_ref() != Some(&req) {
            *self = Browser::default();
            if let FileDialogRequest::Save { name, .. } = &req {
                self.name = name.clone();
            }
            self.shown = Some(req.clone());
        }
        let saving = matches!(req, FileDialogRequest::Save { .. });
        let (places, recent) = {
            let f = files.borrow();
            (f.places(), f.recent())
        };
        if self.trail.is_empty()
            && let Some(start) = recent.first().or(places.first())
        {
            self.go(vec![(start.id.clone(), start.name.clone())]);
        }
        if self.listing.is_none()
            && let Some((id, _)) = self.trail.last()
        {
            let exts = wanted(&req);
            self.listing = Some(Files::list(id).map(|mut items| {
                items.retain(|i| shown(i, &exts));
                items.sort_by(|a, b| b.folder.cmp(&a.folder).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
                items
            }));
        }

        let mut answer: Option<FileDialogAnswer> = None;
        let mut add_place = false;
        let mut go = None;
        let mut picked = None;
        let mut save_now = false;
        let mut up = None;
        let typed = self.name.clone();
        let view = page::View {
            title: if saving { "Save As" } else { "Open" },
            app: "WordCraft",
            saving,
            choosing_folder: false,
            places: &places,
            recent: &recent,
            trail: &self.trail,
            listing: self.listing.as_ref(),
            replacing: self.replace.is_some(),
            note: self.note.as_deref(),
            accent: ACCENT,
        };
        let mut name = typed.clone();
        let actions = page::show(ctx, &view, &mut name);
        if name != typed {
            // A different name drops the question about replacing the old one.
            self.replace = None;
        }
        self.name = name;
        for action in actions {
            match action {
                page::Action::Cancel => answer = Some(Ok(Vec::new())),
                page::Action::AddPlace => add_place = true,
                page::Action::Go(item) => go = Some(item),
                page::Action::Pick(item) => picked = Some(item),
                page::Action::Up(i) => up = Some(i),
                page::Action::Save | page::Action::Replace => save_now = true,
                page::Action::ChooseFolder => {}
            }
        }
        if let Some(i) = up {
            let trail = self.trail[..=i].to_vec();
            self.go(trail);
        }
        if add_place {
            Files::add_place();
        }
        if let Some(place) = go {
            self.go(vec![(place.id, place.name)]);
        }
        if let Some(item) = picked {
            if item.folder {
                let mut trail = self.trail.clone();
                trail.push((item.id, item.name));
                self.go(trail);
            } else if saving {
                self.name = item.name;
                self.replace = None;
            } else {
                let (folder, folder_name) = self.trail.last().cloned().unwrap_or_default();
                match files.borrow_mut().open(&folder, &folder_name, &item) {
                    Ok(path) => answer = Some(Ok(vec![path])),
                    Err(e) => self.note = Some(format!("{}: {e}", item.name)),
                }
            }
        }
        if save_now {
            let name = self.name.trim().to_string();
            let existing = match &self.listing {
                Some(Ok(items)) => items.iter().find(|i| !i.folder && i.name == name).cloned(),
                _ => None,
            };
            if let Some(e) = &existing
                && self.replace.as_deref() != Some(e.id.as_str())
            {
                self.replace = Some(e.id.clone());
            } else if let Some((folder, folder_name)) = self.trail.last().cloned() {
                match files.borrow_mut().begin_save(&folder, &folder_name, &name, existing.as_ref()) {
                    Ok(path) => answer = Some(Ok(vec![path])),
                    Err(e) => self.note = Some(format!("{name}: {e}")),
                }
            }
        }
        if let Some(answer) = answer
            && let Some((_, tx)) = pending.borrow_mut().take()
        {
            let _ = tx.send(answer);
            ctx.request_repaint();
        }
    }
}
