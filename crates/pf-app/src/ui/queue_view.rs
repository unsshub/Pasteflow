//! Screen 4 — Paste Queue view (Universal Paste switch, Auto/Manual mode, FIFO list with reorder).

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, EventControllerKey, Label, ListBox, ListBoxRow, Orientation,
    PolicyType, ScrolledWindow, Separator, Switch, gdk, pango,
};
use pf_core::model::QueueItemRecord;
use pf_store::Store;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Debug, Clone)]
pub enum QueueUiAction {
    ToggleUniversalPaste(bool),
    ToggleManualMode(bool),
    PasteNext,
    SkipHead,
    ClearQueue,
    MoveItem { queue_item_id: i64, delta: i32 },
    RemoveItem(i64),
}

pub struct QueueView {
    root: GtkBox,
    universal_switch: Switch,
    manual_switch: Switch,
    count_label: Label,
    list_box: ListBox,
    suppress_switches: Rc<Cell<bool>>,
    item_ids: Rc<RefCell<Vec<i64>>>,
    store: Rc<RefCell<Store>>,
    on_action: Rc<dyn Fn(QueueUiAction)>,
}

impl QueueView {
    pub fn new(store: Rc<RefCell<Store>>, on_action: Rc<dyn Fn(QueueUiAction)>) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);

        let suppress_switches = Rc::new(Cell::new(false));
        let item_ids = Rc::new(RefCell::new(Vec::new()));

        // Header controls
        let header = GtkBox::new(Orientation::Horizontal, 12);
        header.add_css_class("pf-toolbar");

        let title = Label::new(Some("Paste Queue"));
        title.add_css_class("pf-brand");
        header.append(&title);

        let count_label = Label::new(Some("0 left"));
        count_label.add_css_class("pf-kind-badge");
        header.append(&count_label);

        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        header.append(&spacer);

        let uni_lbl = Label::new(Some("Universal Paste"));
        let universal_switch = Switch::new();
        universal_switch.set_valign(Align::Center);
        universal_switch.set_tooltip_text(Some("Automatically append every new copy to the queue (Ctrl+Shift+U)"));
        {
            let sup = Rc::clone(&suppress_switches);
            let act = Rc::clone(&on_action);
            universal_switch.connect_state_set(move |_sw, state| {
                if !sup.get() {
                    act(QueueUiAction::ToggleUniversalPaste(state));
                }
                glib::Propagation::Proceed
            });
        }
        header.append(&uni_lbl);
        header.append(&universal_switch);

        let man_lbl = Label::new(Some("Manual"));
        let manual_switch = Switch::new();
        manual_switch.set_valign(Align::Center);
        manual_switch.set_tooltip_text(Some("Load clipboard without synthetic Ctrl+V"));
        {
            let sup = Rc::clone(&suppress_switches);
            let act = Rc::clone(&on_action);
            manual_switch.connect_state_set(move |_sw, state| {
                if !sup.get() {
                    act(QueueUiAction::ToggleManualMode(state));
                }
                glib::Propagation::Proceed
            });
        }
        header.append(&man_lbl);
        header.append(&manual_switch);

        root.append(&header);

        // Action bar
        let action_bar = GtkBox::new(Orientation::Horizontal, 8);
        action_bar.add_css_class("pf-toolbar");

        let paste_btn = Button::with_label("Paste Next");
        paste_btn.add_css_class("suggested-action");
        {
            let act = Rc::clone(&on_action);
            paste_btn.connect_clicked(move |_| act(QueueUiAction::PasteNext));
        }

        let skip_btn = Button::with_label("Skip");
        {
            let act = Rc::clone(&on_action);
            skip_btn.connect_clicked(move |_| act(QueueUiAction::SkipHead));
        }

        let clear_btn = Button::with_label("Clear Queue");
        {
            let act = Rc::clone(&on_action);
            clear_btn.connect_clicked(move |_| act(QueueUiAction::ClearQueue));
        }

        action_bar.append(&paste_btn);
        action_bar.append(&skip_btn);
        action_bar.append(&clear_btn);
        root.append(&action_bar);
        root.append(&Separator::new(Orientation::Horizontal));

        // Ordered FIFO list
        let list_box = ListBox::new();
        list_box.set_selection_mode(gtk4::SelectionMode::Single);

        // Alt+Up / Alt+Down keyboard reordering
        {
            let lb_c = list_box.clone();
            let ids_c = Rc::clone(&item_ids);
            let act = Rc::clone(&on_action);
            let key_ctrl = EventControllerKey::new();
            key_ctrl.connect_key_pressed(move |_ctrl, key, _code, mods| {
                if mods.contains(gdk::ModifierType::ALT_MASK)
                    && let Some(row) = lb_c.selected_row()
                {
                    let idx = row.index();
                    if idx >= 0
                        && let Some(&qid) = ids_c.borrow().get(idx as usize)
                    {
                        if key == gdk::Key::Up {
                            act(QueueUiAction::MoveItem {
                                queue_item_id: qid,
                                delta: -1,
                            });
                            return glib::Propagation::Stop;
                        }
                        if key == gdk::Key::Down {
                            act(QueueUiAction::MoveItem {
                                queue_item_id: qid,
                                delta: 1,
                            });
                            return glib::Propagation::Stop;
                        }
                    }
                }
                glib::Propagation::Proceed
            });
            list_box.add_controller(key_ctrl);
        }

        let scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .child(&list_box)
            .build();
        root.append(&scroll);

        Self {
            root,
            universal_switch,
            manual_switch,
            count_label,
            list_box,
            suppress_switches,
            item_ids,
            store,
            on_action,
        }
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    pub fn sync_switches(&self, universal: bool, manual: bool) {
        self.suppress_switches.set(true);
        if self.universal_switch.is_active() != universal {
            self.universal_switch.set_active(universal);
        }
        if self.manual_switch.is_active() != manual {
            self.manual_switch.set_active(manual);
        }
        self.suppress_switches.set(false);
    }

    pub fn refresh(&self) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let items: Vec<QueueItemRecord> = self.store.borrow().queue_list().unwrap_or_default();
        self.count_label.set_text(&format!("{} left", items.len()));

        let mut ids = self.item_ids.borrow_mut();
        ids.clear();

        if items.is_empty() {
            let empty = Label::new(Some(
                "Queue is empty. Press Ctrl+Q on any clip or turn on Universal Paste.",
            ));
            empty.add_css_class("pf-empty-state");
            empty.set_margin_top(48);
            let row = ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            row.set_child(Some(&empty));
            self.list_box.append(&row);
            return;
        }

        for (idx, item) in items.iter().enumerate() {
            ids.push(item.id);
            let hbox = GtkBox::new(Orientation::Horizontal, 8);
            hbox.add_css_class("pf-clip-row");

            let pos_lbl = if idx == 0 {
                let l = Label::new(Some("NEXT"));
                l.add_css_class("pf-kind-badge");
                l.add_css_class("pf-pin-active");
                l
            } else {
                let l = Label::new(Some(&format!("#{}", idx + 1)));
                l.add_css_class("pf-clip-meta");
                l.set_width_chars(4);
                l
            };
            hbox.append(&pos_lbl);

            let preview = Label::new(Some(&item.clip.preview.replace('\n', " ↵ ")));
            preview.set_xalign(0.0);
            preview.set_hexpand(true);
            preview.set_single_line_mode(true);
            preview.set_ellipsize(pango::EllipsizeMode::End);
            hbox.append(&preview);

            let qid = item.id;
            let up_btn = Button::builder()
                .icon_name("go-up-symbolic")
                .tooltip_text("Move up (Alt+↑)")
                .has_frame(false)
                .build();
            {
                let act = Rc::clone(&self.on_action);
                up_btn.connect_clicked(move |_| {
                    act(QueueUiAction::MoveItem {
                        queue_item_id: qid,
                        delta: -1,
                    });
                });
            }

            let down_btn = Button::builder()
                .icon_name("go-down-symbolic")
                .tooltip_text("Move down (Alt+↓)")
                .has_frame(false)
                .build();
            {
                let act = Rc::clone(&self.on_action);
                down_btn.connect_clicked(move |_| {
                    act(QueueUiAction::MoveItem {
                        queue_item_id: qid,
                        delta: 1,
                    });
                });
            }

            let rm_btn = Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text("Remove from queue")
                .has_frame(false)
                .build();
            {
                let act = Rc::clone(&self.on_action);
                rm_btn.connect_clicked(move |_| act(QueueUiAction::RemoveItem(qid)));
            }

            hbox.append(&up_btn);
            hbox.append(&down_btn);
            hbox.append(&rm_btn);

            let row = ListBoxRow::new();
            row.set_child(Some(&hbox));
            self.list_box.append(&row);
        }
    }
}
