//! Collapsible right-hand detail panel with GtkSourceView syntax highlighting,
//! color swatch + HEX/RGB/HSL conversions, URL breakdown, and full image preview / Save / Open.

use crate::ui::{format_bytes, format_relative_time};
use gtk4::gio;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, ContentFit, DrawingArea, DropDown, Label, Orientation, Picture,
    ScrolledWindow, Separator, Stack, StringList, gdk,
};
use pf_core::detect::color::ParsedColor;
use pf_core::model::{ClipKind, ClipRecord, now_unix_ms, parse_file_list};
use pf_store::Store;
use sourceview5::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const LANG_OPTIONS: &[(&str, &str)] = &[
    ("Plain Text", ""),
    ("Rust", "rust"),
    ("Python", "python"),
    ("JavaScript", "js"),
    ("TypeScript", "typescript"),
    ("JSON", "json"),
    ("SQL", "sql"),
    ("Shell / Bash", "sh"),
    ("HTML", "html"),
    ("CSS", "css"),
    ("C / C++", "cpp"),
    ("Go", "go"),
    ("TOML", "toml"),
    ("XML", "xml"),
];

#[derive(Debug, Clone)]
pub enum DetailAction {
    CopyText(String),
    CopyClip(i64),
    SaveImageToDownloads(i64),
    OpenImageInViewer(i64),
    OpenUrl(String),
    SetLanguage { clip_id: i64, lang: String },
}

pub struct DetailPanel {
    root: GtkBox,
    stack: Stack,
    header_kind: Label,
    lang_dropdown: DropDown,
    source_buffer: sourceview5::Buffer,
    color_rgb: Rc<Cell<(f64, f64, f64, f64)>>,
    color_swatch: DrawingArea,
    hex_val: Label,
    rgb_val: Label,
    hsl_val: Label,
    url_host: Label,
    url_scheme: Label,
    url_path: Label,
    url_full: Rc<RefCell<String>>,
    picture: Picture,
    image_info: Label,
    meta_label: Label,
    current_clip_id: Rc<Cell<Option<i64>>>,
    suppress_lang_signal: Rc<Cell<bool>>,
    store: Rc<RefCell<Store>>,
}

impl DetailPanel {
    pub fn new(store: Rc<RefCell<Store>>, on_action: Rc<dyn Fn(DetailAction)>) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_width_request(300);

        let current_clip_id = Rc::new(Cell::new(None));
        let suppress_lang_signal = Rc::new(Cell::new(false));

        // Top header bar
        let header = GtkBox::new(Orientation::Horizontal, 8);
        header.add_css_class("pf-toolbar");
        let header_kind = Label::new(Some("DETAILS"));
        header_kind.add_css_class("pf-kind-badge");
        header_kind.set_xalign(0.0);
        header.append(&header_kind);

        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        header.append(&spacer);

        let lang_names: Vec<&str> = LANG_OPTIONS.iter().map(|(name, _)| *name).collect();
        let lang_dropdown = DropDown::new(Some(StringList::new(&lang_names)), gtk4::Expression::NONE);
        lang_dropdown.set_visible(false);
        {
            let id_c = Rc::clone(&current_clip_id);
            let sup_c = Rc::clone(&suppress_lang_signal);
            let act_c = Rc::clone(&on_action);
            lang_dropdown.connect_selected_notify(move |dd| {
                if sup_c.get() {
                    return;
                }
                if let Some(clip_id) = id_c.get()
                    && let Some((_, lang_id)) = LANG_OPTIONS.get(dd.selected() as usize)
                {
                    act_c(DetailAction::SetLanguage {
                        clip_id,
                        lang: (*lang_id).to_string(),
                    });
                }
            });
        }
        header.append(&lang_dropdown);

        let copy_clip_btn = Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy clip to clipboard")
            .build();
        {
            let id_c = Rc::clone(&current_clip_id);
            let act_c = Rc::clone(&on_action);
            copy_clip_btn.connect_clicked(move |_| {
                if let Some(id) = id_c.get() {
                    act_c(DetailAction::CopyClip(id));
                }
            });
        }
        header.append(&copy_clip_btn);

        root.append(&header);
        root.append(&Separator::new(Orientation::Horizontal));

        // Main preview Stack: empty | code | color | url | image
        let stack = Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);

        // 1. Empty page
        let empty_lbl = Label::new(Some("Select a clip to inspect details."));
        empty_lbl.add_css_class("pf-empty-state");
        empty_lbl.set_halign(Align::Center);
        empty_lbl.set_valign(Align::Center);
        stack.add_named(&empty_lbl, Some("empty"));

        // 2. Code / Text / Files page (GtkSourceView)
        let source_buffer = sourceview5::Buffer::new(None);
        source_buffer.set_highlight_syntax(true);
        if let Some(scheme) = sourceview5::StyleSchemeManager::default()
            .scheme("Adwaita-dark")
            .or_else(|| sourceview5::StyleSchemeManager::default().scheme("classic-dark"))
            .or_else(|| sourceview5::StyleSchemeManager::default().scheme("classic"))
        {
            source_buffer.set_style_scheme(Some(&scheme));
        }
        let source_view = sourceview5::View::with_buffer(&source_buffer);
        source_view.set_editable(false);
        source_view.set_cursor_visible(false);
        source_view.set_monospace(true);
        source_view.set_wrap_mode(gtk4::WrapMode::WordChar);
        source_view.set_show_line_numbers(true);
        source_view.set_top_margin(8);
        source_view.set_bottom_margin(8);
        source_view.set_left_margin(8);
        source_view.set_right_margin(8);

        let code_scroll = ScrolledWindow::builder()
            .vexpand(true)
            .hexpand(true)
            .child(&source_view)
            .build();
        stack.add_named(&code_scroll, Some("code"));

        // 3. Color page
        let color_box = GtkBox::new(Orientation::Vertical, 12);
        color_box.set_margin_top(16);
        color_box.set_margin_bottom(16);
        color_box.set_margin_start(16);
        color_box.set_margin_end(16);

        let color_rgb = Rc::new(Cell::new((0.5, 0.5, 0.5, 1.0)));
        let color_swatch = DrawingArea::new();
        color_swatch.set_content_height(110);
        color_swatch.set_hexpand(true);
        {
            let rgb_c = Rc::clone(&color_rgb);
            color_swatch.set_draw_func(move |_area, cr, w, h| {
                let (r, g, b, a) = rgb_c.get();
                cr.set_source_rgba(r, g, b, a);
                cr.rectangle(0.0, 0.0, f64::from(w), f64::from(h));
                let _ = cr.fill();
            });
        }
        color_box.append(&color_swatch);

        let (hex_row, hex_val) = build_copy_field("HEX", Rc::clone(&on_action));
        let (rgb_row, rgb_val) = build_copy_field("RGB", Rc::clone(&on_action));
        let (hsl_row, hsl_val) = build_copy_field("HSL", Rc::clone(&on_action));
        color_box.append(&hex_row);
        color_box.append(&rgb_row);
        color_box.append(&hsl_row);
        stack.add_named(&color_box, Some("color"));

        // 4. URL page
        let url_box = GtkBox::new(Orientation::Vertical, 10);
        url_box.set_margin_top(16);
        url_box.set_margin_bottom(16);
        url_box.set_margin_start(16);
        url_box.set_margin_end(16);

        let url_host = Label::new(None);
        url_host.set_xalign(0.0);
        url_host.add_css_class("pf-brand");
        url_host.set_selectable(true);
        url_box.append(&url_host);

        let url_scheme = Label::new(None);
        url_scheme.set_xalign(0.0);
        url_scheme.add_css_class("pf-clip-meta");
        url_box.append(&url_scheme);

        let url_path = Label::new(None);
        url_path.set_xalign(0.0);
        url_path.set_wrap(true);
        url_path.set_selectable(true);
        url_box.append(&url_path);

        let url_full = Rc::new(RefCell::new(String::new()));
        let url_btn_row = GtkBox::new(Orientation::Horizontal, 8);
        let open_url_btn = Button::with_label("Open in Browser");
        {
            let u = Rc::clone(&url_full);
            let act_c = Rc::clone(&on_action);
            open_url_btn.connect_clicked(move |_| {
                let s = u.borrow().clone();
                if !s.is_empty() {
                    act_c(DetailAction::OpenUrl(s));
                }
            });
        }
        let copy_url_btn = Button::with_label("Copy URL");
        {
            let u = Rc::clone(&url_full);
            let act_c = Rc::clone(&on_action);
            copy_url_btn.connect_clicked(move |_| {
                let s = u.borrow().clone();
                if !s.is_empty() {
                    act_c(DetailAction::CopyText(s));
                }
            });
        }
        url_btn_row.append(&open_url_btn);
        url_btn_row.append(&copy_url_btn);
        url_box.append(&url_btn_row);
        stack.add_named(&url_box, Some("url"));

        // 5. Image page
        let img_box = GtkBox::new(Orientation::Vertical, 10);
        img_box.set_margin_top(12);
        img_box.set_margin_bottom(12);
        img_box.set_margin_start(12);
        img_box.set_margin_end(12);

        let picture = Picture::new();
        picture.set_content_fit(ContentFit::Contain);
        picture.set_vexpand(true);
        picture.set_hexpand(true);
        img_box.append(&picture);

        let image_info = Label::new(None);
        image_info.set_xalign(0.0);
        image_info.add_css_class("pf-clip-meta");
        img_box.append(&image_info);

        let img_btns = GtkBox::new(Orientation::Horizontal, 8);
        let save_btn = Button::with_label("Save to Downloads");
        {
            let id_c = Rc::clone(&current_clip_id);
            let act_c = Rc::clone(&on_action);
            save_btn.connect_clicked(move |_| {
                if let Some(id) = id_c.get() {
                    act_c(DetailAction::SaveImageToDownloads(id));
                }
            });
        }
        let open_img_btn = Button::with_label("Open in Viewer");
        {
            let id_c = Rc::clone(&current_clip_id);
            let act_c = Rc::clone(&on_action);
            open_img_btn.connect_clicked(move |_| {
                if let Some(id) = id_c.get() {
                    act_c(DetailAction::OpenImageInViewer(id));
                }
            });
        }
        img_btns.append(&save_btn);
        img_btns.append(&open_img_btn);
        img_box.append(&img_btns);
        stack.add_named(&img_box, Some("image"));

        root.append(&stack);

        // Metadata footer
        root.append(&Separator::new(Orientation::Horizontal));
        let meta_label = Label::new(Some(""));
        meta_label.set_xalign(0.0);
        meta_label.set_wrap(true);
        meta_label.set_margin_start(12);
        meta_label.set_margin_end(12);
        meta_label.set_margin_top(6);
        meta_label.set_margin_bottom(6);
        meta_label.add_css_class("pf-clip-meta");
        root.append(&meta_label);

        Self {
            root,
            stack,
            header_kind,
            lang_dropdown,
            source_buffer,
            color_rgb,
            color_swatch,
            hex_val,
            rgb_val,
            hsl_val,
            url_host,
            url_scheme,
            url_path,
            url_full,
            picture,
            image_info,
            meta_label,
            current_clip_id,
            suppress_lang_signal,
            store,
        }
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    pub fn clear(&self) {
        self.current_clip_id.set(None);
        self.header_kind.set_text("DETAILS");
        self.lang_dropdown.set_visible(false);
        self.meta_label.set_text("");
        self.picture.set_paintable(None::<&gdk::Paintable>);
        self.stack.set_visible_child_name("empty");
    }

    pub fn show_clip(&self, clip: &ClipRecord) {
        self.current_clip_id.set(Some(clip.id));
        self.header_kind.set_text(&clip.kind.as_str().to_ascii_uppercase());

        let rel = format_relative_time(clip.last_used_at, now_unix_ms());
        let size = format_bytes(clip.byte_size);
        let src = clip.source_app.as_deref().unwrap_or("unknown");
        self.meta_label.set_text(&format!(
            "Size: {size} · Source: {src} · Copied: {}× · Updated: {rel}",
            clip.copy_count
        ));

        match clip.kind {
            ClipKind::Color => {
                self.lang_dropdown.set_visible(false);
                if let Some(col) = parse_color_from_clip(clip) {
                    self.color_rgb.set((
                        f64::from(col.r) / 255.0,
                        f64::from(col.g) / 255.0,
                        f64::from(col.b) / 255.0,
                        f64::from(col.a) / 255.0,
                    ));
                    self.color_swatch.queue_draw();
                    self.hex_val.set_text(&col.hex);
                    self.rgb_val.set_text(&col.rgb);
                    self.hsl_val.set_text(&col.hsl);
                    self.stack.set_visible_child_name("color");
                    return;
                }
            }
            ClipKind::Url => {
                self.lang_dropdown.set_visible(false);
                let full_text = self.load_primary_text(clip.id).unwrap_or_else(|| clip.preview.clone());
                let trimmed = full_text.trim().to_string();
                let (scheme, host, path) = extract_url_parts(clip, &trimmed);
                self.url_host.set_text(&host);
                self.url_scheme.set_text(&format!("Scheme: {scheme}"));
                self.url_path.set_text(&format!("URL: {trimmed}\nPath: {path}"));
                *self.url_full.borrow_mut() = trimmed;
                self.stack.set_visible_child_name("url");
                return;
            }
            ClipKind::Image => {
                self.lang_dropdown.set_visible(false);
                self.picture.set_paintable(None::<&gdk::Paintable>);
                if let Ok(formats) = self.store.borrow().get_clip_formats(clip.id)
                    && let Some(img_fmt) = formats.iter().find(|f| f.mime.starts_with("image/"))
                {
                    let bytes = gtk4::glib::Bytes::from(&img_fmt.data);
                    if let Ok(tex) = gdk::Texture::from_bytes(&bytes) {
                        self.picture.set_paintable(Some(&tex));
                    }
                }
                if let Some(ref im) = clip.image {
                    self.image_info.set_text(&format!(
                        "{} × {} px · {} · {}",
                        im.width,
                        im.height,
                        im.original_mime,
                        format_bytes(clip.byte_size)
                    ));
                }
                self.stack.set_visible_child_name("image");
                return;
            }
            ClipKind::Files => {
                self.lang_dropdown.set_visible(false);
                let display = if let Ok(formats) = self.store.borrow().get_clip_formats(clip.id)
                    && let Some(f) = formats.first()
                {
                    parse_file_list(&f.mime, &f.data).join("\n")
                } else {
                    clip.preview.clone()
                };
                self.source_buffer.set_language(None::<&sourceview5::Language>);
                self.source_buffer.set_text(&display);
                self.stack.set_visible_child_name("code");
                return;
            }
            _ => {}
        }

        // Default: Code / Text / Email / Phone / Number in GtkSourceView
        let text = self
            .load_primary_text(clip.id)
            .unwrap_or_else(|| clip.preview.clone());
        let lang_id = extract_lang(clip);
        self.apply_language(&lang_id);
        self.source_buffer.set_text(&text);
        self.lang_dropdown.set_visible(true);
        self.stack.set_visible_child_name("code");
    }

    fn apply_language(&self, lang_id: &str) {
        self.suppress_lang_signal.set(true);
        let idx = LANG_OPTIONS
            .iter()
            .position(|(_, id)| *id == lang_id)
            .unwrap_or(0);
        self.lang_dropdown.set_selected(idx as u32);
        self.suppress_lang_signal.set(false);

        if lang_id.is_empty() {
            self.source_buffer.set_language(None::<&sourceview5::Language>);
        } else {
            let lm = sourceview5::LanguageManager::default();
            let lang = lm.language(lang_id);
            self.source_buffer.set_language(lang.as_ref());
        }
    }

    fn load_primary_text(&self, clip_id: i64) -> Option<String> {
        let formats = self.store.borrow().get_clip_formats(clip_id).ok()?;
        let fmt = formats
            .iter()
            .find(|f| f.mime.starts_with("text/plain") || f.mime == "UTF8_STRING")
            .or_else(|| formats.first())?;
        Some(String::from_utf8_lossy(&fmt.data).into_owned())
    }
}

fn build_copy_field(label: &str, on_action: Rc<dyn Fn(DetailAction)>) -> (GtkBox, Label) {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    let key_lbl = Label::new(Some(label));
    key_lbl.set_width_chars(4);
    key_lbl.set_xalign(0.0);
    key_lbl.add_css_class("pf-kind-badge");
    row.append(&key_lbl);

    let val_lbl = Label::new(None);
    val_lbl.set_xalign(0.0);
    val_lbl.set_hexpand(true);
    val_lbl.set_selectable(true);
    row.append(&val_lbl);

    let btn = Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text(&format!("Copy {label}"))
        .has_frame(false)
        .build();
    let val_c = val_lbl.clone();
    btn.connect_clicked(move |_| {
        let text = val_c.text().to_string();
        if !text.is_empty() {
            on_action(DetailAction::CopyText(text));
        }
    });
    row.append(&btn);
    (row, val_lbl)
}

fn parse_color_from_clip(clip: &ClipRecord) -> Option<ParsedColor> {
    if let Some(ref meta) = clip.meta
        && let Ok(parsed) = serde_json::from_str::<ParsedColor>(meta)
    {
        return Some(parsed);
    }
    pf_core::detect::color::parse_color(&clip.preview)
}

fn extract_lang(clip: &ClipRecord) -> String {
    if let Some(ref meta) = clip.meta
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(meta)
        && let Some(lang) = v.get("lang").and_then(|l| l.as_str())
    {
        return lang.to_string();
    }
    String::new()
}

fn extract_url_parts(clip: &ClipRecord, raw: &str) -> (String, String, String) {
    if let Some(ref meta) = clip.meta
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(meta)
    {
        let scheme = v.get("scheme").and_then(|s| s.as_str()).unwrap_or("https");
        let host = v.get("host").and_then(|s| s.as_str()).unwrap_or(raw);
        let path = v.get("path").and_then(|s| s.as_str()).unwrap_or("/");
        return (scheme.to_string(), host.to_string(), path.to_string());
    }
    ("https".to_string(), raw.to_string(), "/".to_string())
}

/// Launch a file or URL with the system default handler via `gio::AppInfo`.
pub fn launch_default_uri(uri: &str) -> anyhow::Result<()> {
    gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>)?;
    Ok(())
}
