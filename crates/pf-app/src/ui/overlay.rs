//! Floating PasteFlow overlay window (520×420, undecorated, positioned at pointer on X11).

use crate::ui::{
    ALL_CARD_CLASSES, card_css_class, format_relative_time, kind_display_name, kind_icon_name,
};
use gdk4_x11::X11Surface;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, Box as GtkBox, Button, DrawingArea, EventControllerKey, Image, Label,
    ListItem, ListView, Orientation, PolicyType, ScrolledWindow, SearchEntry,
    SignalListItemFactory, SingleSelection, Window, gdk, pango,
};
use pf_core::model::{ClipFilter, ClipKind, ClipRecord, now_unix_ms};
use pf_store::Store;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const OVERLAY_WIDTH: i32 = 460;
pub const OVERLAY_HEIGHT: i32 = 520;

const FILTER_CYCLE: [(Option<ClipKind>, &str, &str); 6] = [
    (None, "All", "pf-dot-all"),
    (Some(ClipKind::Text), "Text", "pf-dot-text"),
    (Some(ClipKind::Code), "Code", "pf-dot-code"),
    (Some(ClipKind::Url), "Links", "pf-dot-url"),
    (Some(ClipKind::Image), "Images", "pf-dot-image"),
    (Some(ClipKind::Color), "Colors", "pf-dot-color"),
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
    AddToQueue(i64),
    DeleteClip(i64),
    Closed {
        previous_window: Option<u32>,
    },
}

pub struct OverlayWindow {
    window: Window,
    search_entry: SearchEntry,
    pill_buttons: Rc<Vec<Button>>,
    model: gio::ListStore,
    selection: SingleSelection,
    filter_idx: Rc<Cell<usize>>,
    previous_window: Rc<Cell<Option<u32>>>,
    last_shown_at: Rc<Cell<Instant>>,
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

        // Top pill search bar
        let top_bar = GtkBox::new(Orientation::Horizontal, 8);
        top_bar.add_css_class("pf-toolbar");

        let search_entry = SearchEntry::new();
        search_entry.set_placeholder_text(Some("Search Clipboard…"));
        search_entry.add_css_class("pf-search");
        search_entry.set_hexpand(true);
        top_bar.append(&search_entry);
        vbox.append(&top_bar);

        // Clickable filter pills row matching colors.png (All, Text, Code, Links, Images, Colors)
        let filter_bar = GtkBox::new(Orientation::Horizontal, 6);
        filter_bar.add_css_class("pf-filter-bar");

        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = SingleSelection::new(Some(model.clone()));
        let filter_idx = Rc::new(Cell::new(0usize));

        let mut pill_btns_vec = Vec::with_capacity(FILTER_CYCLE.len());
        for (idx, (_kind, label, dot_cls)) in FILTER_CYCLE.iter().enumerate() {
            let pill_content = GtkBox::new(Orientation::Horizontal, 6);
            let dot = GtkBox::new(Orientation::Horizontal, 0);
            dot.set_valign(Align::Center);
            dot.add_css_class("pf-dot");
            dot.add_css_class(dot_cls);
            let lbl = Label::new(Some(label));
            pill_content.append(&dot);
            pill_content.append(&lbl);

            let btn = Button::new();
            btn.set_child(Some(&pill_content));
            btn.add_css_class("pf-filter-pill");
            if idx == 0 {
                btn.add_css_class("pf-pill-active");
            }
            filter_bar.append(&btn);
            pill_btns_vec.push(btn);
        }
        let pill_buttons = Rc::new(pill_btns_vec);

        for (idx, btn) in pill_buttons.iter().enumerate() {
            let pills_c = Rc::clone(&pill_buttons);
            let fidx_c = Rc::clone(&filter_idx);
            let store_c = Rc::clone(&store);
            let model_c = model.clone();
            let sel_c = selection.clone();
            let search_c = search_entry.clone();
            btn.connect_clicked(move |_| {
                fidx_c.set(idx);
                update_active_pill(&pills_c, idx);
                reload_overlay_list(
                    &store_c.borrow(),
                    &model_c,
                    &sel_c,
                    FILTER_CYCLE[idx].0,
                    search_c.text().as_str(),
                );
            });
        }

        let filter_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Never)
            .child(&filter_bar)
            .build();
        vbox.append(&filter_scroll);

        // Recycled ListView of pastel cards (latest 50 clips)
        let factory = build_overlay_row_factory(thumbs_dir);
        let list_view = ListView::new(Some(selection.clone()), Some(factory));
        list_view.add_css_class("pf-card-list");
        list_view.set_single_click_activate(true);

        let previous_window = Rc::new(Cell::new(None));
        let last_shown_at = Rc::new(Cell::new(Instant::now() - Duration::from_secs(10)));

        // Clicking a row copies & auto-pastes into the previous window
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
                    act_c(OverlayAction::PasteAndClose {
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

        let footer = Label::new(Some(
            "↑↓ Navigate · Enter/Click Paste · Shift+Enter Copy · Tab Filter · Esc Close",
        ));
        footer.add_css_class("pf-status-bar");
        vbox.append(&footer);

        window.set_child(Some(&vbox));

        // Hide when window loses focus (with a 300 ms grace period after show so Muffin's
        // initial map/focus handoff never immediately hides the newly opened overlay).
        {
            let prev_c = Rc::clone(&previous_window);
            let shown_c = Rc::clone(&last_shown_at);
            let act_c = Rc::clone(&on_action);
            window.connect_is_active_notify(move |w| {
                if !w.is_active()
                    && w.is_visible()
                    && shown_c.get().elapsed() > Duration::from_millis(300)
                {
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
                    FILTER_CYCLE[fidx_c.get()].0,
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
            let pills_c = Rc::clone(&pill_buttons);
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
                    update_active_pill(&pills_c, next_idx);
                    reload_overlay_list(
                        &store_c.borrow(),
                        &model_c,
                        &sel_c,
                        FILTER_CYCLE[next_idx].0,
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
                    if key == gdk::Key::q || key == gdk::Key::Q {
                        if let Some(clip_id) = selected_overlay_id(&sel_c) {
                            act_c(OverlayAction::AddToQueue(clip_id));
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
                            act_c(OverlayAction::CopyAndClose {
                                clip_id,
                                previous_window: prev_c.get(),
                            });
                        } else {
                            act_c(OverlayAction::PasteAndClose {
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
            pill_buttons,
            model,
            selection,
            filter_idx,
            previous_window,
            last_shown_at,
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
    /// present it with the hotkey timestamp, and focus the search field immediately.
    pub fn show_and_focus(
        &self,
        previous_window: Option<u32>,
        user_time: Option<u32>,
        position_fn: impl Fn(u32),
        activate_fn: impl FnOnce(u32),
    ) {
        self.previous_window.set(previous_window);
        self.last_shown_at.set(Instant::now());
        self.filter_idx.set(0);
        update_active_pill(&self.pill_buttons, 0);
        if !self.search_entry.text().is_empty() {
            self.search_entry.set_text("");
        }
        self.refresh();

        gtk4::prelude::WidgetExt::realize(&self.window);
        if let Some(xid) = self.xid() {
            position_fn(xid);
        }

        if let Some(ts) = user_time
            && ts != 0
        {
            #[allow(deprecated)]
            self.window.present_with_time(ts);
        } else {
            self.window.present();
        }

        if let Some(xid) = self.xid() {
            position_fn(xid);
            activate_fn(xid);
        }
        self.search_entry.grab_focus();
    }

    pub fn refresh(&self) {
        reload_overlay_list(
            &self.store.borrow(),
            &self.model,
            &self.selection,
            FILTER_CYCLE[self.filter_idx.get()].0,
            self.search_entry.text().as_str(),
        );
    }
}

fn update_active_pill(pills: &[Button], active_idx: usize) {
    for (i, b) in pills.iter().enumerate() {
        if i == active_idx {
            b.add_css_class("pf-pill-active");
        } else {
            b.remove_css_class("pf-pill-active");
        }
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
    card: GtkBox,
    thumb: Image,
    swatch: DrawingArea,
    swatch_rgba: Rc<Cell<(f64, f64, f64, f64)>>,
    preview: Label,
    chip_icon: Image,
    kind_pill: Label,
    time_lbl: Label,
}

fn build_overlay_row_factory(thumbs_dir: PathBuf) -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();

    factory.connect_setup(|_f, obj| {
        let Some(item) = obj.downcast_ref::<ListItem>() else {
            return;
        };
        let card = GtkBox::new(Orientation::Horizontal, 10);
        card.add_css_class("pf-clip-row");

        let thumb = Image::new();
        thumb.set_pixel_size(42);
        thumb.set_valign(Align::Center);
        thumb.add_css_class("pf-thumb-frame");
        thumb.set_visible(false);
        card.append(&thumb);

        let col = GtkBox::new(Orientation::Vertical, 6);
        col.set_hexpand(true);

        // Top line: optional color swatch square + bold preview text
        let top_line = GtkBox::new(Orientation::Horizontal, 8);
        let swatch_rgba = Rc::new(Cell::new((0.5, 0.5, 0.5, 1.0)));
        let swatch = DrawingArea::new();
        swatch.set_content_width(18);
        swatch.set_content_height(18);
        swatch.set_valign(Align::Center);
        swatch.add_css_class("pf-color-swatch");
        swatch.set_visible(false);
        {
            let rgba_c = Rc::clone(&swatch_rgba);
            swatch.set_draw_func(move |_a, cr, w, h| {
                let (r, g, b, a) = rgba_c.get();
                cr.set_source_rgba(r, g, b, a);
                cr.rectangle(0.0, 0.0, f64::from(w), f64::from(h));
                let _ = cr.fill();
            });
        }
        top_line.append(&swatch);

        let preview = Label::new(None);
        preview.set_xalign(0.0);
        preview.set_lines(1);
        preview.set_single_line_mode(true);
        preview.set_ellipsize(pango::EllipsizeMode::End);
        preview.add_css_class("pf-clip-preview");
        preview.set_hexpand(true);
        top_line.append(&preview);
        col.append(&top_line);

        // Bottom metadata line: rounded icon chip + category pill + relative timestamp
        let meta_line = GtkBox::new(Orientation::Horizontal, 6);
        let chip_box = GtkBox::new(Orientation::Horizontal, 0);
        chip_box.add_css_class("pf-icon-chip");
        let chip_icon = Image::new();
        chip_icon.set_pixel_size(13);
        chip_box.append(&chip_icon);
        meta_line.append(&chip_box);

        let kind_pill = Label::new(None);
        kind_pill.add_css_class("pf-kind-badge");
        meta_line.append(&kind_pill);

        let time_lbl = Label::new(None);
        time_lbl.set_xalign(0.0);
        time_lbl.add_css_class("pf-clip-meta");
        meta_line.append(&time_lbl);
        col.append(&meta_line);

        card.append(&col);

        let widgets = Rc::new(OverlayRowWidgets {
            card: card.clone(),
            thumb,
            swatch,
            swatch_rgba,
            preview,
            chip_icon,
            kind_pill,
            time_lbl,
        });
        // SAFETY: Unique qdata key for recycled overlay row widgets.
        unsafe {
            card.set_data("pf-overlay-widgets", widgets);
        }
        item.set_child(Some(&card));
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

        for cls in ALL_CARD_CLASSES {
            w.card.remove_css_class(cls);
        }
        w.card.add_css_class(card_css_class(clip.kind, clip.id < 0));

        let display_text = if let Some(ref title) = clip.title {
            title.clone()
        } else {
            clip.preview.replace('\n', " ↵ ")
        };
        w.preview.set_text(&display_text);
        w.chip_icon.set_icon_name(Some(kind_icon_name(clip.kind)));
        w.kind_pill.set_text(kind_display_name(clip.kind));

        let pin = if clip.pinned { "★ · " } else { "" };
        let rel = format_relative_time(clip.last_used_at, now_unix_ms());
        w.time_lbl.set_text(&format!("{pin}{rel}"));

        // Live color swatch for Color clips
        if clip.kind == ClipKind::Color
            && let Some(col) = pf_core::detect::color::parse_color(&clip.preview)
        {
            w.swatch_rgba.set((
                f64::from(col.r) / 255.0,
                f64::from(col.g) / 255.0,
                f64::from(col.b) / 255.0,
                f64::from(col.a) / 255.0,
            ));
            w.swatch.set_visible(true);
            w.swatch.queue_draw();
        } else {
            w.swatch.set_visible(false);
        }

        // Image thumbnail only for Image clips (matching colors.png)
        if clip.kind == ClipKind::Image
            && let Some(ref im) = clip.image
            && let Some(ref tkey) = im.thumb_key
        {
            let path = thumbs_dir.join(tkey);
            if path.exists() {
                w.thumb.set_from_file(Some(&path));
                w.thumb.set_visible(true);
                return;
            }
        }
        w.thumb.set_visible(false);
    });

    factory
}
