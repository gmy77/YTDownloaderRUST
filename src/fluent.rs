//! Aspetto Windows 11 (Fluent / WinUI 3).
//!
//! I colori sono i token di WinUI (stessi nomi delle risorse XAML, dal design
//! system "WinUI Gallery"), l'accento è quello scelto dall'utente in Windows,
//! lo sfondo della finestra è Mica. Font (Segoe UI Variable) e icone (Segoe
//! Fluent Icons) sono letti dall'installazione di Windows, non inclusi nell'exe.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, Pos2, Rect, Response, RichText, Sense, Shape, Stroke,
    StrokeKind, Vec2, WidgetText,
};
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------- icone

/// Glifo nei due font: Segoe Fluent Icons (Windows 11 / MDL2 su Windows 10)
/// e Material Icons incluso nell'exe come riserva.
#[derive(Clone, Copy)]
pub struct Ico(pub &'static str, pub &'static str);

pub const I_SEARCH: Ico = Ico("\u{e721}", "\u{e8b6}");
pub const I_SETTINGS: Ico = Ico("\u{e713}", "\u{e8b8}");
pub const I_AUTH: Ico = Ico("\u{e8d7}", "\u{f02e}");
pub const I_UPDATE: Ico = Ico("\u{e895}", "\u{e923}");
pub const I_PERSON: Ico = Ico("\u{e77b}", "\u{e7ff}");
pub const I_CLOCK: Ico = Ico("\u{e823}", "\u{e8b5}");
pub const I_CHECK: Ico = Ico("\u{e73e}", "\u{e5ca}");
pub const I_CLOSE: Ico = Ico("\u{e711}", "\u{e5cd}");
pub const I_FOLDER: Ico = Ico("\u{e8b7}", "\u{e2c8}");
pub const I_PLAY: Ico = Ico("\u{e768}", "\u{e039}");
pub const I_CANCEL: Ico = Ico("\u{e711}", "\u{e888}");
pub const I_RESTART: Ico = Ico("\u{e72c}", "\u{f053}");
pub const I_FIND: Ico = Ico("\u{e838}", "\u{e880}");
pub const I_EMPTY: Ico = Ico("\u{e896}", "\u{f06a}");
pub const I_AUTH_OK: Ico = Ico("\u{e930}", "\u{e174}");
pub const I_BUG: Ico = Ico("\u{ebe8}", "\u{e868}");
pub const I_INFO: Ico = Ico("\u{e946}", "\u{e88f}");
pub const I_ERROR: Ico = Ico("\u{e783}", "\u{e000}");
pub const I_CHEVRON_DOWN: Ico = Ico("\u{e70d}", "\u{e5cf}");
pub const I_CHEVRON_UP: Ico = Ico("\u{e70e}", "\u{e5ce}");

static FLUENT_ICONS: AtomicBool = AtomicBool::new(false);

pub fn icon(i: Ico, size: f32, color: Color32) -> RichText {
    let code = if FLUENT_ICONS.load(Ordering::Relaxed) { i.0 } else { i.1 };
    RichText::new(code).family(FontFamily::Name("icons".into())).size(size).color(color)
}

// ---------------------------------------------------------------- font

/// Segoe UI Variable (Windows 11) o Segoe UI; semibold per i titoli;
/// Segoe Fluent Icons (o MDL2 Assets) per le icone.
pub fn setup_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions};
    use std::sync::Arc;
    let mut fonts = FontDefinitions::default();
    let base = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut add = |key: &str, files: &[&str]| -> bool {
        for file in files {
            if let Ok(bytes) = std::fs::read(format!("C:\\Windows\\Fonts\\{file}")) {
                fonts.font_data.insert(key.into(), Arc::new(FontData::from_owned(bytes)));
                return true;
            }
        }
        false
    };
    let regular = add("segoe", &["SegUIVar.ttf", "segoeui.ttf"]);
    let semibold = add("segoe_semibold", &["seguisb.ttf", "segoeuib.ttf"]);
    let fluent = add("fluent_icons", &["SegoeIcons.ttf", "segmdl2.ttf"]);
    fonts.font_data.insert("material".into(), Arc::new(FontData::from_static(include_bytes!("../assets/MaterialIcons-Regular.ttf"))));
    FLUENT_ICONS.store(fluent, Ordering::Relaxed);
    let with = |first: Option<&str>| -> Vec<String> {
        let mut v: Vec<String> = first.into_iter().map(str::to_string).collect();
        if regular {
            v.push("segoe".into());
        }
        v.extend(base.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with(None));
    fonts.families.insert(FontFamily::Name("bold".into()), with(semibold.then_some("segoe_semibold")));
    fonts.families.insert(FontFamily::Name("icons".into()), vec![if fluent { "fluent_icons".into() } else { "material".into() }]);
    ctx.set_fonts(fonts);
}

/// Semibold (600): l'unico peso oltre al normale usato da Fluent.
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

// ---------------------------------------------------------------- colori

/// Le sette tonalità dell'accento di Windows (Light3 … Dark3).
#[derive(Clone, Copy)]
pub struct Accent {
    pub light3: Color32,
    pub light2: Color32,
    pub dark1: Color32,
    pub dark2: Color32,
}

impl Accent {
    /// Blu predefinito di Windows 11 (#0078D4).
    fn default_blue() -> Self {
        Self { light3: hex(0x99EBFFFF), light2: hex(0x4CC2FFFF), dark1: hex(0x0067C0FF), dark2: hex(0x003E92FF) }
    }

    /// Legge la palette dell'accento scelto dall'utente
    /// (`HKCU\…\Explorer\Accent\AccentPalette`: 8 colori RGBx).
    pub fn from_system() -> Self {
        #[cfg(windows)]
        if let Some(b) = read_accent_palette() {
            let c = |i: usize| Color32::from_rgb(b[i * 4], b[i * 4 + 1], b[i * 4 + 2]);
            return Self { light3: c(0), light2: c(1), dark1: c(4), dark2: c(5) };
        }
        Self::default_blue()
    }
}

#[cfg(windows)]
fn read_accent_palette() -> Option<[u8; 32]> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent");
    let value = wide("AccentPalette");
    let mut buf = [0u8; 32];
    let mut len = buf.len() as u32;
    // SAFETY: buffer e lunghezza coerenti, stringhe terminate da zero.
    let rc = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_BINARY, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len)
    };
    (rc == 0 && len >= 24).then_some(buf)
}

/// `0xRRGGBBAA` → colore.
fn hex(v: u32) -> Color32 {
    rgba((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Alfa premoltiplicato nello spazio gamma, come fa XAML: `from_rgba_unmultiplied`
/// di egui premoltiplica in spazio lineare e schiarisce troppo i bianchi traslucidi.
fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    let m = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
    Color32::from_rgba_premultiplied(m(r), m(g), m(b), a)
}

fn with_alpha(c: Color32, a: u8) -> Color32 {
    let [r, g, b, _] = c.to_array();
    rgba(r, g, b, a)
}

/// Token WinUI per il tema corrente (valori di `Common_themeresources_any.xaml`).
#[derive(Clone, Copy)]
pub struct Pal {
    pub dark: bool,
    /// Sfondo finestra: trasparente sopra Mica, altrimenti `SolidBackgroundFillColorBase`.
    pub window: Color32,
    pub base: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub text3: Color32,
    pub text_disabled: Color32,
    pub accent_text: Color32,
    pub on_accent: Color32,
    pub on_accent2: Color32,
    pub on_accent_disabled: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub control_pressed: Color32,
    pub control_disabled: Color32,
    pub control_input_active: Color32,
    pub control_alt2: Color32,
    pub control_solid: Color32,
    pub stroke: Color32,
    pub stroke2: Color32,
    pub stroke_on_accent: Color32,
    pub stroke_on_accent2: Color32,
    pub strong_stroke: Color32,
    pub subtle_hover: Color32,
    pub subtle_pressed: Color32,
    pub card: Color32,
    pub card2: Color32,
    pub card_stroke: Color32,
    pub surface_stroke: Color32,
    pub flyout: Color32,
    pub flyout_stroke: Color32,
    pub divider: Color32,
    pub layer_alt: Color32,
    pub smoke: Color32,
    pub success: Color32,
    pub caution: Color32,
    pub critical: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    pub accent_disabled: Color32,
    pub focus_outer: Color32,
    pub focus_inner: Color32,
}

pub fn pal(dark: bool, accent: &Accent, mica: bool) -> Pal {
    let (l, d) = (|v: u32| hex(v), |v: u32| hex(v));
    let pick = |light: u32, dark_v: u32| if dark { d(dark_v) } else { l(light) };
    let accent_fill = if dark { accent.light2 } else { accent.dark1 };
    let base = pick(0xF3F3F3FF, 0x202020FF);
    Pal {
        dark,
        window: if mica { Color32::TRANSPARENT } else { base },
        base,
        text: pick(0x000000E4, 0xFFFFFFFF),
        text2: pick(0x0000009E, 0xFFFFFFC5),
        text3: pick(0x00000072, 0xFFFFFF87),
        text_disabled: pick(0x0000005C, 0xFFFFFF5D),
        accent_text: if dark { accent.light3 } else { accent.dark2 },
        on_accent: pick(0xFFFFFFFF, 0x000000FF),
        on_accent2: pick(0xFFFFFFB3, 0x00000080),
        on_accent_disabled: pick(0xFFFFFFFF, 0xFFFFFF87),
        control: pick(0xFFFFFFB3, 0xFFFFFF0F),
        control_hover: pick(0xF9F9F980, 0xFFFFFF15),
        control_pressed: pick(0xF9F9F94D, 0xFFFFFF08),
        control_disabled: pick(0xF9F9F94D, 0xFFFFFF0B),
        control_input_active: pick(0xFFFFFFFF, 0x1E1E1EB3),
        control_alt2: pick(0x00000006, 0x00000019),
        control_solid: pick(0xFFFFFFFF, 0x454545FF),
        stroke: pick(0x0000000F, 0xFFFFFF12),
        stroke2: pick(0x00000029, 0xFFFFFF18),
        stroke_on_accent: pick(0xFFFFFF14, 0xFFFFFF14),
        stroke_on_accent2: pick(0x00000066, 0x00000023),
        strong_stroke: pick(0x00000072, 0xFFFFFF8B),
        subtle_hover: pick(0x00000009, 0xFFFFFF0F),
        subtle_pressed: pick(0x00000006, 0xFFFFFF0A),
        card: pick(0xFFFFFFB3, 0xFFFFFF0D),
        card2: pick(0xF6F6F680, 0xFFFFFF08),
        card_stroke: pick(0x0000000F, 0x00000019),
        surface_stroke: pick(0x75757566, 0x75757566),
        // AcrylicBackgroundFillColorDefault (fallback solido dell'acrilico).
        flyout: pick(0xF9F9F9FF, 0x2C2C2CFF),
        flyout_stroke: pick(0x0000000F, 0x00000033),
        divider: pick(0x0000000F, 0xFFFFFF15),
        layer_alt: pick(0xFFFFFFFF, 0xFFFFFF0D),
        smoke: hex(0x0000004D),
        success: pick(0x0F7B0FFF, 0x6CCB5FFF),
        caution: pick(0x9D5D00FF, 0xFCE100FF),
        critical: pick(0xC42B1CFF, 0xFF99A4FF),
        accent: accent_fill,
        accent_hover: with_alpha(accent_fill, 0xE6),
        accent_pressed: with_alpha(accent_fill, 0xCC),
        accent_disabled: pick(0x00000037, 0xFFFFFF28),
        focus_outer: pick(0x000000E4, 0xFFFFFFFF),
        focus_inner: pick(0xFFFFFFB3, 0x000000B3),
    }
}

/// Stile egui ricavato dai token: controlli, popup, menu, barre di scorrimento.
pub fn style(dark: bool, accent: &Accent, mica: bool) -> egui::Style {
    use egui::style::{ScrollStyle, TextStyle, WidgetVisuals, Widgets};
    let p = pal(dark, accent, mica);
    let mut s = egui::Style::default();
    s.text_styles = [
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Heading, bold(20.0)),
        (TextStyle::Monospace, FontId::monospace(12.0)),
    ]
    .into();
    s.interaction.selectable_labels = false;
    s.spacing.button_padding = Vec2::new(11.0, 5.0);
    s.spacing.interact_size.y = 32.0;
    s.spacing.menu_margin = Margin::same(4);
    s.spacing.scroll = ScrollStyle::thin();
    s.spacing.combo_height = 360.0;

    let w = |bg: Color32, stroke: Color32, fg: Color32| WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: Stroke::new(1.0_f32, stroke),
        corner_radius: CornerRadius::same(4),
        fg_stroke: Stroke::new(1.0_f32, fg),
        expansion: 0.0,
    };
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.widgets = Widgets {
        noninteractive: w(p.card, p.divider, p.text),
        inactive: w(p.control, p.stroke2, p.text),
        hovered: w(p.control_hover, p.stroke2, p.text),
        active: w(p.control_pressed, p.stroke, p.text2),
        open: w(p.control_pressed, p.stroke, p.text),
    };
    v.override_text_color = Some(p.text);
    v.panel_fill = p.window;
    v.window_fill = p.flyout;
    v.window_stroke = Stroke::new(1.0_f32, p.flyout_stroke);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 32, spread: 0, color: Color32::from_black_alpha(if dark { 0x60 } else { 0x38 }) };
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if dark { 0x48 } else { 0x24 }) };
    v.extreme_bg_color = p.control;
    v.faint_bg_color = p.subtle_hover;
    v.code_bg_color = p.control;
    v.hyperlink_color = p.accent_text;
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0_f32, p.on_accent);
    v.text_cursor.stroke = Stroke::new(1.5_f32, p.text);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.striped = false;
    s.visuals = v;
    s
}

// ---------------------------------------------------------------- finestra (DWM)

/// Barra del titolo chiara/scura come il tema, angoli arrotondati e, su
/// Windows 11 22H2+, sfondo Mica. Restituisce `true` se Mica è attivo.
#[cfg(windows)]
pub fn apply_window_style(hwnd: isize, dark: bool, try_mica: bool) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute};
    use windows_sys::Win32::UI::Controls::MARGINS;
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMWCP_ROUND: i32 = 2;
    const DWMSBT_MAINWINDOW: i32 = 2;
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    let set = |attr: u32, value: i32| -> bool {
        // SAFETY: hwnd valido per la vita della finestra, valore i32 come da documentazione DWM.
        unsafe { DwmSetWindowAttribute(hwnd, attr, (&value as *const i32).cast(), 4) >= 0 }
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, dark as i32);
    set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
    if !try_mica {
        return false;
    }
    let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
    // SAFETY: come sopra.
    unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
    set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW)
}

#[cfg(not(windows))]
pub fn apply_window_style(_hwnd: isize, _dark: bool, _try_mica: bool) -> bool {
    false
}

// ---------------------------------------------------------------- controlli

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Standard,
    Accent,
    Subtle,
}

/// Bordo "elevation": filo da 1px, più marcato in basso (chiaro) o in alto (scuro).
fn elevation_border(painter: &egui::Painter, rect: Rect, light: Color32, strong: Color32, dark: bool) {
    painter.rect_stroke(rect, 4, Stroke::new(1.0_f32, light), StrokeKind::Inside);
    let y = if dark { rect.top() + 0.5 } else { rect.bottom() - 0.5 };
    painter.hline(rect.left() + 3.0..=rect.right() - 3.0, y, Stroke::new(1.0_f32, strong));
}

fn focus_ring(painter: &egui::Painter, rect: Rect, p: &Pal, radius: u8) {
    painter.rect_stroke(rect.expand(3.0), radius + 3, Stroke::new(2.0_f32, p.focus_outer), StrokeKind::Inside);
    painter.rect_stroke(rect.expand(1.0), radius + 1, Stroke::new(1.0_f32, p.focus_inner), StrokeKind::Inside);
}

/// Pulsante WinUI: altezza 32, padding 11/5/11/6, raggio 4. `width` 0 = adatta al testo.
pub fn button(ui: &mut egui::Ui, p: &Pal, text: &str, kind: Kind, width: f32, enabled: bool) -> Response {
    let galley = WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Truncate), (width - 22.0).max(0.0).max(if width == 0.0 { f32::INFINITY } else { 0.0 }), egui::TextStyle::Button);
    let w = if width > 0.0 { width } else { galley.size().x + 22.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 32.0), if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let pressed = enabled && resp.is_pointer_button_down_on();
        let hovered = enabled && resp.hovered();
        let (fill, fg) = match kind {
            Kind::Accent if !enabled => (p.accent_disabled, p.on_accent_disabled),
            Kind::Accent if pressed => (p.accent_pressed, p.on_accent2),
            Kind::Accent if hovered => (p.accent_hover, p.on_accent),
            Kind::Accent => (p.accent, p.on_accent),
            _ if !enabled => (if kind == Kind::Subtle { Color32::TRANSPARENT } else { p.control_disabled }, p.text_disabled),
            Kind::Subtle if pressed => (p.subtle_pressed, p.text2),
            Kind::Subtle if hovered => (p.subtle_hover, p.text),
            Kind::Subtle => (Color32::TRANSPARENT, p.text),
            _ if pressed => (p.control_pressed, p.text2),
            _ if hovered => (p.control_hover, p.text),
            _ => (p.control, p.text),
        };
        let painter = ui.painter();
        painter.rect_filled(rect, 4, fill);
        match kind {
            Kind::Accent if enabled && !pressed => elevation_border(painter, rect, p.stroke_on_accent, p.stroke_on_accent2, p.dark),
            Kind::Standard if enabled && !pressed => elevation_border(painter, rect, p.stroke, p.stroke2, p.dark),
            Kind::Standard => {
                painter.rect_stroke(rect, 4, Stroke::new(1.0_f32, p.stroke), StrokeKind::Inside);
            }
            _ => {}
        }
        let pos = rect.center() - galley.size() / 2.0 + Vec2::new(0.0, 0.0);
        painter.galley(pos, galley, fg);
        if resp.has_focus() {
            focus_ring(painter, rect, p, 4);
        }
    }
    resp
}

/// Pulsante "subtle" con sola icona (barra comandi, azioni di riga).
pub fn icon_button(ui: &mut egui::Ui, p: &Pal, ico: Ico, size: Vec2, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let pressed = enabled && resp.is_pointer_button_down_on();
        let fill = if pressed {
            p.subtle_pressed
        } else if enabled && resp.hovered() {
            p.subtle_hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 4, fill);
        let color = if !enabled {
            p.text_disabled
        } else if pressed {
            p.text2
        } else {
            p.text
        };
        let galley = WidgetText::from(icon(ico, 16.0, color)).into_galley(ui, None, f32::INFINITY, egui::TextStyle::Body);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
        if resp.has_focus() {
            focus_ring(ui.painter(), rect, p, 4);
        }
    }
    resp
}

/// Contenitore TextBox: riempimento controllo, bordo inferiore forte che diventa
/// accento da 2px con il focus. `add` disegna il TextEdit (senza cornice).
pub fn text_box<R>(ui: &mut egui::Ui, p: &Pal, enabled: bool, add: impl FnOnce(&mut egui::Ui) -> (Response, R)) -> (Response, R) {
    let bg = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::new().inner_margin(Margin { left: 10, right: 6, top: 5, bottom: 6 }).show(ui, |ui| add(ui));
    let rect = inner.response.rect;
    let (resp, r) = inner.inner;
    let focused = resp.has_focus();
    let hovered = ui.rect_contains_pointer(rect);
    let fill = if !enabled {
        p.control_disabled
    } else if focused {
        p.control_input_active
    } else if hovered {
        p.control_hover
    } else {
        p.control
    };
    ui.painter().set(bg, egui::epaint::RectShape::new(rect, 4, fill, Stroke::new(1.0_f32, p.stroke), StrokeKind::Inside));
    if enabled && focused {
        let r = Rect::from_min_max(Pos2::new(rect.left() + 1.0, rect.bottom() - 2.0), Pos2::new(rect.right() - 1.0, rect.bottom()));
        ui.painter().rect_filled(r, CornerRadius { nw: 0, ne: 0, sw: 3, se: 3 }, p.accent);
    } else if enabled {
        ui.painter().hline(rect.left() + 2.0..=rect.right() - 2.0, rect.bottom() - 0.5, Stroke::new(1.0_f32, p.strong_stroke));
    }
    (resp, r)
}

/// TextBox a riga singola con la larghezza data.
pub fn line_edit(ui: &mut egui::Ui, p: &Pal, value: &mut String, width: f32, hint: Option<&str>) -> Response {
    text_box(ui, p, true, |ui| {
        let mut te = egui::TextEdit::singleline(value).frame(false).desired_width(width - 16.0).margin(Margin::ZERO);
        if let Some(h) = hint {
            te = te.hint_text(RichText::new(h).color(p.text2));
        }
        (ui.add(te), ())
    })
    .0
}

/// ToggleSwitch WinUI: traccia 40×20, pomello 12 (14 al passaggio).
pub fn toggle(ui: &mut egui::Ui, p: &Pal, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(40.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.167);
    let hovered = resp.hovered();
    let painter = ui.painter();
    if *on {
        painter.rect_filled(rect, 10, if hovered { p.accent_hover } else { p.accent });
    } else {
        painter.rect(rect, 10, if hovered { p.control_hover } else { p.control_alt2 }, Stroke::new(1.0_f32, p.strong_stroke), StrokeKind::Inside);
    }
    let r = if hovered { 7.0 } else { 6.0 };
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, t);
    painter.circle_filled(Pos2::new(x, rect.center().y), r, if *on { p.on_accent } else { p.text2 });
    if resp.has_focus() {
        focus_ring(painter, rect, p, 10);
    }
    resp
}

/// CheckBox WinUI: casella 20, raggio 4, spunta 12.
pub fn check_box(ui: &mut egui::Ui, p: &Pal, checked: &mut bool, text: &str) -> Response {
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
            check_mark(ui, p, rect, *checked);
            ui.label(text);
        })
        .response
        .interact(Sense::click());
    if resp.clicked() {
        *checked = !*checked;
    }
    resp
}

/// Solo la casella (per le righe di lista a selezione multipla).
pub fn check_mark(ui: &egui::Ui, p: &Pal, rect: Rect, checked: bool) {
    let painter = ui.painter();
    if checked {
        painter.rect_filled(rect, 4, p.accent);
        let g = WidgetText::from(icon(I_CHECK, 12.0, p.on_accent)).into_galley(ui, None, f32::INFINITY, egui::TextStyle::Body);
        painter.galley(rect.center() - g.size() / 2.0, g, p.on_accent);
    } else {
        painter.rect(rect, 4, p.control_alt2, Stroke::new(1.0_f32, p.strong_stroke), StrokeKind::Inside);
    }
}

/// Slider WinUI: traccia 4px, parte piena in accento, cursore 20 con punto da 12.
pub fn slider(ui: &mut egui::Ui, p: &Pal, value: &mut u32, range: std::ops::RangeInclusive<u32>, width: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::click_and_drag());
    let (lo, hi) = (*range.start() as f32, *range.end() as f32);
    let track = rect.shrink2(Vec2::new(10.0, 0.0));
    if let Some(pos) = resp.interact_pointer_pos() {
        let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
        let v = (lo + t * (hi - lo)).round() as u32;
        if v != *value {
            *value = v;
            resp.mark_changed();
        }
    }
    if resp.has_focus() {
        let (l, r) = ui.input(|i| (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight)));
        if l && *value > *range.start() {
            *value -= 1;
        }
        if r && *value < *range.end() {
            *value += 1;
        }
    }
    let t = (*value as f32 - lo) / (hi - lo).max(1.0);
    let x = track.left() + t * track.width();
    let cy = rect.center().y;
    let painter = ui.painter();
    painter.rect_filled(Rect::from_min_max(Pos2::new(track.left(), cy - 2.0), Pos2::new(track.right(), cy + 2.0)), 2, p.strong_stroke);
    painter.rect_filled(Rect::from_min_max(Pos2::new(track.left(), cy - 2.0), Pos2::new(x, cy + 2.0)), 2, p.accent);
    painter.circle(Pos2::new(x, cy), 10.0, p.control_solid, Stroke::new(1.0_f32, p.stroke2));
    let inner = if resp.is_pointer_button_down_on() {
        5.0
    } else if resp.hovered() {
        7.0
    } else {
        6.0
    };
    painter.circle_filled(Pos2::new(x, cy), inner, p.accent);
    resp
}

/// ComboBox con lista in stile WinUI: chevron Fluent, voci con fondo
/// "subtle" e barretta di accento su quella scelta.
pub fn combo<R>(
    ui: &mut egui::Ui,
    p: &Pal,
    id: &str,
    width: f32,
    selected: impl Into<WidgetText>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let chevron_color = p.text2;
    let pal = *p;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(selected)
        .icon(move |ui, rect, _visuals, _open, _dir| {
            let g = WidgetText::from(icon(I_CHEVRON_DOWN, 12.0, chevron_color)).into_galley(ui, None, f32::INFINITY, egui::TextStyle::Body);
            ui.painter().galley(rect.center() - g.size() / 2.0, g, chevron_color);
        })
        .show_ui(ui, |ui| {
            let v = ui.visuals_mut();
            v.selection.bg_fill = pal.subtle_hover;
            v.selection.stroke = Stroke::new(1.0_f32, pal.text);
            v.widgets.hovered.weak_bg_fill = pal.subtle_hover;
            v.widgets.hovered.bg_fill = pal.subtle_hover;
            v.widgets.hovered.bg_stroke = Stroke::NONE;
            v.widgets.active.weak_bg_fill = pal.subtle_pressed;
            v.widgets.active.bg_stroke = Stroke::NONE;
            v.widgets.inactive.bg_stroke = Stroke::NONE;
            ui.spacing_mut().button_padding = Vec2::new(12.0, 7.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            add(ui)
        })
}

/// Voce di una lista ComboBox; restituisce la risposta della voce.
pub fn choice(ui: &mut egui::Ui, p: &Pal, selected: bool, text: impl Into<WidgetText>) -> Response {
    let resp = ui.add(egui::SelectableLabel::new(selected, text));
    if selected {
        let r = resp.rect;
        let pill = Rect::from_center_size(Pos2::new(r.left() + 1.5, r.center().y), Vec2::new(3.0, 16.0));
        ui.painter().rect_filled(pill, 2, p.accent);
    }
    resp
}

pub fn choice_value<V: PartialEq>(ui: &mut egui::Ui, p: &Pal, current: &mut V, value: V, text: impl Into<WidgetText>) {
    if choice(ui, p, *current == value, text).clicked() {
        *current = value;
    }
}

/// ProgressRing WinUI (indeterminato se `fraction` è `None`).
pub fn progress_ring(ui: &mut egui::Ui, p: &Pal, fraction: Option<f32>, time: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    let c = rect.center();
    let r = 8.0;
    let (start, sweep) = match fraction {
        Some(f) if f > 0.0 => {
            ui.painter().circle_stroke(c, r, Stroke::new(2.5_f32, p.divider));
            (-std::f32::consts::FRAC_PI_2, f * std::f32::consts::TAU)
        }
        _ => {
            ui.ctx().request_repaint();
            (time * 5.0, std::f32::consts::FRAC_PI_2 * 1.4)
        }
    };
    let n = 32;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = start + sweep * i as f32 / n as f32;
            c + Vec2::new(a.cos(), a.sin()) * r
        })
        .collect();
    ui.painter().add(Shape::line(pts, Stroke::new(2.5_f32, p.accent)));
}

/// ProgressBar WinUI: traccia da 1px, indicatore da 3px arrotondato.
pub fn progress_bar(ui: &egui::Ui, p: &Pal, rect: Rect, fraction: Option<f32>, time: f32) {
    let cy = rect.center().y;
    ui.painter().hline(rect.x_range(), cy, Stroke::new(1.0_f32, p.strong_stroke));
    let (a, b) = match fraction {
        Some(f) => (rect.left(), rect.left() + rect.width() * f.clamp(0.0, 1.0)),
        None => {
            ui.ctx().request_repaint();
            let x = rect.left() + ((time * 0.6).fract() * 1.4 - 0.3) * rect.width();
            (x.max(rect.left()), (x + rect.width() * 0.3).min(rect.right()))
        }
    };
    if b > a {
        ui.painter().rect_filled(Rect::from_min_max(Pos2::new(a, cy - 1.5), Pos2::new(b, cy + 1.5)), 2, p.accent);
    }
}

// ---------------------------------------------------------------- ContentDialog

/// Parte superiore di un ContentDialog: fondo `LayerFillColorAlt`, padding 24,
/// titolo in stile Subtitle (20/28 semibold).
pub fn dialog_body<R>(ui: &mut egui::Ui, p: &Pal, title: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(p.layer_alt)
        .corner_radius(CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 })
        .inner_margin(24)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(t) = title {
                ui.add(egui::Label::new(RichText::new(t).font(bold(20.0))).wrap());
                ui.add_space(12.0);
            }
            add(ui)
        })
        .inner
}

/// Fascia dei pulsanti di un ContentDialog: separatore, padding 24, colonne uguali.
/// `buttons`: (testo, tipo, abilitato). Restituisce l'indice del pulsante premuto.
pub fn dialog_footer(ui: &mut egui::Ui, p: &Pal, buttons: &[(&str, Kind, bool)], columns: usize) -> Option<usize> {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0, p.card_stroke);
    let mut clicked = None;
    egui::Frame::new().inner_margin(24).show(ui, |ui| {
        let cols = columns.max(buttons.len()).max(1);
        let w = (ui.available_width() - 8.0 * (cols - 1) as f32) / cols as f32;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for (i, (text, kind, enabled)) in buttons.iter().enumerate().rev() {
                if button(ui, p, text, *kind, w, *enabled).clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}
