//! Floating PasteFlow overlay window (520×420, undecorated, positioned at pointer on X11).

use crate::ui::format_relative_time;
use gdk4_x11::X11Surface;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, Box as GtkBox, EventControllerKey, Image, Label, ListItem, ListView,
    Orientation, PolicyType, ScrolledWindow, SearchEntry, Separator, SignalListItemFactory,
    SingleSelection, Window, gdk, pango,
};
use pf_core::model::{ClipFilter, ClipKind, ClipRecord, now_unix_ms};
use pf_store::Store;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

pub const OVERLAY_WIDTH: i32 = 520;
pub const OVERLAY_HEIGHT: i32 = 420;

const FILTER_CYCLE: [Option<ClipKind>; 6] = [
    None,
    Some(ClipKind::Text),
    Some(ClipKind::Code),
    Some(ClipKind::Url),
    Some(ClipKind::Image),
    Some(ClipKind::Files),
];

#[derive(Debug, Clone)]
pub enum OverlayAction {
    /// Copy clip, hide overlay, restore previous window focus.
    CopyAndClose {
        clip_id: i64,
        previous_window: Option<u32>,
    },
    /// Copy clip, hide overlay, restore previous window focus, and inject synthetic paste.
    PasteAndClose {
        clip_id: i64,
        previous_window: Option<u32>,
    },
    TogglePin(i64),
    DeleteClip(i64),
    Closed {
        previous_window: Option<u32>,
    },
}

pub struct OverlayWindow {
    window: Window,
    search_entry: SearchEntry,
    filter_badge: Label,
    model: gio::ListStore,
    selection: SingleSelection,
    filter_idx: Rc<Cell<usize>>,
    previous_window: Rc<Cell<Option<u32>>>,
    store: Rc<RefCell<Store>>,
}

impl OverlayWindow {
    pub fn new(
        app: &Application,
        store: Rc<RefCell<Store>>,
        thumbs_dir: PathBuf,
        on_action: Rc<dyn Fn(OverlayAction)>,
    ) -> Self {
        let window = Window::builder()
            .application(app)
            .title("PasteFlow Quick Picker")
            .decorated(false)
            .resizable(false)
            .default_width(OVERLAY_WIDTH)
            .default_height(OVERLAY_HEIGHT)
            .build();
        window.add_css_class("pf-overlay");

        let vbox = GtkBox::new(Orientation::Vertical, 0);

        // Top search bar + active type filter pill
        let top_bar = GtkBox::new(Orientation::Horizontal, 8);
        top_bar.add_css_class("pf-toolbar");

        let search_entry = SearchEntry::new();
        search_entry.set_placeholder_text(Some("Type to search… (Enter copy · Shift+Enter paste)"));
        search_entry.set_hexpand(true);
        top_bar.append(&search_entry);

        let filter_badge = Label::new(Some("ALL"));
        filter_badge.add_css_class("pf-kind-badge");
        filter_badge.set_tooltip_text(Some("Press Tab to cycle type filter"));
        top_bar.append(&filter_badge);

        vbox.append(&top_bar);
        vbox.append(&Separator::new(Orientation::Horizontal));

        // Recycled ListView (latest 50 clips)
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = SingleSelection::new(Some(model.clone()));
        let factory = build_overlay_row_factory(thumbs_dir);
        let list_view = ListView::new(Some(selection.clone()), Some(factory));
        list_view.set_single_click_activate(true);

        let previous_window = Rc::new(Cell::new(None));
        let filter_idx = Rc::new(Cell::new(0usize));

        // Clicking a row copies & closes
        {
            let sel_c = selection.clone();
            let prev_c = Rc::clone(&previous_window);
            let act_c = Rc::clone(&on_action);
            let win_weak = window.downgrade();
            list_view.connect_activate(move |_lv, pos| {
                if let Some(obj) = sel_c.item(pos)
                    && let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>()
                {
                    let clip_id = boxed.borrow::<ClipRecord>().id;
                    if let Some(w) = win_weak.upgrade() {
                        w.set_visible(false);
                    }
                    act_c(OverlayAction::CopyAndClose {
                        clip_id,
                        previous_window: prev_c.get(),
                    });
                }
            });
        }

        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .child(&list_view)
            .build();
        vbox.append(&scrolled);

        vbox.append(&Separator::new(Orientation::Horizontal));
        let footer = Label::new(Some(
            "↑↓ Navigate · Enter Copy · Shift+Enter Paste · Ctrl+1-9 Quick · Tab Filter · Esc Close",
        ));
        footer.add_css_class("pf-clip-meta");
        footer.set_margin_top(4);
        footer.set_margin_bottom(4);
        vbox.append(&footer);

        window.set_child(Some(&vbox));

        // Hide when window loses focus
        {
            let prev_c = Rc::clone(&previous_window);
            let act_c = Rc::clone(&on_action);
            window.connect_is_active_notify(move |w| {
                if !w.is_active() && w.is_visible() {
                    w.set_visible(false);
                    act_c(OverlayAction::Closed {
                        previous_window: prev_c.get(),
                    });
                }
            });
        }

        // Prevent close from destroying the cached overlay window
        window.connect_close_request(|w| {
            w.set_visible(false);
            glib::Propagation::Stop
        });

        // Search live refresh
        {
            let model_c = model.clone();
            let sel_c = selection.clone();
            let store_c = Rc::clone(&store);
            let fidx_c = Rc::clone(&filter_idx);
            search_entry.connect_search_changed(move |entry| {
                reload_overlay_list(
                    &store_c.borrow(),
                    &model_c,
                    &sel_c,
                    FILTER_CYCLE[fidx_c.get()],
                    entry.text().as_str(),
                );
            });
        }

        // Capture-phase keyboard controller so arrows/Enter/Tab/Ctrl+1..9 work even while search has focus
        {
            let key_ctrl = EventControllerKey::new();
            key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let sel_c = selection.clone();
            let model_c = model.clone();
            let search_c = search_entry.clone();
            let badge_c = filter_badge.clone();
            let fidx_c = Rc::clone(&filter_idx);
            let prev_c = Rc::clone(&previous_window);
            let store_c = Rc::clone(&store);
            let act_c = Rc::clone(&on_action);
            let win_weak = window.downgrade();

            key_ctrl.connect_key_pressed(move |_ctrl, key, _code, mods| {
                let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
                let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);

                if key == gdk::Key::Escape {
                    if let Some(w) = win_weak.upgrade() {
                        w.set_visible(false);
                    }
                    act_c(OverlayAction::Closed {
                        previous_window: prev_c.get(),
                    });
                    return glib::Propagation::Stop;
                }

                if key == gdk::Key::Down {
                    let n = model_c.n_items();
                    if n > 0 {
                        let cur = sel_c.selected();
                        let next = if cur + 1 < n { cur + 1 } else { 0 };
                        sel_c.set_selected(next);
                    }
                    return glib::Propagation::Stop;
                }

                if key == gdk::Key::Up {
                    let n = model_c.n_items();
                    if n > 0 {
                        let cur = sel_c.selected();
                        let next = if cur == 0 { n - 1 } else { cur - 1 };
                        sel_c.set_selected(next);
                    }
                    return glib::Propagation::Stop;
                }

                if key == gdk::Key::Tab || key == gdk::Key::ISO_Left_Tab {
                    let next_idx = (fidx_c.get() + 1) % FILTER_CYCLE.len();
                    fidx_c.set(next_idx);
                    let kind = FILTER_CYCLE[next_idx];
                    badge_c.set_text(kind.map(|k| k.as_str()).unwrap_or("all").to_ascii_uppercase().as_str());
                    reload_overlay_list(
                        &store_c.borrow(),
                        &model_c,
                        &sel_c,
                        kind,
                        search_c.text().as_str(),
                    );
                    return glib::Propagation::Stop;
                }

                // Ctrl+1..9 quick pick
                if ctrl {
                    if let Some(digit) = key_to_digit(key) {
                        let idx = digit - 1;
                        if idx < model_c.n_items()
                            && let Some(obj) = model_c.item(idx)
                            && let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>()
                        {
                            let clip_id = boxed.borrow::<ClipRecord>().id;
                            if let Some(w) = win_weak.upgrade() {
                                w.set_visible(false);
                            }
                            if shift {
                                act_c(OverlayAction::PasteAndClose {
                                    clip_id,
                                    previous_window: prev_c.get(),
                                });
                            } else {
                                act_c(OverlayAction::CopyAndClose {
                                    clip_id,
                                    previous_window: prev_c.get(),
                                });
                            }
                            return glib::Propagation::Stop;
                        }
                    }
                    if key == gdk::Key::p || key == gdk::Key::P {
                        if let Some(clip_id) = selected_overlay_id(&sel_c) {
                            act_c(OverlayAction::TogglePin(clip_id));
                            return glib::Propagation::Stop;
                        }
                    }
                }

                if key == gdk::Key::Return || key == gdk::Key::KP_Enter {
                    if let Some(clip_id) = selected_overlay_id(&sel_c) {
                        if let Some(w) = win_weak.upgrade() {
                            w.set_visible(false);
                        }
                        if shift {
                            act_c(OverlayAction::PasteAndClose {
                                clip_id,
                                previous_window: prev_c.get(),
                            });
                        } else {
                            act_c(OverlayAction::CopyAndClose {
                                clip_id,
                                previous_window: prev_c.get(),
                            });
                        }
                        return glib::Propagation::Stop;
                    }
                }

                if key == gdk::Key::Delete && search_c.text().is_empty() {
                    if let Some(clip_id) = selected_overlay_id(&sel_c) {
                        act_c(OverlayAction::DeleteClip(clip_id));
                        return glib::Propagation::Stop;
                    }
                }

                glib::Propagation::Proceed
            });
            window.add_controller(key_ctrl);
        }

        Self {
            window,
            search_entry,
            filter_badge,
            model,
            selection,
            filter_idx,
            previous_window,
            store,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    /// Return the underlying X11 window ID once realized.
    pub fn xid(&self) -> Option<u32> {
        let surface = self.window.surface()?;
        let x11 = surface.downcast_ref::<X11Surface>()?;
        Some(x11.xid() as u32)
    }

    /// Prepare the overlay (reset search & filter, reload latest 50 clips),
    /// present it, and focus the search field immediately.
    pub fn show_and_focus(&self, previous_window: Option<u32>, position_fn: impl FnOnce(u32)) {
        self.previous_window.set(previous_window);
        self.filter_idx.set(0);
        self.filter_badge.set_text("ALL");
        if !self.search_entry.text().is_empty() {
            self.search_entry.set_text("");
        }
        self.refresh();

        gtk4::prelude::WidgetExt::realize(&self.window);
        if let Some(xid) = self.xid() {
            position_fn(xid);
        }
        self.window.present();
        self.search_entry.grab_focus();
    }

    pub fn refresh(&self) {
        reload_overlay_list(
            &self.store.borrow(),
            &self.model,
            &self.selection,
            FILTER_CYCLE[self.filter_idx.get()],
            self.search_entry.text().as_str(),
        );
    }
}

fn selected_overlay_id(selection: &SingleSelection) -> Option<i64> {
    let obj = selection.selected_item()?;
    let boxed = obj.downcast::<glib::BoxedAnyObject>().ok()?;
    let id = boxed.borrow::<ClipRecord>().id;
    Some(id)
}

fn key_to_digit(key: gdk::Key) -> Option<u32> {
    match key {
        gdk::Key::_1 | gdk::Key::KP_1 => Some(1),
        gdk::Key::_2 | gdk::Key::KP_2 => Some(2),
        gdk::Key::_3 | gdk::Key::KP_3 => Some(3),
        gdk::Key::_4 | gdk::Key::KP_4 => Some(4),
        gdk::Key::_5 | gdk::Key::KP_5 => Some(5),
        gdk::Key::_6 | gdk::Key::KP_6 => Some(6),
        gdk::Key::_7 | gdk::Key::KP_7 => Some(7),
        gdk::Key::_8 | gdk::Key::KP_8 => Some(8),
        gdk::Key::_9 | gdk::Key::KP_9 => Some(9),
        _ => None,
    }
}

fn reload_overlay_list(
    store: &Store,
    model: &gio::ListStore,
    selection: &SingleSelection,
    kind: Option<ClipKind>,
    query: &str,
) {
    let filter = ClipFilter {
        query: if query.trim().is_empty() {
            None
        } else {
            Some(query.to_string())
        },
        kind,
        limit: 50,
        ..Default::default()
    };
    let clips = store.list_clips(&filter).unwrap_or_default();
    let objects: Vec<glib::BoxedAnyObject> = clips
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect();
    model.splice(0, model.n_items(), &objects);
    if model.n_items() > 0 {
        selection.set_selected(0);
    }
}

struct OverlayRowWidgets {
    index_lbl: Label,
    thumb: Image,
    preview: Label,
    meta: Label,
}

fn build_overlay_row_factory(thumbs_dir: PathBuf) -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();

    factory.connect_setup(|_f, obj| {
        let Some(item) = obj.downcast_ref::<ListItem>() else {
            return;
        };
        let hbox = GtkBox::new(Orientation::Horizontal, 8);
        hbox.add_css_class("pf-clip-row");

        let index_lbl = Label::new(None);
        index_lbl.add_css_class("pf-clip-meta");
        index_lbl.set_width_chars(2);
        index_lbl.set_valign(Align::Center);
        hbox.append(&index_lbl);

        let thumb = Image::new();
        thumb.set_pixel_size(28);
        thumb.set_valign(Align::Center);
        hbox.append(&thumb);

        let col = GtkBox::new(Orientation::Vertical, 2);
        col.set_hexpand(true);

        let preview = Label::new(None);
        preview.set_xalign(0.0);
        preview.set_lines(1);
        preview.set_single_line_mode(true);
        preview.set_ellipsize(pango::EllipsizeMode::End);
        preview.add_css_class("pf-clip-preview");
        col.append(&preview);

        let meta = Label::new(None);
        meta.set_xalign(0.0);
        meta.add_css_class("pf-clip-meta");
        col.append(&meta);

        hbox.append(&col);

        let widgets = Rc::new(OverlayRowWidgets {
            index_lbl,
            thumb,
            preview,
            meta,
        });
        // SAFETY: Unique qdata key for recycled overlay row widgets.
        unsafe {
            hbox.set_data("pf-overlay-widgets", widgets);
        }
        item.set_child(Some(&hbox));
    });

    factory.connect_bind(move |_f, obj| {
        let Some(item) = obj.downcast_ref::<ListItem>() else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|o| o.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let Some(child) = item.child() else {
            return;
        };
        let clip = boxed.borrow::<ClipRecord>();
        // SAFETY: Same key and type set in `connect_setup`.
        let widgets_opt: Option<Rc<OverlayRowWidgets>> = unsafe {
            child
                .data::<Rc<OverlayRowWidgets>>("pf-overlay-widgets")
                .map(|p| Rc::clone(p.as_ref()))
        };
        let Some(w) = widgets_opt else {
            return;
        };

        let pos = item.position();
        if pos < 9 {
            w.index_lbl.set_text(&format!("{}", pos + 1));
        } else {
            w.index_lbl.set_text("");
        }

        let one_line = clip.preview.replace('\n', " ↵ ");
        w.preview.set_text(&one_line);

        let pin = if clip.pinned { "★ · " } else { "" };
        let rel = format_relative_time(clip.last_used_at, now_unix_ms());
        w.meta.set_text(&format!(
            "{pin}{} · {rel}",
            clip.kind.as_str().to_ascii_uppercase()
        ));

        if clip.kind == ClipKind::Image
            && let Some(ref im) = clip.image
            && let Some(ref tkey) = im.thumb_key
        {
            let path = thumbs_dir.join(tkey);
            if path.exists() {
                w.thumb.set_from_file(Some(&path));
                return;
            }
        }
        let icon = match clip.kind {
            ClipKind::Image => "image-x-generic-symbolic",
            ClipKind::Files => "folder-documents-symbolic",
            ClipKind::Code => "utilities-terminal-symbolic",
            ClipKind::Url => "insert-link-symbolic",
            ClipKind::Color => "applications-graphics-symbolic",
            _ => "text-x-generic-symbolic",
        };
        w.thumb.set_icon_name(Some(icon));
    });

    factory
}
