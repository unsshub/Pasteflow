//! Dialogs for Spaces CRUD, Move-to-Space (`Ctrl+M`), and Rename/Label Clip (`F2`).

use gtk4::prelude::*;
use gtk4::{
    Align, Application, Box as GtkBox, Button, CheckButton, DropDown, Entry, Label, Orientation,
    Separator, StringList, Window,
};
use pf_core::model::SpaceRecord;
use std::rc::Rc;

const COLOR_PALETTE: &[(&str, &str)] = &[
    ("Slate", "#8a8f98"),
    ("Blue", "#3584e4"),
    ("Purple", "#c061cb"),
    ("Green", "#33d17a"),
    ("Yellow", "#f6d32d"),
    ("Orange", "#ff7800"),
    ("Red", "#e01b24"),
    ("Teal", "#2ec27e"),
    ("Pink", "#f66151"),
    ("Indigo", "#613583"),
    ("Cyan", "#62a0ea"),
    ("Brown", "#986a44"),
];

const ICON_OPTIONS: &[(&str, &str)] = &[
    ("Folder", "folder-symbolic"),
    ("Terminal", "utilities-terminal-symbolic"),
    ("Graphics", "applications-graphics-symbolic"),
    ("Home", "user-home-symbolic"),
    ("Document", "x-office-document-symbolic"),
    ("Star", "starred-symbolic"),
    ("Grid", "view-grid-symbolic"),
    ("Bookmarks", "user-bookmarks-symbolic"),
];

#[derive(Debug, Clone)]
pub enum SpaceDialogResult {
    Create {
        name: String,
        color: String,
        icon: String,
    },
    Update {
        id: i64,
        name: String,
        color: String,
        icon: String,
    },
    Delete {
        id: i64,
        delete_clips: bool,
    },
}

/// Show dialog to create a new Space or edit/delete an existing Space.
pub fn show_space_editor(
    app: &Application,
    existing: Option<&SpaceRecord>,
    on_submit: Rc<dyn Fn(SpaceDialogResult)>,
) {
    let title = if existing.is_some() {
        "Edit Space"
    } else {
        "New Space"
    };
    let win = Window::builder()
        .application(app)
        .title(title)
        .default_width(380)
        .resizable(false)
        .build();

    let vbox = GtkBox::new(Orientation::Vertical, 10);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);

    let name_entry = Entry::new();
    name_entry.set_placeholder_text(Some("Space name (e.g. Work, Snippets)"));
    if let Some(sp) = existing {
        name_entry.set_text(&sp.name);
    }
    vbox.append(&Label::builder().label("Name").xalign(0.0).build());
    vbox.append(&name_entry);

    let color_names: Vec<&str> = COLOR_PALETTE.iter().map(|(n, _)| *n).collect();
    let color_dd = DropDown::new(Some(StringList::new(&color_names)), gtk4::Expression::NONE);
    if let Some(sp) = existing
        && let Some(idx) = COLOR_PALETTE
            .iter()
            .position(|(_, hex)| hex.eq_ignore_ascii_case(&sp.color))
    {
        color_dd.set_selected(idx as u32);
    }
    vbox.append(&Label::builder().label("Color").xalign(0.0).build());
    vbox.append(&color_dd);

    let icon_names: Vec<&str> = ICON_OPTIONS.iter().map(|(n, _)| *n).collect();
    let icon_dd = DropDown::new(Some(StringList::new(&icon_names)), gtk4::Expression::NONE);
    if let Some(sp) = existing
        && let Some(idx) = ICON_OPTIONS.iter().position(|(_, ic)| *ic == sp.icon)
    {
        icon_dd.set_selected(idx as u32);
    }
    vbox.append(&Label::builder().label("Icon").xalign(0.0).build());
    vbox.append(&icon_dd);

    let delete_clips_check = CheckButton::with_label("Also delete clips in this Space (otherwise move to General)");
    if let Some(sp) = existing
        && !sp.is_default
    {
        vbox.append(&Separator::new(Orientation::Horizontal));
        vbox.append(&delete_clips_check);
    }

    vbox.append(&Separator::new(Orientation::Horizontal));

    let btn_row = GtkBox::new(Orientation::Horizontal, 8);
    if let Some(sp) = existing
        && !sp.is_default
    {
        let del_btn = Button::with_label("Delete Space");
        del_btn.add_css_class("destructive-action");
        let sp_id = sp.id;
        let chk = delete_clips_check.clone();
        let act = Rc::clone(&on_submit);
        let win_weak = win.downgrade();
        del_btn.connect_clicked(move |_| {
            act(SpaceDialogResult::Delete {
                id: sp_id,
                delete_clips: chk.is_active(),
            });
            if let Some(w) = win_weak.upgrade() {
                w.close();
            }
        });
        btn_row.append(&del_btn);
    }

    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    btn_row.append(&spacer);

    let cancel_btn = Button::with_label("Cancel");
    {
        let win_weak = win.downgrade();
        cancel_btn.connect_clicked(move |_| {
            if let Some(w) = win_weak.upgrade() {
                w.close();
            }
        });
    }
    let save_btn = Button::with_label(if existing.is_some() { "Save" } else { "Create" });
    save_btn.add_css_class("suggested-action");
    {
        let existing_id = existing.map(|s| s.id);
        let entry = name_entry.clone();
        let cdd = color_dd.clone();
        let idd = icon_dd.clone();
        let act = Rc::clone(&on_submit);
        let win_weak = win.downgrade();
        save_btn.connect_clicked(move |_| {
            let name = entry.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            let color = COLOR_PALETTE
                .get(cdd.selected() as usize)
                .map(|(_, h)| (*h).to_string())
                .unwrap_or_else(|| "#8a8f98".to_string());
            let icon = ICON_OPTIONS
                .get(idd.selected() as usize)
                .map(|(_, ic)| (*ic).to_string())
                .unwrap_or_else(|| "folder-symbolic".to_string());

            if let Some(id) = existing_id {
                act(SpaceDialogResult::Update {
                    id,
                    name,
                    color,
                    icon,
                });
            } else {
                act(SpaceDialogResult::Create { name, color, icon });
            }
            if let Some(w) = win_weak.upgrade() {
                w.close();
            }
        });
    }

    btn_row.append(&cancel_btn);
    btn_row.append(&save_btn);
    vbox.append(&btn_row);

    win.set_child(Some(&vbox));
    win.present();
}

/// Show a quick Move-to-Space picker (`Ctrl+M`).
pub fn show_move_to_space_dialog(
    app: &Application,
    spaces: &[SpaceRecord],
    current_space_id: i64,
    on_pick: Rc<dyn Fn(i64)>,
) {
    let win = Window::builder()
        .application(app)
        .title("Move Clip to Space")
        .default_width(300)
        .resizable(false)
        .build();

    let vbox = GtkBox::new(Orientation::Vertical, 6);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    for sp in spaces {
        let label = if sp.id == current_space_id {
            format!("{} (current)", sp.name)
        } else {
            sp.name.clone()
        };
        let btn = Button::with_label(&label);
        btn.set_sensitive(sp.id != current_space_id);
        let target_id = sp.id;
        let pick = Rc::clone(&on_pick);
        let win_weak = win.downgrade();
        btn.connect_clicked(move |_| {
            pick(target_id);
            if let Some(w) = win_weak.upgrade() {
                w.close();
            }
        });
        vbox.append(&btn);
    }

    win.set_child(Some(&vbox));
    win.present();
}

/// Show a quick Rename / Label Clip dialog (`F2`).
pub fn show_rename_clip_dialog(
    app: &Application,
    current_title: Option<&str>,
    on_save: Rc<dyn Fn(Option<String>)>,
) {
    let win = Window::builder()
        .application(app)
        .title("Rename / Label Clip")
        .default_width(360)
        .resizable(false)
        .build();

    let vbox = GtkBox::new(Orientation::Vertical, 10);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);

    let entry = Entry::new();
    entry.set_placeholder_text(Some("Optional label (leave blank to clear)"));
    if let Some(t) = current_title {
        entry.set_text(t);
    }
    vbox.append(&entry);

    let btn_row = GtkBox::new(Orientation::Horizontal, 8);
    btn_row.set_halign(Align::End);
    let cancel_btn = Button::with_label("Cancel");
    {
        let win_weak = win.downgrade();
        cancel_btn.connect_clicked(move |_| {
            if let Some(w) = win_weak.upgrade() {
                w.close();
            }
        });
    }
    let save_btn = Button::with_label("Save");
    save_btn.add_css_class("suggested-action");
    {
        let e = entry.clone();
        let act = Rc::clone(&on_save);
        let win_weak = win.downgrade();
        save_btn.connect_clicked(move |_| {
            let txt = e.text().trim().to_string();
            act(if txt.is_empty() { None } else { Some(txt) });
            if let Some(w) = win_weak.upgrade() {
                w.close();
            }
        });
    }
    btn_row.append(&cancel_btn);
    btn_row.append(&save_btn);
    vbox.append(&btn_row);

    win.set_child(Some(&vbox));
    win.present();
}
