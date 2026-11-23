//! Shortcut recorder & global keybinding configuration window.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, Box as GtkBox, Button, CheckButton, EventControllerKey, Label, Orientation,
    Separator, Window, gdk,
};
use pf_core::backend::HotkeyAction;
use pf_core::config::{Config, ShortcutSpec};
use pf_x11::HotkeyStatus;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct ShortcutSettingsUpdate {
    pub overlay_shortcut: String,
    pub paste_next_shortcut: String,
    pub use_cinnamon_keybindings: bool,
    pub overlay_at_pointer: bool,
}

/// Present a lightweight shortcut configuration window with live key recording,
/// conflict reporting, Pastily preset, and Cinnamon keybinding provider toggle.
pub fn show_shortcut_dialog(
    app: &Application,
    current: &Config,
    statuses: &[HotkeyStatus],
    on_apply: Rc<dyn Fn(ShortcutSettingsUpdate) -> Vec<HotkeyStatus>>,
) {
    let win = Window::builder()
        .application(app)
        .title("PasteFlow — Global Shortcuts")
        .default_width(480)
        .resizable(false)
        .build();

    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);

    let overlay_val = Rc::new(RefCell::new(current.overlay_shortcut.clone()));
    let paste_val = Rc::new(RefCell::new(current.paste_next_shortcut.clone()));

    let (overlay_row, overlay_btn, overlay_status_lbl) =
        build_recorder_row("Open Overlay", &current.overlay_shortcut, status_for(statuses, HotkeyAction::ToggleOverlay), Rc::clone(&overlay_val));
    let (paste_row, paste_btn, paste_status_lbl) =
        build_recorder_row("Paste Next (Queue)", &current.paste_next_shortcut, status_for(statuses, HotkeyAction::PasteNext), Rc::clone(&paste_val));

    vbox.append(&overlay_row);
    vbox.append(&paste_row);
    vbox.append(&Separator::new(Orientation::Horizontal));

    let cinnamon_check = CheckButton::with_label("Use Cinnamon custom keybindings instead of X11 grab");
    cinnamon_check.set_active(current.use_cinnamon_keybindings);
    vbox.append(&cinnamon_check);

    let pointer_check = CheckButton::with_label("Open floating overlay at mouse pointer");
    pointer_check.set_active(current.overlay_at_pointer);
    vbox.append(&pointer_check);

    // Presets row
    let preset_box = GtkBox::new(Orientation::Horizontal, 8);
    let default_preset = Button::with_label("PasteFlow Default (Super+V)");
    let pastily_preset = Button::with_label("Pastily Preset (Ctrl+Shift+B)");
    preset_box.append(&default_preset);
    preset_box.append(&pastily_preset);
    vbox.append(&preset_box);

    {
        let ov = Rc::clone(&overlay_val);
        let pv = Rc::clone(&paste_val);
        let ob = overlay_btn.clone();
        let pb = paste_btn.clone();
        default_preset.connect_clicked(move |_| {
            *ov.borrow_mut() = "<Super>v".to_string();
            *pv.borrow_mut() = "<Super><Shift>v".to_string();
            ob.set_label("<Super>v");
            pb.set_label("<Super><Shift>v");
        });
    }
    {
        let ov = Rc::clone(&overlay_val);
        let pv = Rc::clone(&paste_val);
        let ob = overlay_btn.clone();
        let pb = paste_btn.clone();
        pastily_preset.connect_clicked(move |_| {
            *ov.borrow_mut() = "<Primary><Shift>b".to_string();
            *pv.borrow_mut() = "<Primary><Shift>v".to_string();
            ob.set_label("<Primary><Shift>b");
            pb.set_label("<Primary><Shift>v");
        });
    }

    vbox.append(&Separator::new(Orientation::Horizontal));

    let action_box = GtkBox::new(Orientation::Horizontal, 8);
    action_box.set_halign(Align::End);
    let close_btn = Button::with_label("Close");
    let apply_btn = Button::with_label("Apply");
    apply_btn.add_css_class("suggested-action");
    action_box.append(&close_btn);
    action_box.append(&apply_btn);
    vbox.append(&action_box);

    {
        let win_weak = win.downgrade();
        close_btn.connect_clicked(move |_| {
            if let Some(w) = win_weak.upgrade() {
                w.close();
            }
        });
    }

    {
        let ov = Rc::clone(&overlay_val);
        let pv = Rc::clone(&paste_val);
        let cin = cinnamon_check.clone();
        let ptr = pointer_check.clone();
        let o_lbl = overlay_status_lbl.clone();
        let p_lbl = paste_status_lbl.clone();
        apply_btn.connect_clicked(move |_| {
            let new_statuses = on_apply(ShortcutSettingsUpdate {
                overlay_shortcut: ov.borrow().clone(),
                paste_next_shortcut: pv.borrow().clone(),
                use_cinnamon_keybindings: cin.is_active(),
                overlay_at_pointer: ptr.is_active(),
            });
            o_lbl.set_text(&status_for(&new_statuses, HotkeyAction::ToggleOverlay));
            p_lbl.set_text(&status_for(&new_statuses, HotkeyAction::PasteNext));
        });
    }

    win.set_child(Some(&vbox));
    win.present();
}

fn status_for(statuses: &[HotkeyStatus], action: HotkeyAction) -> String {
    match statuses.iter().find(|s| s.action == action) {
        Some(s) if s.grabbed => "Active".to_string(),
        Some(s) => format!("Conflict: {}", s.error.as_deref().unwrap_or("unavailable")),
        None => "Not grabbed".to_string(),
    }
}

fn build_recorder_row(
    title: &str,
    initial_accel: &str,
    initial_status: String,
    target_val: Rc<RefCell<String>>,
) -> (GtkBox, Button, Label) {
    let outer = GtkBox::new(Orientation::Vertical, 4);
    let row = GtkBox::new(Orientation::Horizontal, 8);

    let lbl = Label::new(Some(title));
    lbl.set_xalign(0.0);
    lbl.set_hexpand(true);
    row.append(&lbl);

    let btn = Button::with_label(initial_accel);
    let recording = Rc::new(Cell::new(false));
    {
        let rec_c = Rc::clone(&recording);
        let btn_c = btn.clone();
        btn.connect_clicked(move |_| {
            rec_c.set(true);
            btn_c.set_label("Press shortcut…");
        });
    }

    let key_ctrl = EventControllerKey::new();
    {
        let rec_c = Rc::clone(&recording);
        let btn_c = btn.clone();
        let val_c = Rc::clone(&target_val);
        key_ctrl.connect_key_pressed(move |_ctrl, key, _code, mods| {
            if !rec_c.get() {
                return glib::Propagation::Proceed;
            }
            if key == gdk::Key::Escape {
                rec_c.set(false);
                btn_c.set_label(&val_c.borrow());
                return glib::Propagation::Stop;
            }
            // Ignore bare modifier presses
            if matches!(
                key,
                gdk::Key::Shift_L
                    | gdk::Key::Shift_R
                    | gdk::Key::Control_L
                    | gdk::Key::Control_R
                    | gdk::Key::Alt_L
                    | gdk::Key::Alt_R
                    | gdk::Key::Super_L
                    | gdk::Key::Super_R
            ) {
                return glib::Propagation::Stop;
            }

            if let Some(key_name) = key.name() {
                let spec = ShortcutSpec {
                    ctrl: mods.contains(gdk::ModifierType::CONTROL_MASK),
                    alt: mods.contains(gdk::ModifierType::ALT_MASK),
                    shift: mods.contains(gdk::ModifierType::SHIFT_MASK),
                    super_key: mods.contains(gdk::ModifierType::SUPER_MASK),
                    key: key_name.to_ascii_lowercase(),
                };
                if spec.x11_keysym().is_some()
                    && (spec.ctrl || spec.alt || spec.super_key)
                {
                    let accel = spec.to_accel_string();
                    *val_c.borrow_mut() = accel.clone();
                    btn_c.set_label(&accel);
                    rec_c.set(false);
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Stop
        });
    }
    btn.add_controller(key_ctrl);
    row.append(&btn);
    outer.append(&row);

    let status_lbl = Label::new(Some(&initial_status));
    status_lbl.set_xalign(0.0);
    status_lbl.add_css_class("pf-clip-meta");
    outer.append(&status_lbl);

    (outer, btn, status_lbl)
}
