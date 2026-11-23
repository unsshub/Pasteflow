use crate::ui::detail::{DetailAction, DetailPanel};
use crate::ui::queue_view::{QueueUiAction, QueueView};
use crate::ui::{format_bytes, format_relative_time};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, DragSource, DropTarget,
    EventControllerKey, Image, Label, ListBox, ListBoxRow, ListItem, ListView, Orientation, Paned,
    PolicyType, ScrolledWindow, SearchEntry, Separator, SignalListItemFactory, SingleSelection,
    Stack, ToggleButton, gdk, pango,
};
use pf_core::model::{ClipFilter, ClipKind, ClipRecord, now_unix_ms};
use pf_store::{EphemeralStore, Store};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

/// Actions triggered by user interaction inside the Main Window.
#[derive(Debug, Clone)]
pub enum UiAction {
    CopyClip(i64),
    CopyText(String),
    DeleteClip(i64),
    TogglePin(i64),
    TogglePause,
    ClearUnpinned,
    OpenShortcuts,
    SaveImageToDownloads(i64),
    OpenImageInViewer(i64),
    OpenUrl(String),
    SetLanguage { clip_id: i64, lang: String },
    SetExpiry { clip_id: i64, secs: Option<u64> },
    AddToQueue(i64),
    Queue(QueueUiAction),
    ToggleUniversalPasteShortcut,
    NewSpace,
    EditSpace(i64),
    MoveClipToSpace { clip_id: i64, space_id: i64 },
    PromptMoveClip(i64),
    PromptRenameClip(i64),
}

#[derive(Debug, Clone, Default)]
struct NavSelection {
    show_queue: bool,
    pinned_only: bool,
    kind: Option<ClipKind>,
    kinds: Vec<ClipKind>,
    space_id: Option<i64>,
}

/// Main PasteFlow application window (1000×640, plain GTK4 following Mint-Y).
pub struct MainWindow {
    window: ApplicationWindow,
    model: gio::ListStore,
    selection: SingleSelection,
    search_entry: SearchEntry,
    stack: Stack,
    queue_view: Rc<QueueView>,
    queue_badge: Label,
    spaces_list: ListBox,
    spaces_ids: Rc<RefCell<Vec<i64>>>,
    paused_banner: Label,
    pause_btn: ToggleButton,
    status_label: Label,
    nav_state: Rc<RefCell<NavSelection>>,
    store: Rc<RefCell<Store>>,
    ephemeral: Rc<RefCell<EphemeralStore>>,
    on_action: Rc<dyn Fn(UiAction)>,
}

impl MainWindow {
    pub fn new(
        app: &Application,
        store: Rc<RefCell<Store>>,
        ephemeral: Rc<RefCell<EphemeralStore>>,
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

        // Paste Queue row with live count badge
        let queue_badge = Label::new(Some("0"));
        queue_badge.add_css_class("pf-kind-badge");
        {
            let h = GtkBox::new(Orientation::Horizontal, 8);
            h.set_margin_top(4);
            h.set_margin_bottom(4);
            h.set_margin_start(6);
            h.set_margin_end(6);
            let img = Image::from_icon_name("view-list-ordered-symbolic");
            let lbl = Label::new(Some("Paste Queue"));
            lbl.set_xalign(0.0);
            lbl.set_hexpand(true);
            h.append(&img);
            h.append(&lbl);
            h.append(&queue_badge);
            let row = ListBoxRow::new();
            row.set_child(Some(&h));
            nav_list.append(&row);
            nav_targets.push(NavSelection {
                show_queue: true,
                ..Default::default()
            });
        }

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
            ("accessories-calculator-symbolic", "Numbers", ClipKind::Number),
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
        add_nav_row(
            &nav_list,
            &mut nav_targets,
            "contact-new-symbolic",
            "Contacts",
            NavSelection {
                kinds: vec![ClipKind::Email, ClipKind::Phone],
                ..Default::default()
            },
        );

        let nav_scroll_inner = GtkBox::new(Orientation::Vertical, 4);
        nav_scroll_inner.append(&nav_list);

        // Spaces section (dynamic list with clip counts + drag-and-drop DropTarget per Space)
        let spaces_header = GtkBox::new(Orientation::Horizontal, 4);
        let sp_lbl = Label::new(Some("SPACES"));
        sp_lbl.set_xalign(0.0);
        sp_lbl.set_hexpand(true);
        sp_lbl.add_css_class("pf-section-label");
        let new_space_btn = Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("New Space")
            .has_frame(false)
            .build();
        {
            let act = Rc::clone(&on_action);
            new_space_btn.connect_clicked(move |_| act(UiAction::NewSpace));
        }
        spaces_header.append(&sp_lbl);
        spaces_header.append(&new_space_btn);
        nav_scroll_inner.append(&spaces_header);

        let spaces_list = ListBox::new();
        spaces_list.set_selection_mode(gtk4::SelectionMode::Single);
        spaces_list.add_css_class("navigation-sidebar");
        let spaces_ids: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        nav_scroll_inner.append(&spaces_list);

        if let Some(first) = nav_list.row_at_index(0) {
            nav_list.select_row(Some(&first));
        }

        let nav_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vexpand(true)
            .child(&nav_scroll_inner)
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

        let detail_toggle_btn = ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text("Toggle detail panel (F9)")
            .active(true)
            .build();
        toolbar.append(&detail_toggle_btn);

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

        // Collapsible right-hand Detail Panel (`GtkPaned`, F9 toggle)
        let on_detail_action: Rc<dyn Fn(DetailAction)> = {
            let act = Rc::clone(&on_action);
            Rc::new(move |da| match da {
                DetailAction::CopyText(t) => act(UiAction::CopyText(t)),
                DetailAction::CopyClip(id) => act(UiAction::CopyClip(id)),
                DetailAction::SaveImageToDownloads(id) => act(UiAction::SaveImageToDownloads(id)),
                DetailAction::OpenImageInViewer(id) => act(UiAction::OpenImageInViewer(id)),
                DetailAction::OpenUrl(u) => act(UiAction::OpenUrl(u)),
                DetailAction::SetLanguage { clip_id, lang } => {
                    act(UiAction::SetLanguage { clip_id, lang })
                }
                DetailAction::SetExpiry { clip_id, secs } => {
                    act(UiAction::SetExpiry { clip_id, secs })
                }
            })
        };
        let detail_panel = Rc::new(DetailPanel::new(Rc::clone(&store), on_detail_action));

        let paned = Paned::new(Orientation::Horizontal);
        paned.set_vexpand(true);
        paned.set_hexpand(true);
        paned.set_start_child(Some(&stack));
        paned.set_end_child(Some(detail_panel.widget()));
        paned.set_resize_start_child(true);
        paned.set_resize_end_child(false);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);
        paned.set_position(460);
        let on_queue_action: Rc<dyn Fn(QueueUiAction)> = {
            let act = Rc::clone(&on_action);
            Rc::new(move |qa| act(UiAction::Queue(qa)))
        };
        let queue_view = Rc::new(QueueView::new(Rc::clone(&store), on_queue_action));

        let main_stack = Stack::new();
        main_stack.set_vexpand(true);
        main_stack.set_hexpand(true);
        main_stack.add_named(&paned, Some("clips"));
        main_stack.add_named(queue_view.widget(), Some("queue"));
        content_vbox.append(&main_stack);

        // Update detail panel whenever selection changes
        {
            let dp = Rc::clone(&detail_panel);
            selection.connect_selected_item_notify(move |sel| {
                if let Some(obj) = sel.selected_item()
                    && let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>()
                {
                    dp.show_clip(&boxed.borrow::<ClipRecord>());
                } else {
                    dp.clear();
                }
            });
        }

        // Toggle detail panel visibility from toolbar button
        {
            let dp = Rc::clone(&detail_panel);
            detail_toggle_btn.connect_toggled(move |btn| {
                dp.widget().set_visible(btn.is_active());
            });
        }

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
            queue_view,
            queue_badge,
            spaces_list: spaces_list.clone(),
            spaces_ids: Rc::clone(&spaces_ids),
            paused_banner,
            pause_btn,
            status_label,
            nav_state,
            store,
            ephemeral,
            on_action: Rc::clone(&on_action),
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
            let eph_c = Rc::clone(&main_win.ephemeral);
            main_win.search_entry.connect_search_changed(move |entry| {
                reload_list(
                    &store_c.borrow(),
                    &eph_c.borrow(),
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
            let eph_c = Rc::clone(&main_win.ephemeral);
            let search_c = main_win.search_entry.clone();
            let qv_c = Rc::clone(&main_win.queue_view);
            let mstack_c = main_stack.clone();
            let sp_list_c = spaces_list.clone();
            let targets = Rc::new(nav_targets);
            nav_list.connect_row_selected(move |_lb, row_opt| {
                let Some(row) = row_opt else { return };
                let idx = row.index();
                if idx < 0 {
                    return;
                }
                sp_list_c.unselect_all();
                if let Some(target) = targets.get(idx as usize) {
                    *nav_c.borrow_mut() = target.clone();
                    if target.show_queue {
                        qv_c.refresh();
                        mstack_c.set_visible_child_name("queue");
                    } else {
                        mstack_c.set_visible_child_name("clips");
                        reload_list(
                            &store_c.borrow(),
                            &eph_c.borrow(),
                            &model_c,
                            &stack_c,
                            &status_c,
                            &nav_c.borrow(),
                            search_c.text().as_str(),
                        );
                    }
                }
            });
        }

        // Wire Spaces sidebar selection
        {
            let model_c = main_win.model.clone();
            let stack_c = main_win.stack.clone();
            let status_c = main_win.status_label.clone();
            let nav_c = Rc::clone(&main_win.nav_state);
            let store_c = Rc::clone(&main_win.store);
            let eph_c = Rc::clone(&main_win.ephemeral);
            let search_c = main_win.search_entry.clone();
            let mstack_c = main_stack.clone();
            let nav_list_c = nav_list.clone();
            let sp_ids_c = Rc::clone(&spaces_ids);
            spaces_list.connect_row_selected(move |_lb, row_opt| {
                let Some(row) = row_opt else { return };
                let idx = row.index();
                if idx < 0 {
                    return;
                }
                nav_list_c.unselect_all();
                if let Some(&space_id) = sp_ids_c.borrow().get(idx as usize) {
                    *nav_c.borrow_mut() = NavSelection {
                        space_id: Some(space_id),
                        ..Default::default()
                    };
                    mstack_c.set_visible_child_name("clips");
                    reload_list(
                        &store_c.borrow(),
                        &eph_c.borrow(),
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
            let dt_btn = detail_toggle_btn.clone();

            key_ctrl.connect_key_pressed(move |_ctrl, key, _code, mods| {
                let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
                let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
                if key == gdk::Key::F9 {
                    dt_btn.set_active(!dt_btn.is_active());
                    return glib::Propagation::Stop;
                }
                if ctrl && shift && (key == gdk::Key::u || key == gdk::Key::U) {
                    act_c(UiAction::ToggleUniversalPasteShortcut);
                    return glib::Propagation::Stop;
                }
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
                    if ctrl && (key == gdk::Key::q || key == gdk::Key::Q) {
                        act_c(UiAction::AddToQueue(id));
                        return glib::Propagation::Stop;
                    }
                    if ctrl && (key == gdk::Key::m || key == gdk::Key::M) {
                        act_c(UiAction::PromptMoveClip(id));
                        return glib::Propagation::Stop;
                    }
                    if key == gdk::Key::F2 {
                        act_c(UiAction::PromptRenameClip(id));
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

    pub fn sync_queue_state(&self, universal: bool, manual: bool, count: usize) {
        self.queue_badge.set_text(&count.to_string());
        self.queue_view.sync_switches(universal, manual);
        self.queue_view.refresh();
    }

    pub fn refresh(&self) {
        let qcount = self.store.borrow().queue_count().unwrap_or(0);
        self.queue_badge.set_text(&qcount.to_string());
        self.queue_view.refresh();
        self.reload_spaces_sidebar();
        reload_list(
            &self.store.borrow(),
            &self.ephemeral.borrow(),
            &self.model,
            &self.stack,
            &self.status_label,
            &self.nav_state.borrow(),
            self.search_entry.text().as_str(),
        );
    }

    fn reload_spaces_sidebar(&self) {
        let selected_space = self.nav_state.borrow().space_id;
        while let Some(child) = self.spaces_list.first_child() {
            self.spaces_list.remove(&child);
        }
        let mut ids = self.spaces_ids.borrow_mut();
        ids.clear();

        let spaces = self
            .store
            .borrow()
            .list_spaces_with_counts()
            .unwrap_or_default();

        for (sp, count) in spaces {
            ids.push(sp.id);
            let h = GtkBox::new(Orientation::Horizontal, 6);
            h.set_margin_top(4);
            h.set_margin_bottom(4);
            h.set_margin_start(6);
            h.set_margin_end(4);

            let img = Image::from_icon_name(&sp.icon);
            let lbl = Label::new(Some(&sp.name));
            lbl.set_xalign(0.0);
            lbl.set_hexpand(true);
            lbl.set_ellipsize(pango::EllipsizeMode::End);

            let cnt = Label::new(Some(&count.to_string()));
            cnt.add_css_class("pf-clip-meta");

            let edit_btn = Button::builder()
                .icon_name("document-edit-symbolic")
                .tooltip_text("Edit Space")
                .has_frame(false)
                .build();
            {
                let sid = sp.id;
                let act = Rc::clone(&self.on_action);
                edit_btn.connect_clicked(move |_| act(UiAction::EditSpace(sid)));
            }

            h.append(&img);
            h.append(&lbl);
            h.append(&cnt);
            h.append(&edit_btn);

            let row = ListBoxRow::new();
            row.set_child(Some(&h));

            // GTK4 DropTarget: dropping a clip ID onto a Space row moves the clip to that Space
            let drop_target = DropTarget::new(i64::static_type(), gdk::DragAction::MOVE);
            {
                let target_space_id = sp.id;
                let act = Rc::clone(&self.on_action);
                drop_target.connect_drop(move |_dt, val, _x, _y| {
                    if let Ok(clip_id) = val.get::<i64>() {
                        act(UiAction::MoveClipToSpace {
                            clip_id,
                            space_id: target_space_id,
                        });
                        return true;
                    }
                    false
                });
            }
            row.add_controller(drop_target);

            self.spaces_list.append(&row);
            if selected_space == Some(sp.id) {
                self.spaces_list.select_row(Some(&row));
            }
        }
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
    ephemeral: &EphemeralStore,
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
        kinds: nav.kinds.clone(),
        space_id: nav.space_id,
        pinned_only: nav.pinned_only,
        limit: 200,
        offset: 0,
    };

    let mut clips = Vec::new();
    // Prepend memory-only ephemeral clips in All Clips view when no search query is active
    if !nav.pinned_only && nav.space_id.is_none() && nav.kind.is_none() && nav.kinds.is_empty() && query.trim().is_empty() {
        clips.extend(ephemeral.list());
    }
    clips.extend(store.list_clips(&filter).unwrap_or_default());
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

            let queue_btn = Button::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text("Add to Paste Queue (Ctrl+Q)")
                .has_frame(false)
                .valign(Align::Center)
                .build();
            {
                let id_c = Rc::clone(&clip_id);
                let act = Rc::clone(&on_action_setup);
                queue_btn.connect_clicked(move |_| {
                    let id = id_c.get();
                    if id > 0 {
                        act(UiAction::AddToQueue(id));
                    }
                });
            }

            row_box.append(&pin_btn);
            row_box.append(&queue_btn);
            row_box.append(&copy_btn);
            row_box.append(&del_btn);

            // GTK4 DragSource: drag any clip row onto a sidebar Space to move it
            let drag_src = DragSource::new();
            drag_src.set_actions(gdk::DragAction::MOVE);
            {
                let id_c = Rc::clone(&clip_id);
                drag_src.connect_prepare(move |_src, _x, _y| {
                    let id = id_c.get();
                    if id > 0 {
                        Some(gdk::ContentProvider::for_value(&id.to_value()))
                    } else {
                        None
                    }
                });
            }
            row_box.add_controller(drag_src);

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
        if let Some(ref title) = clip.title {
            w.preview.set_text(&format!("{title}\n{}", clip.preview));
        } else {
            w.preview.set_text(&clip.preview);
        }

        let rel = format_relative_time(clip.last_used_at, now_unix_ms());
        let size = format_bytes(clip.byte_size);
        let mut meta_parts = vec![rel, size];
        if let Some(ref app) = clip.source_app {
            meta_parts.push(app.clone());
        }
        if clip.copy_count > 1 {
            meta_parts.push(format!("×{}", clip.copy_count));
        }
        if let Some(exp) = clip.expires_at {
            let rem_s = ((exp - now_unix_ms()).max(0) + 999) / 1000;
            if rem_s < 60 {
                meta_parts.push(format!("⏱ {rem_s}s"));
            } else if rem_s < 3600 {
                meta_parts.push(format!("⏱ {}m", rem_s / 60));
            } else {
                meta_parts.push(format!("⏱ {}h", rem_s / 3600));
            }
        }
        if clip.id < 0 {
            meta_parts.push("RAM-only".to_string());
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
