//! WordCraft's file dialogs on Android: one folder browser for Open and Save As (and Export,
//! Insert Picture…), drawn over the app. ChromeOS shows its own picker to Android apps only for
//! opening files, so the app browses the user's folders itself and every dialog looks and works the
//! same: locations and recent folders on the left, the folder's contents on the right, and when
//! saving a name box. The app asks through its file dialog hook ([`Request`]) and gets back a
//! local path, as from a desktop dialog ([`crate::files`] does the rest).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Align2, Sense, Ui, pos2, vec2};
use wordcraft_ui_egui::{FileDialogAnswer, FileDialogRequest, open_filters};

use crate::files::{Files, Item};

const SIDE_W: f32 = 220.0;

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

fn row(ui: &mut Ui, icon: &str, name: &str, detail: &str, selected: bool) -> egui::Response {
    let visuals = ui.visuals().clone();
    let height = if detail.is_empty() { 30.0 } else { 42.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, name));
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 4.0, visuals.selection.bg_fill);
    } else if resp.hovered() {
        p.rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let text = visuals.text_color();
    let font = egui::FontId::proportional(14.0);
    p.text(pos2(rect.left() + 16.0, rect.center().y), Align2::CENTER_CENTER, icon, font.clone(), text);
    let clip = p.with_clip_rect(rect.intersect(p.clip_rect()));
    if detail.is_empty() {
        clip.text(pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, name, font, text);
    } else {
        clip.text(pos2(rect.left() + 34.0, rect.center().y - 8.0), Align2::LEFT_CENTER, name, font, text);
        clip.text(pos2(rect.left() + 34.0, rect.center().y + 9.0), Align2::LEFT_CENTER, detail, egui::FontId::proportional(11.5), visuals.weak_text_color());
    }
    resp
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
        let size = ctx.content_rect().size();
        let modal = egui::Modal::new(egui::Id::new("android_file_browser")).show(ctx, |ui| {
            ui.set_width((size.x - 80.0).clamp(320.0, 900.0));
            ui.heading(if saving { "Save As" } else { "Open" });
            ui.add_space(8.0);
            if saving {
                ui.horizontal(|ui| {
                    let label = ui.label("File name");
                    let resp = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(360.0)).labelled_by(label.id);
                    if resp.changed() {
                        self.replace = None;
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let ready = !self.trail.is_empty() && !self.name.trim().is_empty();
                    save_now = (ui.add_enabled(ready, egui::Button::new("Save")).clicked() || enter) && ready;
                    if ui.button("Cancel").clicked() {
                        answer = Some(Ok(Vec::new()));
                    }
                });
                if self.replace.is_some() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} already exists. Replace it?", self.name.trim()));
                        if ui.button("Replace").clicked() {
                            save_now = true;
                        }
                    });
                }
                ui.add_space(8.0);
            }
            if let Some(note) = &self.note {
                ui.colored_label(ui.visuals().warn_fg_color, note);
                ui.add_space(6.0);
            }
            let height = (size.y - 220.0).clamp(200.0, 560.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(SIDE_W);
                    egui::ScrollArea::vertical().id_salt("places").max_height(height).show(ui, |ui| {
                        ui.strong("Locations");
                        let at = self.trail.first().map(|(id, _)| id.clone());
                        for p in &places {
                            if row(ui, "📁", &p.name, "", at.as_deref() == Some(p.id.as_str())).clicked() {
                                go = Some(p.clone());
                            }
                        }
                        if row(ui, "+", "Add a location…", "", false).clicked() {
                            add_place = true;
                        }
                        if !recent.is_empty() {
                            ui.add_space(10.0);
                            ui.strong("Recent folders");
                            for r in &recent {
                                if row(ui, "🕘", &r.name, &r.detail, false).clicked() {
                                    go = Some(r.clone());
                                }
                            }
                        }
                    });
                });
                ui.separator();
                ui.vertical(|ui| {
                    if self.trail.is_empty() {
                        ui.label("Choose Add a location… and pick a folder WordCraft may use, such as My files.");
                        return;
                    }
                    let mut up = None;
                    ui.horizontal_wrapped(|ui| {
                        let last = self.trail.len() - 1;
                        for (i, (_, name)) in self.trail.iter().enumerate() {
                            if i > 0 {
                                ui.label("›");
                            }
                            if i == last {
                                ui.strong(name.as_str());
                            } else if ui.link(name.as_str()).clicked() {
                                up = Some(i);
                            }
                        }
                    });
                    if let Some(i) = up {
                        let trail = self.trail[..=i].to_vec();
                        self.go(trail);
                        return;
                    }
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().id_salt("listing").max_height(height).show(ui, |ui| match &self.listing {
                        Some(Ok(items)) if items.is_empty() => {
                            ui.weak("Nothing here to open.");
                        }
                        Some(Ok(items)) => {
                            for item in items {
                                let icon = if item.folder { "📁" } else { "📄" };
                                let selected = saving && !item.folder && self.name.trim() == item.name;
                                if row(ui, icon, &item.name, "", selected).clicked() {
                                    picked = Some(item.clone());
                                }
                            }
                        }
                        Some(Err(e)) => {
                            ui.weak(e.as_str());
                        }
                        None => {}
                    });
                });
            });
            if !saving {
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        answer = Some(Ok(Vec::new()));
                    }
                });
            }
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(Ok(Vec::new()));
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
