use crate::ui::{format_bytes, format_relative_time};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerKey, Image, Label,
    ListBox, ListBoxRow, ListItem, ListView, Orientation, PolicyType, ScrolledWindow, SearchEntry,
    Separator, SignalListItemFactory, SingleSelection, Stack, ToggleButton, gdk, pango,
};
use pf_core::model::{ClipFilter, ClipKind, ClipRecord, now_unix_ms};
use pf_store::Store;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

/// Actions triggered by user interaction inside the Main Window.
#[derive(Debug, Clone)]
pub enum UiAction {
    CopyClip(i64),
    DeleteClip(i64),
    TogglePin(i64),
    TogglePause,
    ClearUnpinned,
    OpenShortcuts,
}

#[derive(Debug, Clone, Default)]
struct NavSelection {
    pinned_only: bool,
    kind: Option<ClipKind>,
    space_id: Option<i64>,
}

/// Main PasteFlow application window (1000×640, plain GTK4 following Mint-Y).
pub struct MainWindow {
    window: ApplicationWindow,
    model: gio::ListStore,
    selection: SingleSelection,
    search_entry: SearchEntry,
    stack: Stack,
    paused_banner: Label,
    pause_btn: ToggleButton,
    status_label: Label,
    nav_state: Rc<RefCell<NavSelection>>,
    store: Rc<RefCell<Store>>,
}

impl MainWindow {
    pub fn new(
        app: &Application,
        store: Rc<RefCell<Store>>,
        thumbs_dir: PathBuf,
        initial_paused: bool,
        on_action: Rc<dyn Fn(UiAction)>,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("PasteFlow")
            .default_width(1000)
            .default_height(640)
            .build();

        // Close button hides to tray instead of quitting the resident daemon.
        window.connect_close_request(|w| {
            w.set_visible(false);
            glib::Propagation::Stop
        });

        let root_hbox = GtkBox::new(Orientation::Horizontal, 0);

        // --- Sidebar (220 px) ---
        let sidebar_box = GtkBox::new(Orientation::Vertical, 4);
        sidebar_box.set_width_request(220);
        sidebar_box.add_css_class("pf-sidebar");

        let brand = Label::new(Some("PasteFlow"));
        brand.set_xalign(0.0);
        brand.add_css_class("pf-brand");
        sidebar_box.append(&brand);

        let nav_list = ListBox::new();
        nav_list.set_selection_mode(gtk4::SelectionMode::Single);
        nav_list.add_css_class("navigation-sidebar");

        let mut nav_targets: Vec<NavSelection> = Vec::new();
        let add_nav_row =
            |list: &ListBox, targets: &mut Vec<NavSelection>, icon: &str, label: &str, nav: NavSelection| {
                let h = GtkBox::new(Orientation::Horizontal, 8);
                h.set_margin_top(4);
                h.set_margin_bottom(4);
                h.set_margin_start(6);
                h.set_margin_end(6);
                let img = Image::from_icon_name(icon);
                let lbl = Label::new(Some(label));
                lbl.set_xalign(0.0);
                h.append(&img);
                h.append(&lbl);
                let row = ListBoxRow::new();
                row.set_child(Some(&h));
                list.append(&row);
                targets.push(nav);
            };

        add_nav_row(
            &nav_list,
            &mut nav_targets,
            "edit-paste-symbolic",
            "All Clips",
            NavSelection::default(),
        );
        add_nav_row(
            &nav_list,
            &mut nav_targets,
            "starred-symbolic",
            "Pinned",
            NavSelection {
                pinned_only: true,
                ..Default::default()
            },
        );

        let add_section_header = |list: &ListBox, targets: &mut Vec<NavSelection>, title: &str| {
            let lbl = Label::new(Some(title));
            lbl.set_xalign(0.0);
            lbl.add_css_class("pf-section-label");
            let row = ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            row.set_child(Some(&lbl));
            list.append(&row);
            targets.push(NavSelection::default());
        };

        add_section_header(&nav_list, &mut nav_targets, "COLLECTIONS");
        for (icon, name, kind) in [
            ("text-x-generic-symbolic", "Text", ClipKind::Text),
            ("utilities-terminal-symbolic", "Code", ClipKind::Code),
            ("insert-link-symbolic", "Links", ClipKind::Url),
            ("applications-graphics-symbolic", "Colors", ClipKind::Color),
            ("image-x-generic-symbolic", "Images", ClipKind::Image),
            ("folder-documents-symbolic", "Files", ClipKind::Files),
        ] {
            add_nav_row(
                &nav_list,
                &mut nav_targets,
                icon,
                name,
                NavSelection {
                    kind: Some(kind),
                    ..Default::default()
                },
            );
        }

        add_section_header(&nav_list, &mut nav_targets, "SPACES");
        if let Ok(spaces) = store.borrow().list_spaces() {
            for sp in spaces {
                add_nav_row(
                    &nav_list,
                    &mut nav_targets,
                    &sp.icon,
                    &sp.name,
                    NavSelection {
                        space_id: Some(sp.id),
                        ..Default::default()
                    },
                );
            }
        }

        if let Some(first) = nav_list.row_at_index(0) {
            nav_list.select_row(Some(&first));
        }

        let nav_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vexpand(true)
            .child(&nav_list)
            .build();
        sidebar_box.append(&nav_scroll);

        root_hbox.append(&sidebar_box);
        root_hbox.append(&Separator::new(Orientation::Vertical));

        // --- Right main content pane ---
        let content_vbox = GtkBox::new(Orientation::Vertical, 0);
        content_vbox.set_hexpand(true);
        content_vbox.set_vexpand(true);

        // Toolbar: Search + Pause + Clear
        let toolbar = GtkBox::new(Orientation::Horizontal, 8);
        toolbar.add_css_class("pf-toolbar");

        let search_entry = SearchEntry::new();
        search_entry.set_placeholder_text(Some("Search clipboard history… (Ctrl+F)"));
        search_entry.set_hexpand(true);
        toolbar.append(&search_entry);

        let pause_btn = ToggleButton::builder()
            .icon_name("media-playback-pause-symbolic")
            .tooltip_text("Pause clipboard capture")
            .active(initial_paused)
            .build();
        toolbar.append(&pause_btn);

        let clear_btn = Button::builder()
            .icon_name("edit-clear-all-symbolic")
            .tooltip_text("Clear unpinned history")
            .build();
        toolbar.append(&clear_btn);

        let shortcuts_btn = Button::builder()
            .icon_name("preferences-desktop-keyboard-shortcuts-symbolic")
            .tooltip_text("Global shortcuts")
            .build();
        toolbar.append(&shortcuts_btn);

        content_vbox.append(&toolbar);

        let paused_banner = Label::new(Some("Clipboard capture is paused"));
        paused_banner.add_css_class("pf-paused-banner");
        paused_banner.set_visible(initial_paused);
        content_vbox.append(&paused_banner);
        content_vbox.append(&Separator::new(Orientation::Horizontal));

        // Clip ListView (recycled rows)
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = SingleSelection::new(Some(model.clone()));
        let factory = build_row_factory(thumbs_dir, Rc::clone(&on_action));
        let list_view = ListView::new(Some(selection.clone()), Some(factory));
        list_view.set_single_click_activate(false);

        // Double-click / Enter on a row copies it back to CLIPBOARD
        {
            let sel_clone = selection.clone();
            let act_clone = Rc::clone(&on_action);
            list_view.connect_activate(move |_lv, pos| {
                if let Some(obj) = sel_clone.item(pos)
                    && let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>()
                {
                    let id = boxed.borrow::<ClipRecord>().id;
                    act_clone(UiAction::CopyClip(id));
                }
            });
        }

        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .child(&list_view)
            .build();

        let empty_label = Label::new(Some("No clips yet. Copy something to see it here."));
        empty_label.add_css_class("pf-empty-state");
        empty_label.set_valign(Align::Center);
        empty_label.set_halign(Align::Center);

        let stack = Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        stack.add_named(&scrolled, Some("list"));
        stack.add_named(&empty_label, Some("empty"));
        content_vbox.append(&stack);

        // Bottom status bar
        content_vbox.append(&Separator::new(Orientation::Horizontal));
        let status_label = Label::new(Some("0 clips"));
        status_label.set_xalign(0.0);
        status_label.set_margin_start(12);
        status_label.set_margin_top(4);
        status_label.set_margin_bottom(4);
        status_label.add_css_class("pf-clip-meta");
        content_vbox.append(&status_label);

        root_hbox.append(&content_vbox);
        window.set_child(Some(&root_hbox));

        let nav_state = Rc::new(RefCell::new(NavSelection::default()));

        let main_win = Self {
            window,
            model,
            selection,
            search_entry,
            stack,
            paused_banner,
            pause_btn,
            status_label,
            nav_state,
            store,
        };

        // Wire toolbar signals
        {
            let act = Rc::clone(&on_action);
            main_win.pause_btn.connect_toggled(move |_| {
                act(UiAction::TogglePause);
            });
        }
        {
            let act = Rc::clone(&on_action);
            clear_btn.connect_clicked(move |_| {
                act(UiAction::ClearUnpinned);
            });
        }
        {
            let act = Rc::clone(&on_action);
            shortcuts_btn.connect_clicked(move |_| {
                act(UiAction::OpenShortcuts);
            });
        }

        // Wire search live refresh
        {
            let model_c = main_win.model.clone();
            let stack_c = main_win.stack.clone();
            let status_c = main_win.status_label.clone();
            let nav_c = Rc::clone(&main_win.nav_state);
            let store_c = Rc::clone(&main_win.store);
            main_win.search_entry.connect_search_changed(move |entry| {
                reload_list(
                    &store_c.borrow(),
                    &model_c,
                    &stack_c,
                    &status_c,
                    &nav_c.borrow(),
                    entry.text().as_str(),
                );
            });
        }

        // Wire sidebar selection
        {
            let model_c = main_win.model.clone();
            let stack_c = main_win.stack.clone();
            let status_c = main_win.status_label.clone();
            let nav_c = Rc::clone(&main_win.nav_state);
            let store_c = Rc::clone(&main_win.store);
            let search_c = main_win.search_entry.clone();
            let targets = Rc::new(nav_targets);
            nav_list.connect_row_selected(move |_lb, row_opt| {
                let Some(row) = row_opt else { return };
                let idx = row.index();
                if idx < 0 {
                    return;
                }
                if let Some(target) = targets.get(idx as usize) {
                    *nav_c.borrow_mut() = target.clone();
                    reload_list(
                        &store_c.borrow(),
                        &model_c,
                        &stack_c,
                        &status_c,
                        &nav_c.borrow(),
                        search_c.text().as_str(),
                    );
                }
            });
        }

        // Wire window keyboard shortcuts (Ctrl+F, Esc, Delete, Ctrl+P, Enter)
        {
            let key_ctrl = EventControllerKey::new();
            let search_c = main_win.search_entry.clone();
            let sel_c = main_win.selection.clone();
            let win_weak = main_win.window.downgrade();
            let act_c = Rc::clone(&on_action);

            key_ctrl.connect_key_pressed(move |_ctrl, key, _code, mods| {
                let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
                if ctrl && (key == gdk::Key::f || key == gdk::Key::F) {
                    search_c.grab_focus();
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Escape {
                    if !search_c.text().is_empty() {
                        search_c.set_text("");
                    } else if let Some(w) = win_weak.upgrade() {
                        w.set_visible(false);
                    }
                    return glib::Propagation::Stop;
                }
                if let Some(id) = selected_clip_id(&sel_c) {
                    if ctrl && (key == gdk::Key::p || key == gdk::Key::P) {
                        act_c(UiAction::TogglePin(id));
                        return glib::Propagation::Stop;
                    }
                    if key == gdk::Key::Delete && !search_c.has_focus() {
                        act_c(UiAction::DeleteClip(id));
                        return glib::Propagation::Stop;
                    }
                    if (key == gdk::Key::Return || key == gdk::Key::KP_Enter)
                        && !search_c.has_focus()
                    {
                        act_c(UiAction::CopyClip(id));
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            });
            main_win.window.add_controller(key_ctrl);
        }

        main_win.refresh();
        main_win
    }

    pub fn window(&self) -> &ApplicationWindow {
        &self.window
    }

    pub fn present(&self) {
        self.refresh();
        self.window.present();
    }

    pub fn toggle(&self) {
        if self.window.is_visible() {
            self.window.set_visible(false);
        } else {
            self.present();
        }
    }

    pub fn set_paused_ui(&self, paused: bool) {
        self.paused_banner.set_visible(paused);
        if self.pause_btn.is_active() != paused {
            self.pause_btn.set_active(paused);
        }
    }

    pub fn refresh(&self) {
        reload_list(
            &self.store.borrow(),
            &self.model,
            &self.stack,
            &self.status_label,
            &self.nav_state.borrow(),
            self.search_entry.text().as_str(),
        );
    }
}

fn selected_clip_id(selection: &SingleSelection) -> Option<i64> {
    let obj = selection.selected_item()?;
    let boxed = obj.downcast::<glib::BoxedAnyObject>().ok()?;
    let id = boxed.borrow::<ClipRecord>().id;
    Some(id)
}

fn reload_list(
    store: &Store,
    model: &gio::ListStore,
    stack: &Stack,
    status_label: &Label,
    nav: &NavSelection,
    query: &str,
) {
    let filter = ClipFilter {
        query: if query.trim().is_empty() {
            None
        } else {
            Some(query.to_string())
        },
        kind: nav.kind,
        space_id: nav.space_id,
        pinned_only: nav.pinned_only,
        limit: 200,
        offset: 0,
    };

    let clips = store.list_clips(&filter).unwrap_or_default();
    let count = clips.len();
    let objects: Vec<glib::BoxedAnyObject> = clips
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect();
    model.splice(0, model.n_items(), &objects);

    if count == 0 {
        stack.set_visible_child_name("empty");
    } else {
        stack.set_visible_child_name("list");
    }
    status_label.set_text(&format!(
        "{count} clip{}",
        if count == 1 { "" } else { "s" }
    ));
}

struct RowWidgets {
    clip_id: Rc<Cell<i64>>,
    thumb: Image,
    kind_badge: Label,
    preview: Label,
    meta: Label,
    pin_btn: Button,
}

fn build_row_factory(
    thumbs_dir: PathBuf,
    on_action: Rc<dyn Fn(UiAction)>,
) -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();

    {
        let on_action_setup = Rc::clone(&on_action);
        factory.connect_setup(move |_f, obj| {
            let Some(item) = obj.downcast_ref::<ListItem>() else {
                return;
            };

            let row_box = GtkBox::new(Orientation::Horizontal, 10);
            row_box.add_css_class("pf-clip-row");

            let thumb = Image::new();
            thumb.set_pixel_size(40);
            thumb.set_valign(Align::Center);
            row_box.append(&thumb);

            let text_col = GtkBox::new(Orientation::Vertical, 3);
            text_col.set_hexpand(true);

            let preview = Label::new(None);
            preview.set_xalign(0.0);
            preview.set_lines(2);
            preview.set_single_line_mode(false);
            preview.set_ellipsize(pango::EllipsizeMode::End);
            preview.set_wrap(true);
            preview.set_wrap_mode(pango::WrapMode::WordChar);
            preview.add_css_class("pf-clip-preview");
            text_col.append(&preview);

            let meta_row = GtkBox::new(Orientation::Horizontal, 8);
            let kind_badge = Label::new(None);
            kind_badge.add_css_class("pf-kind-badge");
            meta_row.append(&kind_badge);

            let meta = Label::new(None);
            meta.set_xalign(0.0);
            meta.add_css_class("pf-clip-meta");
            meta_row.append(&meta);
            text_col.append(&meta_row);

            row_box.append(&text_col);

            // Row action buttons (connected once per recycled widget)
            let clip_id = Rc::new(Cell::new(0i64));

            let pin_btn = Button::builder()
                .icon_name("non-starred-symbolic")
                .tooltip_text("Pin / Unpin (Ctrl+P)")
                .has_frame(false)
                .valign(Align::Center)
                .build();
            {
                let id_c = Rc::clone(&clip_id);
                let act = Rc::clone(&on_action_setup);
                pin_btn.connect_clicked(move |_| {
                    let id = id_c.get();
                    if id > 0 {
                        act(UiAction::TogglePin(id));
                    }
                });
            }

            let copy_btn = Button::builder()
                .icon_name("edit-copy-symbolic")
                .tooltip_text("Copy to clipboard (Enter)")
                .has_frame(false)
                .valign(Align::Center)
                .build();
            {
                let id_c = Rc::clone(&clip_id);
                let act = Rc::clone(&on_action_setup);
                copy_btn.connect_clicked(move |_| {
                    let id = id_c.get();
                    if id > 0 {
                        act(UiAction::CopyClip(id));
                    }
                });
            }

            let del_btn = Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Delete clip (Del)")
                .has_frame(false)
                .valign(Align::Center)
                .build();
            {
                let id_c = Rc::clone(&clip_id);
                let act = Rc::clone(&on_action_setup);
                del_btn.connect_clicked(move |_| {
                    let id = id_c.get();
                    if id > 0 {
                        act(UiAction::DeleteClip(id));
                    }
                });
            }

            row_box.append(&pin_btn);
            row_box.append(&copy_btn);
            row_box.append(&del_btn);

            let widgets = Rc::new(RowWidgets {
                clip_id,
                thumb,
                kind_badge,
                preview,
                meta,
                pin_btn,
            });
            // SAFETY: Storing Rust struct in GObject qdata keyed uniquely on the row widget.
            unsafe {
                row_box.set_data("pf-row-widgets", widgets);
            }

            item.set_child(Some(&row_box));
        });
    }

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
        let widgets_opt: Option<Rc<RowWidgets>> = unsafe {
            child
                .data::<Rc<RowWidgets>>("pf-row-widgets")
                .map(|p| Rc::clone(p.as_ref()))
        };
        let Some(w) = widgets_opt else {
            return;
        };

        w.clip_id.set(clip.id);
        w.kind_badge.set_text(&clip.kind.as_str().to_ascii_uppercase());
        w.preview.set_text(&clip.preview);

        let rel = format_relative_time(clip.last_used_at, now_unix_ms());
        let size = format_bytes(clip.byte_size);
        let mut meta_parts = vec![rel, size];
        if let Some(ref app) = clip.source_app {
            meta_parts.push(app.clone());
        }
        if clip.copy_count > 1 {
            meta_parts.push(format!("×{}", clip.copy_count));
        }
        w.meta.set_text(&meta_parts.join(" · "));

        if clip.pinned {
            w.pin_btn.set_icon_name("starred-symbolic");
            w.pin_btn.add_css_class("pf-pin-active");
        } else {
            w.pin_btn.set_icon_name("non-starred-symbolic");
            w.pin_btn.remove_css_class("pf-pin-active");
        }

        // Load thumbnail for image clips or symbolic icon for other kinds
        if clip.kind == ClipKind::Image
            && let Some(ref im) = clip.image
            && let Some(ref tkey) = im.thumb_key
        {
            let path = thumbs_dir.join(tkey);
            if path.exists() {
                w.thumb.set_from_file(Some(&path));
            } else {
                w.thumb.set_icon_name(Some("image-x-generic-symbolic"));
            }
        } else {
            let icon = match clip.kind {
                ClipKind::Image => "image-x-generic-symbolic",
                ClipKind::Files => "folder-documents-symbolic",
                ClipKind::Code => "utilities-terminal-symbolic",
                ClipKind::Url => "insert-link-symbolic",
                ClipKind::Color => "applications-graphics-symbolic",
                _ => "text-x-generic-symbolic",
            };
            w.thumb.set_icon_name(Some(icon));
        }
    });

    factory
}
