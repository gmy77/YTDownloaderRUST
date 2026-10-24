#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! YTDownloaderRUST: scarica video, audio e sottotitoli da YouTube.
//! Interfaccia e comportamento ricalcano YoutubeDownloader di Oleksii Holub
//! (Tyrrrz, licenza MIT); il motore è yt-dlp, lanciato come processo esterno.

mod engine;
mod i18n;
mod update;

use eframe::egui::{
    self, text::LayoutJob, Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Frame, Key, Layout, Margin, RichText, Sense, Shape,
    Stroke, TextFormat, Vec2,
};
use egui_extras::{Column, TableBuilder};
use engine::{AuthSource, DownloadOption, Preference, QualityPreference, QueryKind, SubKind, SubLanguage, SubSpec, Video};
use i18n::{fmt, Language, Tr};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use update::Patch;

const APP_NAME: &str = "YTDownloaderRUST";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const ACCENT: Color32 = Color32::from_rgb(0xF9, 0xA8, 0x25);

// Icone (Material Icons di Google, licenza Apache 2.0).
const I_SEARCH: &str = "\u{e8b6}";
const I_ARROW: &str = "\u{e5c8}";
const I_SETTINGS: &str = "\u{e8b8}";
const I_AUTH: &str = "\u{f02e}";
const I_UPDATE: &str = "\u{e923}";
const I_PERSON: &str = "\u{e7ff}";
const I_CLOCK: &str = "\u{e8b5}";
const I_CHECK: &str = "\u{e5ca}";
const I_CLOSE: &str = "\u{e5cd}";
const I_FOLDER: &str = "\u{e2c8}";
const I_PLAY: &str = "\u{e039}";
const I_CANCEL: &str = "\u{e888}";
const I_RESTART: &str = "\u{f053}";
const I_FIND: &str = "\u{e880}";
const I_YOUTUBE: &str = "\u{f06a}";
const I_AUTH_OK: &str = "\u{e174}";
const I_BUG: &str = "\u{e868}";
const I_INFO: &str = "\u{e88f}";

fn main() -> eframe::Result {
    let icon = image::load_from_memory(include_bytes!("../assets/icon256.png")).map(|i| {
        let i = i.to_rgba8();
        egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() }
    });
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("{APP_NAME} v{VERSION}"))
        .with_inner_size([720.0, 620.0])
        .with_min_inner_size([600.0, 400.0]);
    if let Ok(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(APP_NAME, options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

// ---------------------------------------------------------------- impostazioni

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
enum ThemePref {
    System,
    Light,
    Dark,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Settings {
    theme: ThemePref,
    language: Language,
    auto_update: bool,
    persist_auth: bool,
    auth: AuthSource,
    ffmpeg_path: String,
    ytdlp_path: String,
    inject_langs: bool,
    inject_subs: bool,
    inject_tags: bool,
    skip_existing: bool,
    template: String,
    parallel: u32,
    last_container: String,
    last_quality: QualityPreference,
    last_sub_lang: String,
    last_sub_auto: bool,
    patch: Patch,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePref::System,
            language: Language::System,
            auto_update: true,
            persist_auth: true,
            auth: AuthSource::None,
            ffmpeg_path: String::new(),
            ytdlp_path: String::new(),
            inject_langs: true,
            inject_subs: true,
            inject_tags: true,
            skip_existing: false,
            template: "$title".into(),
            parallel: 2,
            last_container: "mp4".into(),
            last_quality: QualityPreference::Highest,
            last_sub_lang: "it".into(),
            last_sub_auto: true,
            patch: Patch::default(),
        }
    }
}

// ---------------------------------------------------------------- colori

struct Pal {
    header: Color32,
    card: Color32,
    body: Color32,
    divider: Color32,
    text: Color32,
    weak: Color32,
    ok: Color32,
    warn: Color32,
    fail: Color32,
}

/// Colori misurati sull'originale (tema scuro e chiaro).
fn pal(dark: bool) -> Pal {
    if dark {
        Pal {
            header: Color32::from_gray(0x30),
            card: Color32::from_gray(0x42),
            body: Color32::from_gray(0x42),
            divider: Color32::from_gray(0x59),
            text: Color32::from_gray(0xEE),
            weak: Color32::from_gray(0xA8),
            ok: Color32::from_rgb(144, 238, 144),
            warn: Color32::from_rgb(255, 165, 0),
            fail: Color32::from_rgb(255, 69, 0),
        }
    } else {
        Pal {
            header: Color32::from_gray(0x30),
            card: Color32::WHITE,
            body: Color32::WHITE,
            divider: Color32::from_gray(0xE0),
            text: Color32::from_gray(0x21),
            weak: Color32::from_gray(0x75),
            ok: Color32::from_rgb(0, 100, 0),
            warn: Color32::from_rgb(255, 140, 0),
            fail: Color32::from_rgb(139, 0, 0),
        }
    }
}

fn visuals(dark: bool) -> egui::Visuals {
    let p = pal(dark);
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.body;
    v.window_fill = p.card;
    v.extreme_bg_color = p.card;
    v.faint_bg_color = if dark { Color32::from_gray(0x4A) } else { Color32::from_gray(0xF5) };
    v.override_text_color = Some(p.text);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.text_cursor.stroke = Stroke::new(2.0_f32, ACCENT);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.divider);
    v
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let base = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut add = |key: &str, file: &str| -> bool {
        match std::fs::read(format!("C:\\Windows\\Fonts\\{file}")) {
            Ok(bytes) => {
                fonts.font_data.insert(key.into(), Arc::new(FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    // Segoe UI viene letto dall'installazione di Windows, non è incluso nell'exe.
    let regular = add("segoe", "segoeui.ttf");
    let bold = add("segoe_bold", "seguisb.ttf") || add("segoe_bold", "segoeuib.ttf");
    let light = add("segoe_light", "segoeuil.ttf");
    fonts.font_data.insert("material".into(), Arc::new(FontData::from_static(include_bytes!("../assets/MaterialIcons-Regular.ttf"))));
    let with = |first: Option<&str>| -> Vec<String> {
        let mut v: Vec<String> = first.into_iter().map(str::to_string).collect();
        if regular {
            v.push("segoe".into());
        }
        v.extend(base.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with(None));
    fonts.families.insert(FontFamily::Name("bold".into()), with(bold.then_some("segoe_bold")));
    fonts.families.insert(FontFamily::Name("light".into()), with(light.then_some("segoe_light")));
    fonts.families.insert(FontFamily::Name("icons".into()), vec!["material".into()]);
    ctx.set_fonts(fonts);
}

fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}
fn light(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("light".into()))
}
fn icon(code: &str, size: f32, color: Color32) -> RichText {
    RichText::new(code).family(FontFamily::Name("icons".into())).size(size).color(color)
}

/// `**grassetto**` → testo con i pezzi in grassetto.
fn md_job(text: &str, size: f32, color: Color32, base: FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (i, part) in text.split("**").enumerate() {
        let font = if i % 2 == 1 { bold(size) } else { FontId::new(size, base.family.clone()) };
        job.append(part, 0.0, TextFormat { font_id: font, color, ..Default::default() });
    }
    job
}

fn plain(text: &str) -> String {
    text.replace("**", "")
}

// ---------------------------------------------------------------- download

#[derive(Clone, Copy, PartialEq, Debug)]
enum Status {
    Enqueued,
    Started,
    Completed,
    Canceled,
    Failed,
}

struct DlState {
    status: Status,
    error: Option<String>,
    /// File effettivamente prodotto (per i sottotitoli può cambiare nome).
    path: PathBuf,
}

struct Download {
    seq: u64,
    video: Video,
    option: Option<DownloadOption>,
    pref: Option<Preference>,
    path: PathBuf,
    state: Arc<Mutex<DlState>>,
    progress: Arc<Mutex<engine::Progress>>,
    cancel: Arc<AtomicBool>,
    launched: bool,
}

impl Download {
    fn new(seq: u64, video: Video, option: Option<DownloadOption>, pref: Option<Preference>, path: PathBuf) -> Self {
        Self {
            seq,
            video,
            option,
            pref,
            state: Arc::new(Mutex::new(DlState { status: Status::Enqueued, error: None, path: path.clone() })),
            path,
            progress: Arc::new(Mutex::new(engine::Progress::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            launched: false,
        }
    }
    fn status(&self) -> Status {
        self.state.lock().unwrap().status
    }
    fn file_name(&self) -> String {
        let p = self.state.lock().unwrap().path.clone();
        p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    }
    fn is_subtitles(&self) -> bool {
        self.option.as_ref().is_some_and(DownloadOption::is_subtitles) || self.pref.as_ref().is_some_and(|p| p.subs.is_some())
    }
}

/// L'originale crea subito un file vuoto per "prenotare" il nome; se il
/// download non va a buon fine lo si elimina.
fn remove_placeholder(path: &PathBuf) {
    if std::fs::metadata(path).map(|m| m.len() == 0).unwrap_or(false) {
        let _ = std::fs::remove_file(path);
    }
}

// ---------------------------------------------------------------- finestre

struct SingleDlg {
    video: Video,
    options: Vec<DownloadOption>,
    selected: usize,
    sub_langs: Vec<SubLanguage>,
    /// 0 = tutte le lingue (solo quelle dell'autore), poi gli indici di `sub_langs` + 1.
    sub_lang: usize,
}

struct MultipleDlg {
    title: String,
    videos: Vec<Video>,
    selected: Vec<bool>,
    container: String,
    quality: QualityPreference,
    sub_lang: String,
    sub_auto: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum MsgAction {
    None,
    DownloadFfmpeg,
    DownloadYtdlp,
    OpenSettings,
}

struct MessageDlg {
    title: String,
    text: String,
    ok: String,
    cancel: Option<String>,
    on_ok: MsgAction,
    on_cancel: MsgAction,
}

enum Dialog {
    Single(SingleDlg),
    Multiple(MultipleDlg),
    Settings,
    Auth,
    Message(MessageDlg),
    Report(ReportDlg),
    About,
}

struct ReportDlg {
    description: String,
    include_url: bool,
    video_url: Option<String>,
    error: Option<String>,
    /// Versioni e sistema, raccolti all'apertura.
    environment: String,
}

const MULTI_SUB_LANGS: [(&str, &str); 7] = [
    ("all", ""),
    ("it", "Italiano"),
    ("en", "English"),
    ("es", "Español"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("pt", "Português"),
];

// ---------------------------------------------------------------- eventi dai thread

struct QueryOutcome {
    result: engine::QueryResult,
    options: Vec<DownloadOption>,
    sub_langs: Vec<SubLanguage>,
    errors: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum SnackAction {
    Restart,
}

enum Event {
    Query(Result<QueryOutcome, String>),
    Snack(String),
    SnackAction(String, SnackAction),
    Message(String, String),
    UpdateDone,
    Patch(Patch),
    ToolDownloaded,
}

struct Snack {
    text: String,
    action: Option<SnackAction>,
    until: Instant,
}

#[derive(Clone, Copy, PartialEq)]
enum SortCol {
    File,
    Status,
}

enum RowAction {
    ShowFile(usize),
    Open(usize),
    Cancel(usize),
    Restart(usize),
    CopyError(usize),
    Report(usize),
}

// ---------------------------------------------------------------- applicazione

struct App {
    settings: Settings,
    query: String,
    query_focused: bool,
    busy: bool,
    updating: bool,
    downloads: Vec<Download>,
    next_seq: u64,
    dialog: Option<Dialog>,
    snacks: VecDeque<Snack>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    started: bool,
    sort: Option<(SortCol, bool)>,
    applied_theme: Option<ThemePref>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, visuals(true));
        cc.egui_ctx.set_visuals_of(egui::Theme::Light, visuals(false));
        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();
        let (tx, rx) = channel();
        Self {
            settings,
            query: String::new(),
            query_focused: false,
            busy: false,
            updating: false,
            downloads: Vec::new(),
            next_seq: 0,
            dialog: None,
            snacks: VecDeque::new(),
            tx,
            rx,
            started: false,
            sort: None,
            applied_theme: None,
        }
    }

    fn tr(&self) -> &'static Tr {
        self.settings.language.strings()
    }

    fn tools(&self) -> Option<engine::Tools> {
        Some(engine::Tools {
            ytdlp: engine::resolve_ytdlp(&self.settings.ytdlp_path)?,
            ffmpeg: engine::resolve_ffmpeg(&self.settings.ffmpeg_path),
            auth: self.settings.auth.clone(),
            extra_args: update::safe_args(&self.settings.patch.ytdlp_args),
        })
    }

    fn snack(&mut self, text: impl Into<String>) {
        self.snacks.push_back(Snack { text: text.into(), action: None, until: Instant::now() + Duration::from_secs(5) });
    }

    fn message(&mut self, title: &str, text: &str) {
        self.dialog = Some(Dialog::Message(MessageDlg {
            title: title.into(),
            text: text.into(),
            ok: self.tr().close_button.into(),
            cancel: None,
            on_ok: MsgAction::None,
            on_cancel: MsgAction::None,
        }));
    }

    /// Come all'avvio dell'originale: se manca uno strumento, propone di scaricarlo.
    fn check_tools(&mut self) {
        let tr = self.tr();
        if engine::resolve_ytdlp(&self.settings.ytdlp_path).is_none() {
            self.dialog = Some(Dialog::Message(MessageDlg {
                title: tr.ytdlp_missing_title.into(),
                text: fmt(tr.ytdlp_missing_message, &[APP_NAME]),
                ok: tr.download_button.into(),
                cancel: Some(tr.close_button.into()),
                on_ok: MsgAction::DownloadYtdlp,
                on_cancel: MsgAction::OpenSettings,
            }));
        } else if engine::resolve_ffmpeg(&self.settings.ffmpeg_path).is_none() {
            self.dialog = Some(Dialog::Message(MessageDlg {
                title: tr.ffmpeg_missing_title.into(),
                text: fmt(tr.ffmpeg_missing_message, &[APP_NAME]),
                ok: tr.download_button.into(),
                cancel: Some(tr.close_button.into()),
                on_ok: MsgAction::DownloadFfmpeg,
                on_cancel: MsgAction::OpenSettings,
            }));
        }
    }

    fn run_action(&mut self, ctx: &egui::Context, action: MsgAction) {
        let tr = self.tr();
        let (tx, ctx2) = (self.tx.clone(), ctx.clone());
        match action {
            MsgAction::None => {}
            MsgAction::OpenSettings => self.dialog = Some(Dialog::Settings),
            MsgAction::DownloadYtdlp | MsgAction::DownloadFfmpeg => {
                let ytdlp = action == MsgAction::DownloadYtdlp;
                self.snack(if ytdlp { tr.ytdlp_downloading } else { tr.ffmpeg_downloading });
                self.updating = true;
                std::thread::spawn(move || {
                    let res = if ytdlp { update::download_ytdlp() } else { update::download_ffmpeg() };
                    let _ = tx.send(match res {
                        Ok(_) => Event::Snack(if ytdlp { tr.ytdlp_downloaded } else { tr.ffmpeg_downloaded }.into()),
                        Err(e) => Event::Message(tr.error_title.into(), e),
                    });
                    let _ = tx.send(Event::ToolDownloaded);
                    ctx2.request_repaint();
                });
            }
        }
    }

    /// I tre livelli di aggiornamento; `manual` = premuto il tasto Aggiorna.
    fn run_update(&mut self, ctx: &egui::Context, manual: bool) {
        if self.updating {
            return;
        }
        self.updating = true;
        let tr = self.tr();
        if manual {
            self.snack(tr.update_checking);
        }
        let (tx, ctx2) = (self.tx.clone(), ctx.clone());
        let custom_ytdlp = !self.settings.ytdlp_path.trim().is_empty();
        let current = engine::resolve_ytdlp(&self.settings.ytdlp_path);
        let known_revision = self.settings.patch.revision;
        std::thread::spawn(move || {
            let send = |e: Event| {
                let _ = tx.send(e);
                ctx2.request_repaint();
            };
            let mut changed = false;
            // 1. correzione rapida
            let mut min_ytdlp = String::new();
            if let Ok(patch) = update::fetch_patch() {
                min_ytdlp = patch.min_ytdlp.clone();
                if patch.revision != known_revision {
                    if !patch.message.is_empty() {
                        send(Event::Snack(fmt(tr.patch_applied, &[&patch.message])));
                    }
                    send(Event::Patch(patch));
                    changed = true;
                }
            }
            // 2. yt-dlp (non si tocca una copia scelta a mano nelle impostazioni)
            let mut version = current.as_deref().and_then(engine::ytdlp_version).unwrap_or_default();
            if !custom_ytdlp && current.is_some() {
                match update::update_ytdlp(current.as_deref()) {
                    Ok((before, after)) => {
                        if before != after {
                            send(Event::Snack(fmt(tr.ytdlp_updated, &[&after])));
                            changed = true;
                        }
                        version = after;
                    }
                    Err(e) => send(Event::Snack(fmt(tr.ytdlp_update_failed, &[&e]))),
                }
                if !min_ytdlp.is_empty() && version < min_ytdlp {
                    send(Event::Snack(fmt(tr.ytdlp_update_failed, &[&format!("{version} < {min_ytdlp}")])));
                }
            }
            // 3. l'app
            match update::check_app_update() {
                Ok(Some(v)) => {
                    send(Event::Snack(fmt(tr.update_downloading, &[APP_NAME, &v])));
                    match update::install_app_update() {
                        Ok(()) => send(Event::SnackAction(tr.update_ready.into(), SnackAction::Restart)),
                        Err(_) => send(Event::Snack(tr.update_failed.into())),
                    }
                    changed = true;
                }
                Ok(None) => {}
                Err(_) if !manual => {}
                Err(e) => send(Event::Snack(format!("{}: {e}", tr.update_failed))),
            }
            if manual && !changed {
                send(Event::Snack(fmt(tr.update_all_current, &[&version])));
            }
            send(Event::UpdateDone);
        });
    }

    fn process_query(&mut self, ctx: &egui::Context) {
        if self.busy || self.query.trim().is_empty() {
            return;
        }
        let Some(tools) = self.tools() else {
            self.check_tools();
            return;
        };
        self.busy = true;
        let lines: Vec<String> = self.query.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
        let include_langs = self.settings.inject_langs;
        let (tx, ctx2) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let outcome = (|| {
                let mut results = Vec::new();
                let mut errors = Vec::new();
                for line in &lines {
                    match engine::resolve(&tools, line) {
                        Ok(r) => results.push(r),
                        // Con più righe un errore non ferma le altre (come l'originale).
                        Err(e) if lines.len() > 1 => errors.push(e),
                        Err(e) => return Err(e),
                    }
                }
                if results.is_empty() {
                    return Err(errors.join("\n"));
                }
                let result = engine::aggregate(results);
                let (options, sub_langs) = if result.videos.len() == 1 {
                    let info = match &result.info {
                        Some(i) => i.clone(),
                        None => engine::video_info(&tools, &result.videos[0])?,
                    };
                    (engine::download_options(&info, include_langs), engine::subtitle_languages(&info))
                } else {
                    (Vec::new(), Vec::new())
                };
                Ok(QueryOutcome { result, options, sub_langs, errors })
            })();
            let _ = tx.send(Event::Query(outcome));
            ctx2.request_repaint();
        });
    }

    fn on_query(&mut self, outcome: Result<QueryOutcome, String>) {
        self.busy = false;
        let tr = self.tr();
        let out = match outcome {
            Ok(o) => o,
            Err(e) => {
                self.message(tr.error_title, &e);
                return;
            }
        };
        for e in &out.errors {
            self.snack(e.clone());
        }
        let r = out.result;
        match r.videos.len() {
            0 => self.message(tr.nothing_found_title, tr.nothing_found_message),
            1 => {
                let mut options = out.options;
                if options.is_empty() && out.sub_langs.is_empty() {
                    self.message(tr.error_title, tr.no_suitable_format);
                    return;
                }
                if !out.sub_langs.is_empty() {
                    for f in engine::SUBTITLE_FORMATS {
                        options.push(DownloadOption::subtitles(f, SubSpec { langs: String::new(), auto: false }));
                    }
                }
                let selected = options.iter().position(|o| o.container == self.settings.last_container).unwrap_or(0);
                let sub_lang = out
                    .sub_langs
                    .iter()
                    .position(|l| l.code == self.settings.last_sub_lang || l.code.starts_with(&format!("{}-", self.settings.last_sub_lang)))
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let video = r.videos.into_iter().next().unwrap();
                self.dialog = Some(Dialog::Single(SingleDlg { video, options, selected, sub_langs: out.sub_langs, sub_lang }));
            }
            n => {
                let title = match r.kind {
                    QueryKind::Playlist => fmt(tr.kind_playlist, &[&r.name]),
                    QueryKind::Channel => fmt(tr.kind_channel, &[&r.name]),
                    QueryKind::Search => fmt(tr.kind_search, &[&r.name]),
                    QueryKind::Aggregate => fmt(tr.kind_aggregate, &[&r.name]),
                    QueryKind::Video => r.name.clone(),
                };
                // Preselezionati se arrivano da una playlist o da un canale, non da una ricerca.
                let preselect = !matches!(r.kind, QueryKind::Search | QueryKind::Aggregate);
                self.dialog = Some(Dialog::Multiple(MultipleDlg {
                    title,
                    videos: r.videos,
                    selected: vec![preselect; n],
                    container: self.settings.last_container.clone(),
                    quality: self.settings.last_quality,
                    sub_lang: self.settings.last_sub_lang.clone(),
                    sub_auto: self.settings.last_sub_auto,
                }));
            }
        }
    }

    fn enqueue(&mut self, d: Download, position: usize) {
        let pos = position.min(self.downloads.len());
        self.downloads.insert(pos, d);
    }

    fn new_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// Avvia i download in attesa fino al limite di download paralleli.
    fn schedule(&mut self, ctx: &egui::Context) {
        let active = self.downloads.iter().filter(|d| d.launched && d.status() == Status::Started).count();
        let mut free = (self.settings.parallel as usize).saturating_sub(active);
        let mut waiting: Vec<usize> = (0..self.downloads.len())
            .filter(|&i| !self.downloads[i].launched && self.downloads[i].status() == Status::Enqueued)
            .collect();
        waiting.sort_by_key(|&i| self.downloads[i].seq);
        for i in waiting {
            if free == 0 {
                break;
            }
            self.start_download(ctx, i);
            free -= 1;
        }
    }

    fn start_download(&mut self, ctx: &egui::Context, i: usize) {
        let tools = self.tools();
        let tr = self.tr();
        let flags = engine::DownloadFlags { subtitles: self.settings.inject_subs, tags: self.settings.inject_tags };
        let include_langs = self.settings.inject_langs;
        let d = &mut self.downloads[i];
        d.launched = true;
        d.state.lock().unwrap().status = Status::Started;
        let (video, option, pref, path) = (d.video.clone(), d.option.clone(), d.pref.clone(), d.path.clone());
        let (state, progress, cancel, ctx2) = (d.state.clone(), d.progress.clone(), d.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = (|| {
                let tools = tools.ok_or_else(|| engine::DownloadError::Failed(tr.ytdlp_missing_title.into()))?;
                let option = match (option, pref) {
                    (Some(o), _) => o,
                    (None, Some(p)) if p.subs.is_some() => p.best(&[]).unwrap(),
                    (None, Some(p)) => {
                        let info = engine::video_info(&tools, &video).map_err(engine::DownloadError::Failed)?;
                        p.best(&engine::download_options(&info, include_langs))
                            .ok_or_else(|| engine::DownloadError::Failed(tr.no_suitable_format.into()))?
                    }
                    (None, None) => return Err(engine::DownloadError::Failed(tr.no_suitable_format.into())),
                };
                if cancel.load(Ordering::Relaxed) {
                    return Err(engine::DownloadError::Canceled);
                }
                engine::download(&tools, &video, &option, &path, &flags, &progress, &cancel)
            })();
            let mut st = state.lock().unwrap();
            match result {
                Ok(p) => {
                    st.status = Status::Completed;
                    st.path = p;
                }
                Err(engine::DownloadError::Canceled) => {
                    remove_placeholder(&path);
                    st.status = Status::Canceled;
                }
                Err(engine::DownloadError::Failed(e)) => {
                    remove_placeholder(&path);
                    st.status = Status::Failed;
                    st.error = Some(e);
                }
            }
            drop(st);
            ctx2.request_repaint();
        });
    }

    fn cancel(&mut self, i: usize) {
        let d = &self.downloads[i];
        d.cancel.store(true, Ordering::Relaxed);
        if !d.launched {
            d.state.lock().unwrap().status = Status::Canceled;
            remove_placeholder(&d.path);
        }
    }

    fn restart(&mut self, i: usize) {
        let old = self.downloads.remove(i);
        old.cancel.store(true, Ordering::Relaxed);
        if !old.is_subtitles() && !old.path.exists() {
            let _ = std::fs::write(&old.path, []);
        }
        let seq = self.new_seq();
        self.enqueue(Download::new(seq, old.video, old.option, old.pref, old.path), i);
    }

    fn remove_where(&mut self, f: impl Fn(Status) -> bool) {
        let mut i = 0;
        while i < self.downloads.len() {
            if f(self.downloads[i].status()) {
                self.cancel(i);
                self.downloads.remove(i);
            } else {
                i += 1;
            }
        }
    }

    fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Query(o) => self.on_query(o),
                Event::Snack(s) => self.snack(s),
                Event::SnackAction(s, a) => self.snacks.push_back(Snack { text: s, action: Some(a), until: Instant::now() + Duration::from_secs(30) }),
                Event::Message(t, m) => self.message(&t, &m),
                Event::UpdateDone => self.updating = false,
                Event::Patch(p) => self.settings.patch = p,
                Event::ToolDownloaded => {
                    self.updating = false;
                    if self.dialog.is_none() {
                        self.check_tools();
                    }
                }
            }
        }
        let _ = ctx;
    }

    // ------------------------------------------------------------ disegno

    fn header(&mut self, ctx: &egui::Context, p: &Pal) {
        let tr = self.tr();
        egui::TopBottomPanel::top("header")
            .frame(Frame::new().fill(p.header).inner_margin(Margin { left: 12, right: 8, top: 12, bottom: 12 }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let buttons = 5.0 * 44.0;
                    let card_w = (ui.available_width() - buttons).max(200.0);
                    let enabled = !self.busy;
                    ui.allocate_ui_with_layout(Vec2::new(card_w, 0.0), Layout::left_to_right(Align::Center), |ui| {
                        Frame::new()
                            .fill(p.card)
                            .corner_radius(4)
                            .inner_margin(Margin { left: 10, right: 6, top: 8, bottom: 8 })
                            .shadow(egui::Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_black_alpha(60) })
                            .show(ui, |ui| {
                                ui.set_width(card_w - 16.0);
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.label(icon(I_SEARCH, 24.0, p.text));
                                    ui.add_space(4.0);
                                    // Invio = elabora, Maiusc+Invio = nuova riga (come l'originale).
                                    let shift = ui.input(|i| i.modifiers.shift);
                                    let mut submit = false;
                                    if self.query_focused && !shift && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                                        submit = true;
                                    }
                                    let edit_w = ui.available_width() - 36.0;
                                    let resp = ui.add_enabled(
                                        enabled,
                                        egui::TextEdit::multiline(&mut self.query)
                                            .frame(false)
                                            .desired_rows(1)
                                            .desired_width(edit_w)
                                            .margin(Margin::symmetric(0, 2))
                                            .font(FontId::proportional(16.0))
                                            .hint_text(RichText::new(tr.query_placeholder).color(p.weak).size(16.0)),
                                    );
                                    self.query_focused = resp.has_focus();
                                    resp.on_hover_text(tr.query_tooltip);
                                    let can = enabled && !self.query.trim().is_empty();
                                    let go = ui.add_enabled(can, egui::Button::new(icon(I_ARROW, 24.0, p.weak)).frame(false)).on_hover_text(tr.process_query_tooltip);
                                    if submit || go.clicked() {
                                        self.process_query(ui.ctx());
                                    }
                                });
                            });
                    });
                    let white = Color32::WHITE;
                    let icon_btn = |ui: &mut egui::Ui, code: &str, tip: &str, on: bool| {
                        ui.add_enabled(on, egui::Button::new(icon(code, 24.0, white)).frame(false).min_size(Vec2::splat(36.0))).on_hover_text(tip).clicked()
                    };
                    if icon_btn(ui, I_INFO, tr.about_tooltip, true) {
                        self.dialog = Some(Dialog::About);
                    }
                    if icon_btn(ui, I_BUG, tr.report_tooltip, true) {
                        self.open_report(None, None);
                    }
                    if icon_btn(ui, I_UPDATE, tr.update_tooltip, !self.updating) {
                        self.run_update(ui.ctx(), true);
                    }
                    if icon_btn(ui, I_AUTH, tr.auth_tooltip, enabled) {
                        self.dialog = Some(Dialog::Auth);
                    }
                    if icon_btn(ui, I_SETTINGS, tr.settings_tooltip, enabled) {
                        self.dialog = Some(Dialog::Settings);
                    }
                });
            });

        // Barra di avanzamento da 2 px sotto l'intestazione.
        egui::TopBottomPanel::top("progress").exact_height(2.0).frame(Frame::new().fill(p.header)).show_separator_line(false).show(ctx, |ui| {
            let rect = ui.max_rect();
            let fractions: Vec<f32> = self
                .downloads
                .iter()
                .filter(|d| d.launched && d.status() == Status::Started)
                .map(|d| d.progress.lock().unwrap().fraction.unwrap_or(0.0))
                .collect();
            if self.busy || self.updating {
                let t = ui.input(|i| i.time) as f32;
                let x = rect.left() + ((t * 0.8).fract() * 1.4 - 0.2) * rect.width();
                let seg = egui::Rect::from_min_max(egui::pos2(x.max(rect.left()), rect.top()), egui::pos2((x + rect.width() * 0.25).min(rect.right()), rect.bottom()));
                ui.painter().rect_filled(seg, 0.0, ACCENT);
                ui.ctx().request_repaint();
            } else if !fractions.is_empty() {
                let f = fractions.iter().sum::<f32>() / fractions.len() as f32;
                let r = egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + rect.width() * f, rect.bottom()));
                ui.painter().rect_filled(r, 0.0, ACCENT);
            }
        });
    }

    fn body(&mut self, ctx: &egui::Context, p: &Pal) {
        let tr = self.tr();
        egui::CentralPanel::default().frame(Frame::new().fill(p.body)).show(ctx, |ui| {
            if self.downloads.is_empty() {
                ui.add_space(32.0);
                ui.vertical_centered(|ui| {
                    ui.label(icon(I_YOUTUBE, 256.0, p.divider));
                    let mut job = md_job(tr.dashboard_placeholder, 18.0, p.text, light(18.0));
                    job.halign = Align::Center;
                    job.wrap.max_width = ui.available_width() - 16.0;
                    ui.label(job);
                });
                return;
            }
            let area = ui.available_rect_before_wrap();
            let bg = ui.interact(area, egui::Id::new("downloads-area"), Sense::click());
            let mut actions: Vec<RowAction> = Vec::new();
            let mut order: Vec<usize> = (0..self.downloads.len()).collect();
            if let Some((col, asc)) = self.sort {
                order.sort_by(|&a, &b| {
                    let o = match col {
                        SortCol::File => self.downloads[a].file_name().to_lowercase().cmp(&self.downloads[b].file_name().to_lowercase()),
                        SortCol::Status => {
                            let f = |d: &Download| d.progress.lock().unwrap().fraction.unwrap_or(if d.status() == Status::Completed { 1.0 } else { 0.0 });
                            f(&self.downloads[a]).total_cmp(&f(&self.downloads[b]))
                        }
                    };
                    if asc { o } else { o.reverse() }
                });
            }
            let mut sort_click: Option<SortCol> = None;
            let time = ui.input(|i| i.time) as f32;
            Frame::new().inner_margin(Margin { left: 0, right: 28, top: 0, bottom: 0 }).show(ui, |ui| {
                TableBuilder::new(ui)
                    .striped(false)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::exact(64.0))
                    .column(Column::remainder().clip(true))
                    .column(Column::auto().at_least(120.0))
                    .column(Column::exact(136.0))
                    .header(30.0, |mut h| {
                        h.col(|_| {});
                        for (col, label) in [(SortCol::File, tr.col_file), (SortCol::Status, tr.col_status)] {
                            h.col(|ui| {
                                let arrow = match self.sort {
                                    Some((c, true)) if c == col => " ▲",
                                    Some((c, false)) if c == col => " ▼",
                                    _ => "",
                                };
                                if ui.add(egui::Label::new(RichText::new(format!("{label}{arrow}")).font(bold(13.0)).color(p.weak)).sense(Sense::click())).clicked() {
                                    sort_click = Some(col);
                                }
                            });
                        }
                        h.col(|_| {});
                    })
                    .body(|body| {
                        body.rows(56.0, order.len(), |mut row| {
                            let i = order[row.index()];
                            let d = &self.downloads[i];
                            let (status, error) = {
                                let s = d.state.lock().unwrap();
                                (s.status, s.error.clone())
                            };
                            row.col(|ui| {
                                ui.add_space(8.0);
                                ui.add(egui::Image::new(d.video.thumb_low()).fit_to_exact_size(Vec2::splat(48.0)));
                            });
                            row.col(|ui| {
                                let name = d.file_name();
                                ui.add(egui::Label::new(RichText::new(&name).size(14.0)).truncate()).on_hover_text(&name);
                            });
                            row.col(|ui| {
                                let (text, color, sense) = match status {
                                    Status::Enqueued => (tr.status_enqueued.to_string(), p.text.gamma_multiply(0.7), false),
                                    Status::Started => {
                                        let pr = d.progress.lock().unwrap();
                                        circular_progress(ui, if pr.processing { None } else { pr.fraction }, time, p.divider);
                                        let t = if pr.processing {
                                            tr.status_processing.to_string()
                                        } else {
                                            pr.fraction.map(|f| format!("{:.1}%", f * 100.0)).unwrap_or_default()
                                        };
                                        (t, p.text, false)
                                    }
                                    Status::Completed => (tr.status_completed.to_string(), p.ok, false),
                                    Status::Canceled => (tr.status_canceled.to_string(), p.warn, false),
                                    Status::Failed => (tr.status_failed.to_string(), p.fail, true),
                                };
                                let mut label = egui::Label::new(RichText::new(text).color(color)).truncate();
                                if sense {
                                    label = label.sense(Sense::click());
                                }
                                let r = ui.add(label);
                                if let Some(e) = error.filter(|_| sense) {
                                    let r = r.on_hover_ui(|ui| {
                                        ui.label(RichText::new(tr.click_to_copy_error).font(bold(13.0)));
                                        ui.add_space(6.0);
                                        ui.label(e);
                                    });
                                    if r.clicked() {
                                        actions.push(RowAction::CopyError(i));
                                    }
                                    r.context_menu(|ui| {
                                        if ui.button(tr.report_this_error).clicked() {
                                            actions.push(RowAction::Report(i));
                                            ui.close_menu();
                                        }
                                    });
                                    r.on_hover_cursor(egui::CursorIcon::PointingHand);
                                }
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(8.0);
                                    let btn = |ui: &mut egui::Ui, code: &str, tip: &str| ui.add(egui::Button::new(icon(code, 24.0, p.text)).frame(false)).on_hover_text(tip).clicked();
                                    if matches!(status, Status::Canceled | Status::Failed) && btn(ui, I_RESTART, tr.restart_download) {
                                        actions.push(RowAction::Restart(i));
                                    }
                                    if matches!(status, Status::Enqueued | Status::Started) && btn(ui, I_CANCEL, tr.cancel_download) {
                                        actions.push(RowAction::Cancel(i));
                                    }
                                    if status == Status::Completed {
                                        if btn(ui, I_PLAY, tr.play) {
                                            actions.push(RowAction::Open(i));
                                        }
                                        if btn(ui, I_FIND, tr.show_file) {
                                            actions.push(RowAction::ShowFile(i));
                                        }
                                    }
                                });
                            });
                        });
                    });
            });
            if let Some(col) = sort_click {
                self.sort = match self.sort {
                    Some((c, asc)) if c == col => Some((col, !asc)),
                    _ => Some((col, true)),
                };
            }
            bg.context_menu(|ui| {
                if ui.button(tr.ctx_remove_successful).clicked() {
                    self.remove_where(|s| s == Status::Completed);
                    ui.close_menu();
                }
                if ui.button(tr.ctx_remove_inactive).clicked() {
                    self.remove_where(|s| matches!(s, Status::Completed | Status::Failed | Status::Canceled));
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(tr.ctx_restart_failed).clicked() {
                    let failed: Vec<usize> = (0..self.downloads.len()).filter(|&i| self.downloads[i].status() == Status::Failed).collect();
                    for i in failed {
                        self.restart(i);
                    }
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(tr.ctx_cancel_all).clicked() {
                    for i in 0..self.downloads.len() {
                        self.cancel(i);
                    }
                    ui.close_menu();
                }
            });
            for a in actions {
                match a {
                    RowAction::Cancel(i) => self.cancel(i),
                    RowAction::Restart(i) => self.restart(i),
                    RowAction::ShowFile(i) => show_in_explorer(&self.downloads[i].state.lock().unwrap().path),
                    RowAction::Open(i) => open_file(&self.downloads[i].state.lock().unwrap().path),
                    RowAction::CopyError(i) => {
                        if let Some(e) = self.downloads[i].state.lock().unwrap().error.clone() {
                            ui.ctx().copy_text(e);
                        }
                        self.snack(tr.copied);
                    }
                    RowAction::Report(i) => {
                        let error = self.downloads[i].state.lock().unwrap().error.clone();
                        let video = self.downloads[i].video.clone();
                        self.open_report(Some(&video), error);
                    }
                }
            }
        });
    }

    fn snackbar(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        while self.snacks.front().is_some_and(|s| s.until < now) {
            self.snacks.pop_front();
        }
        let Some(s) = self.snacks.front() else { return };
        let (text, action) = (s.text.clone(), s.action);
        let tr = self.tr();
        let mut clicked = false;
        egui::Area::new(egui::Id::new("snackbar")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -16.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            Frame::new().fill(Color32::from_gray(0x32)).corner_radius(4).inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
                ui.set_max_width(520.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&text).color(Color32::WHITE));
                    if let Some(SnackAction::Restart) = action {
                        ui.add_space(12.0);
                        clicked = ui.add(egui::Button::new(RichText::new(tr.update_install_now).color(ACCENT).font(bold(14.0))).frame(false)).clicked();
                    }
                });
            });
        });
        if clicked {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).spawn();
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    // ------------------------------------------------------------ finestre di dialogo

    fn dialogs(&mut self, ctx: &egui::Context, p: &Pal) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let width = match dialog {
            Dialog::Settings => 380.0,
            Dialog::Auth => 450.0,
            Dialog::Message(_) => 420.0,
            Dialog::Report(_) => 540.0,
            Dialog::About => 520.0,
            _ => 500.0,
        };
        let mut keep = true;
        let mut action: Option<MsgAction> = None;
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        egui::Modal::new(egui::Id::new("dialog"))
            .frame(Frame::new().fill(p.card).corner_radius(4).inner_margin(0))
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.set_max_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                keep = match &mut dialog {
                    Dialog::Single(d) => self.single_dialog(ui, p, d),
                    Dialog::Multiple(d) => self.multiple_dialog(ui, p, d),
                    Dialog::Settings => self.settings_dialog(ui, p),
                    Dialog::Auth => self.auth_dialog(ui, p),
                    Dialog::Report(r) => self.report_dialog(ui, p, r),
                    Dialog::About => self.about_dialog(ui, p),
                    Dialog::Message(m) => {
                        let (k, a) = message_dialog(ui, p, m);
                        action = a;
                        k
                    }
                };
            });
        if escape {
            keep = false;
            if let Dialog::Message(m) = &dialog {
                action = Some(m.on_cancel);
            }
        }
        if keep && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
        if let Some(a) = action {
            self.run_action(ctx, a);
        }
    }

    fn single_dialog(&mut self, ui: &mut egui::Ui, p: &Pal, d: &mut SingleDlg) -> bool {
        let tr = self.tr();
        Frame::new().inner_margin(16).show(ui, |ui| {
            let t = ui.add(egui::Label::new(RichText::new(&d.video.title).size(19.0)).truncate().sense(Sense::click())).on_hover_text(&d.video.title);
            t.context_menu(|ui| {
                if ui.button(tr.copy).clicked() {
                    ui.ctx().copy_text(d.video.title.clone());
                    ui.close_menu();
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(icon(I_PERSON, 16.0, p.text));
                ui.label(RichText::new(&d.video.author).font(light(14.0)));
                ui.add_space(16.0);
                ui.label(icon(I_CLOCK, 16.0, p.text));
                ui.label(RichText::new(d.video.duration_text().unwrap_or_else(|| tr.live.into())).font(light(14.0)));
            });
        });
        divider(ui, p);
        ui.add(egui::Image::new(d.video.thumb_high()).fit_to_exact_size(Vec2::new(500.0, 281.0)).maintain_aspect_ratio(true));
        divider(ui, p);
        Frame::new().inner_margin(16).show(ui, |ui| {
            ui.label(RichText::new(tr.format).size(12.0).color(p.weak));
            ui.add_space(4.0);
            let label = |o: &DownloadOption| -> (String, String) {
                if o.is_subtitles() {
                    let extra = match o.container.as_str() {
                        "txt" => format!(" ({})", tr.plain_text),
                        "raw" => format!(" ({})", tr.raw_format),
                        _ => String::new(),
                    };
                    (tr.subtitles.to_string(), format!("{}{extra}", o.container))
                } else if o.audio_only {
                    (tr.audio.to_string(), o.container.clone())
                } else {
                    let up = if o.upscaled { format!("  [{}]", tr.upscaled) } else { String::new() };
                    (format!("{}{up}", o.quality_label()), o.container.clone())
                }
            };
            let (title, sub) = label(&d.options[d.selected]);
            egui::ComboBox::from_id_salt("format")
                .width(468.0)
                .height(420.0)
                .selected_text(RichText::new(format!("{title}  ·  {sub}")).font(bold(14.0)))
                .show_ui(ui, |ui| {
                    for (i, o) in d.options.iter().enumerate() {
                        let (a, b) = label(o);
                        let mut job = LayoutJob::default();
                        job.append(&a, 0.0, TextFormat { font_id: bold(14.0), color: p.text, ..Default::default() });
                        job.append(&format!("\n{b}"), 0.0, TextFormat { font_id: light(13.0), color: p.weak, ..Default::default() });
                        if ui.add(egui::SelectableLabel::new(d.selected == i, job)).clicked() {
                            d.selected = i;
                        }
                    }
                });
            if d.options[d.selected].is_subtitles() {
                ui.add_space(12.0);
                ui.label(RichText::new(tr.sub_language).size(12.0).color(p.weak));
                ui.add_space(4.0);
                let name = |i: usize| -> String {
                    if i == 0 {
                        return tr.sub_all.to_string();
                    }
                    let l = &d.sub_langs[i - 1];
                    match l.kind {
                        SubKind::Manual => format!("{} ({})", l.name, l.code),
                        SubKind::Auto => format!("{} {}", l.name, tr.sub_auto_suffix),
                        SubKind::Translated => format!("{} {}", l.name, tr.sub_translated),
                    }
                };
                egui::ComboBox::from_id_salt("sublang").width(468.0).height(360.0).selected_text(name(d.sub_lang)).show_ui(ui, |ui| {
                    for i in 0..=d.sub_langs.len() {
                        ui.selectable_value(&mut d.sub_lang, i, name(i));
                    }
                });
            }
        });
        let (ok, cancel) = dialog_buttons(ui, tr.download_button, tr.cancel_button, true);
        if cancel {
            return false;
        }
        if ok {
            let mut option = d.options[d.selected].clone();
            if option.is_subtitles() {
                let spec = if d.sub_lang == 0 {
                    SubSpec { langs: "all,-live_chat".into(), auto: false }
                } else {
                    let l = &d.sub_langs[d.sub_lang - 1];
                    self.settings.last_sub_lang = l.code.trim_end_matches("-orig").to_string();
                    SubSpec { langs: escape_regex(&l.code), auto: l.kind != SubKind::Manual }
                };
                option.subs = Some(spec);
            }
            let ext = if option.container == "raw" { "srt".to_string() } else { option.container.clone() };
            let name = engine::apply_template(&self.settings.template, &d.video, &ext, None);
            let mut dlg = rfd::FileDialog::new().set_file_name(&name);
            if option.container != "raw" {
                dlg = dlg.add_filter(format!("{} file", option.container), &[option.container.as_str()]);
            }
            let Some(path) = dlg.save_file() else { return true };
            if !option.is_subtitles() {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&path, []);
            }
            self.settings.last_container = option.container.clone();
            let seq = self.new_seq();
            let video = d.video.clone();
            self.enqueue(Download::new(seq, video, Some(option), None, path), 0);
            self.query.clear();
            return false;
        }
        true
    }

    fn multiple_dialog(&mut self, ui: &mut egui::Ui, p: &Pal, d: &mut MultipleDlg) -> bool {
        let tr = self.tr();
        Frame::new().inner_margin(16).show(ui, |ui| {
            let t = ui.add(egui::Label::new(RichText::new(&d.title).size(19.0)).truncate().sense(Sense::click()));
            t.context_menu(|ui| {
                if ui.button(tr.copy).clicked() {
                    ui.ctx().copy_text(d.title.clone());
                    ui.close_menu();
                }
            });
        });
        divider(ui, p);
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::A)) {
            let all = d.selected.iter().all(|s| *s);
            d.selected.iter_mut().for_each(|s| *s = !all);
        }
        egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
            for (i, v) in d.videos.iter().enumerate() {
                let sel = d.selected[i];
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 64.0), Sense::click());
                if sel {
                    ui.painter().rect_filled(rect, 0.0, ACCENT.gamma_multiply(0.15));
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 0.0, p.divider.gamma_multiply(0.4));
                }
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(8.0, 8.0))).layout(Layout::left_to_right(Align::Center)));
                child.add(egui::Image::new(v.thumb_low()).fit_to_exact_size(Vec2::splat(48.0)));
                child.add_space(8.0);
                let text_w = child.available_width() - 40.0;
                child.allocate_ui_with_layout(Vec2::new(text_w, 48.0), Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.add(egui::Label::new(RichText::new(&v.title).size(16.0)).truncate().selectable(false)).on_hover_text(&v.title);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.label(icon(I_PERSON, 16.0, p.text));
                        ui.add(egui::Label::new(RichText::new(&v.author).font(light(13.0))).truncate().selectable(false));
                        ui.add_space(8.0);
                        ui.label(icon(I_CLOCK, 16.0, p.text));
                        ui.label(RichText::new(v.duration_text().unwrap_or_else(|| tr.live.into())).font(light(13.0)));
                    });
                });
                if sel {
                    child.label(icon(I_CHECK, 24.0, p.text));
                }
                if resp.clicked() {
                    d.selected[i] = !sel;
                }
            }
        });
        divider(ui, p);
        Frame::new().inner_margin(16).show(ui, |ui| {
            ui.columns(2, |cols| {
                cols[0].label(RichText::new(tr.container).size(12.0).color(p.weak));
                cols[0].add_space(4.0);
                let cname = |c: &str| -> String {
                    if engine::container_is_subtitles(c) {
                        format!("{c} ({})", tr.subtitles.to_lowercase())
                    } else if engine::container_is_audio_only(c) {
                        format!("{c} ({})", tr.audio.to_lowercase())
                    } else {
                        c.to_string()
                    }
                };
                egui::ComboBox::from_id_salt("container").width(cols[0].available_width()).selected_text(cname(&d.container)).show_ui(&mut cols[0], |ui| {
                    for c in engine::CONTAINERS {
                        ui.selectable_value(&mut d.container, c.to_string(), cname(c));
                    }
                });
                if engine::container_is_subtitles(&d.container) {
                    cols[1].label(RichText::new(tr.sub_language).size(12.0).color(p.weak));
                    cols[1].add_space(4.0);
                    let lname = |code: &str| -> String {
                        MULTI_SUB_LANGS.iter().find(|(c, _)| *c == code).map(|(c, n)| if *c == "all" { tr.sub_all.to_string() } else { n.to_string() }).unwrap_or_else(|| code.to_string())
                    };
                    egui::ComboBox::from_id_salt("mlang").width(cols[1].available_width()).selected_text(lname(&d.sub_lang)).show_ui(&mut cols[1], |ui| {
                        for (c, _) in MULTI_SUB_LANGS {
                            ui.selectable_value(&mut d.sub_lang, c.to_string(), lname(c));
                        }
                    });
                    cols[1].add_space(4.0);
                    cols[1].checkbox(&mut d.sub_auto, tr.sub_include_auto);
                } else {
                    cols[1].label(RichText::new(tr.video_quality).size(12.0).color(p.weak));
                    cols[1].add_space(4.0);
                    let audio = engine::container_is_audio_only(&d.container);
                    let qname = |q: QualityPreference| -> String {
                        match q {
                            QualityPreference::Highest => tr.quality_highest.into(),
                            QualityPreference::Lowest => tr.quality_lowest.into(),
                            q => format!("≤ {}p", q.max_height().unwrap_or(0)),
                        }
                    };
                    let w = cols[1].available_width();
                    cols[1].add_enabled_ui(!audio, |ui| {
                        egui::ComboBox::from_id_salt("quality").width(w).selected_text(qname(d.quality)).show_ui(ui, |ui| {
                            for q in QualityPreference::MENU {
                                ui.selectable_value(&mut d.quality, q, qname(q));
                            }
                        });
                    });
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new(tr.select_all_hint).size(11.0).color(p.weak));
        });
        let count = d.selected.iter().filter(|s| **s).count();
        let (ok, cancel) = dialog_buttons(ui, &format!("{} ({count})", tr.download_button), tr.cancel_button, count > 0);
        if cancel {
            return false;
        }
        if ok {
            let Some(dir) = rfd::FileDialog::new().set_title(tr.choose_folder).pick_folder() else { return true };
            let subs = engine::container_is_subtitles(&d.container).then(|| SubSpec {
                langs: if d.sub_lang == "all" { "all,-live_chat".into() } else { format!("{}.*", d.sub_lang) },
                auto: d.sub_auto && d.sub_lang != "all",
            });
            let pref = Preference { container: d.container.clone(), quality: d.quality, subs };
            let chosen: Vec<&Video> = d.videos.iter().zip(&d.selected).filter(|(_, s)| **s).map(|(v, _)| v).collect();
            let width = chosen.len().to_string().len();
            let ext = if d.container == "raw" { "srt".to_string() } else { d.container.clone() };
            for (i, v) in chosen.into_iter().enumerate() {
                let number = format!("{:0width$}", i + 1);
                let base = dir.join(engine::apply_template(&self.settings.template, v, &ext, Some(&number)));
                if self.settings.skip_existing && base.exists() {
                    continue;
                }
                let path = engine::unique_path(&base);
                if pref.subs.is_none() {
                    let _ = std::fs::write(&path, []);
                }
                let seq = self.new_seq();
                self.enqueue(Download::new(seq, v.clone(), None, Some(pref.clone()), path), 0);
            }
            self.settings.last_container = d.container.clone();
            self.settings.last_quality = d.quality;
            self.settings.last_sub_lang = d.sub_lang.clone();
            self.settings.last_sub_auto = d.sub_auto;
            self.query.clear();
            return false;
        }
        true
    }

    fn settings_dialog(&mut self, ui: &mut egui::Ui, p: &Pal) -> bool {
        let tr = self.tr();
        Frame::new().inner_margin(16).show(ui, |ui| {
            ui.label(RichText::new(tr.settings_title).font(light(19.0)));
        });
        divider(ui, p);
        egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
            Frame::new().inner_margin(Margin::symmetric(16, 8)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let s = &mut self.settings;
                let theme_name = |t: ThemePref| match t {
                    ThemePref::System => tr.theme_system,
                    ThemePref::Light => tr.theme_light,
                    ThemePref::Dark => tr.theme_dark,
                };
                setting_row(ui, tr.theme_label, tr.theme_tooltip, |ui| {
                    egui::ComboBox::from_id_salt("theme").width(160.0).selected_text(theme_name(s.theme)).show_ui(ui, |ui| {
                        for t in [ThemePref::System, ThemePref::Light, ThemePref::Dark] {
                            ui.selectable_value(&mut s.theme, t, theme_name(t));
                        }
                    });
                });
                setting_row(ui, tr.language_label, tr.language_tooltip, |ui| {
                    egui::ComboBox::from_id_salt("lang").width(160.0).selected_text(s.language.name()).show_ui(ui, |ui| {
                        for l in Language::ALL {
                            ui.selectable_value(&mut s.language, l, l.name());
                        }
                    });
                });
                setting_row(ui, tr.auto_update_label, tr.auto_update_tooltip, |ui| toggle(ui, &mut s.auto_update));
                setting_row(ui, tr.persist_auth_label, tr.persist_auth_tooltip, |ui| toggle(ui, &mut s.persist_auth));
                path_row(ui, tr.ffmpeg_label, tr.ffmpeg_tooltip, tr, &mut s.ffmpeg_path, "ffmpeg.exe");
                path_row(ui, tr.ytdlp_label, tr.ytdlp_tooltip, tr, &mut s.ytdlp_path, "yt-dlp.exe");
                setting_row(ui, tr.inject_langs_label, tr.inject_langs_tooltip, |ui| toggle(ui, &mut s.inject_langs));
                setting_row(ui, tr.inject_subs_label, tr.inject_subs_tooltip, |ui| toggle(ui, &mut s.inject_subs));
                setting_row(ui, tr.inject_tags_label, tr.inject_tags_tooltip, |ui| toggle(ui, &mut s.inject_tags));
                setting_row(ui, tr.skip_existing_label, tr.skip_existing_tooltip, |ui| toggle(ui, &mut s.skip_existing));
                setting_row(ui, tr.template_label, tr.template_tooltip, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut s.template).desired_width(150.0).font(FontId::proportional(13.0)));
                });
                setting_row(ui, tr.parallel_label, tr.parallel_tooltip, |ui| {
                    ui.spacing_mut().slider_width = 150.0;
                    ui.add(egui::Slider::new(&mut s.parallel, 1..=10).show_value(false));
                    ui.label(s.parallel.to_string());
                });
            });
        });
        divider(ui, p);
        let mut close = false;
        Frame::new().inner_margin(16).show(ui, |ui| {
            close = outline_button(ui, tr.close_button, ui.available_width(), true).clicked();
        });
        !close
    }

    fn auth_dialog(&mut self, ui: &mut egui::Ui, p: &Pal) -> bool {
        let tr = self.tr();
        Frame::new().inner_margin(16).show(ui, |ui| {
            ui.label(RichText::new(tr.auth_title).font(light(19.0)));
        });
        divider(ui, p);
        Frame::new().inner_margin(16).show(ui, |ui| {
            ui.set_min_height(320.0);
            let source_name = |a: &AuthSource| -> String {
                match a {
                    AuthSource::None => tr.auth_none.into(),
                    AuthSource::Browser(b) => browser_name(b),
                    AuthSource::CookiesFile(_) => tr.auth_cookies_file.into(),
                }
            };
            if self.settings.auth.is_set() && !matches!(&self.settings.auth, AuthSource::CookiesFile(f) if f.is_empty()) {
                ui.vertical_centered(|ui| {
                    ui.add_space(16.0);
                    ui.label(icon(I_AUTH_OK, 160.0, p.divider));
                    ui.add_space(16.0);
                    ui.label(RichText::new(tr.authenticated_text).font(light(18.0)));
                    ui.add_space(4.0);
                    let detail = match &self.settings.auth {
                        AuthSource::CookiesFile(f) => f.clone(),
                        a => source_name(a),
                    };
                    ui.label(RichText::new(detail).color(p.weak));
                    ui.add_space(16.0);
                    if ui.add(egui::Button::new(RichText::new(tr.log_out).color(ACCENT).size(18.0)).frame(false)).clicked() {
                        self.settings.auth = AuthSource::None;
                    }
                });
            } else {
                ui.label(RichText::new(tr.auth_intro).size(14.0));
                ui.add_space(16.0);
                ui.label(RichText::new(tr.auth_source).size(12.0).color(p.weak));
                ui.add_space(4.0);
                let mut choice = self.settings.auth.clone();
                egui::ComboBox::from_id_salt("auth").width(ui.available_width()).selected_text(source_name(&choice)).show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, AuthSource::None, tr.auth_none);
                    for b in engine::BROWSERS {
                        ui.selectable_value(&mut choice, AuthSource::Browser(b.into()), browser_name(b));
                    }
                    let file = matches!(choice, AuthSource::CookiesFile(_));
                    if ui.selectable_label(file, tr.auth_cookies_file).clicked() {
                        choice = AuthSource::CookiesFile(String::new());
                    }
                });
                if let AuthSource::CookiesFile(f) = &mut choice {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(f).desired_width(ui.available_width() - 40.0).hint_text("cookies.txt"));
                        if ui.add(egui::Button::new(icon(I_FOLDER, 18.0, p.text)).frame(false)).on_hover_text(tr.browse_tooltip).clicked() {
                            if let Some(path) = rfd::FileDialog::new().add_filter("cookies.txt", &["txt"]).pick_file() {
                                *f = path.display().to_string();
                            }
                        }
                    });
                }
                self.settings.auth = choice;
                ui.add_space(16.0);
                ui.label(RichText::new(tr.auth_browser_note).size(12.0).color(p.weak));
            }
        });
        divider(ui, p);
        let mut close = false;
        Frame::new().inner_margin(16).show(ui, |ui| {
            close = outline_button(ui, tr.close_button, ui.available_width(), true).clicked();
        });
        !close
    }
    fn about_dialog(&mut self, ui: &mut egui::Ui, p: &Pal) -> bool {
        let tr = self.tr();
        Frame::new().inner_margin(16).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Image::new(egui::include_image!("../assets/icon256.png")).fit_to_exact_size(Vec2::splat(56.0)));
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(APP_NAME).font(bold(22.0)));
                    ui.label(RichText::new(format!("v{VERSION}")).color(p.weak));
                });
            });
            ui.add_space(10.0);
            ui.label(RichText::new(tr.about_tagline).font(bold(15.0)).color(ACCENT));
        });
        divider(ui, p);
        egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
            Frame::new().inner_margin(16).show(ui, |ui| {
                ui.set_max_width(520.0 - 32.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.add(egui::Label::new(RichText::new(tr.about_intro).size(14.0)).wrap());
                ui.add_space(4.0);
                for line in tr.about_features.lines() {
                    ui.horizontal_top(|ui| {
                        ui.label(icon(I_CHECK, 18.0, ACCENT));
                        let job = md_job(line, 14.0, p.text, FontId::proportional(14.0));
                        ui.add(egui::Label::new(job).wrap());
                    });
                }
                ui.add_space(6.0);
                let job = md_job(tr.about_credits, 12.5, p.weak, FontId::proportional(12.5));
                ui.add(egui::Label::new(job).wrap());
                ui.add(egui::Label::new(RichText::new(tr.about_legal).size(12.5).italics().color(p.weak)).wrap());
            });
        });
        divider(ui, p);
        let mut close = false;
        Frame::new().inner_margin(16).show(ui, |ui| {
            let w = (ui.available_width() - 16.0) / 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                if outline_button(ui, tr.about_github, w, true).clicked() {
                    open_url(&format!("https://github.com/{}", update::APP_REPO));
                }
                close = outline_button(ui, tr.close_button, w, true).clicked();
            });
        });
        !close
    }

    fn open_report(&mut self, video: Option<&Video>, error: Option<String>) {
        let ytdlp = engine::resolve_ytdlp(&self.settings.ytdlp_path);
        let ytdlp_version = ytdlp.as_deref().and_then(engine::ytdlp_version).unwrap_or_else(|| "?".into());
        let managed = ytdlp.as_deref() == Some(engine::managed_ytdlp().as_path());
        let environment = format!(
            "- {APP_NAME} {VERSION}\n- yt-dlp {ytdlp_version} ({})\n- FFmpeg: {}\n- {}\n- patch.json: revisione {}\n- Autenticazione: {}",
            if managed { "copia dell'app" } else { "esterno" },
            if engine::resolve_ffmpeg(&self.settings.ffmpeg_path).is_some() { "trovato" } else { "mancante" },
            windows_version(),
            self.settings.patch.revision,
            if self.settings.auth.is_set() { "sì" } else { "no" },
        );
        self.dialog = Some(Dialog::Report(ReportDlg {
            description: String::new(),
            include_url: true,
            video_url: video.map(Video::url),
            error,
            environment,
        }));
    }

    fn report_dialog(&mut self, ui: &mut egui::Ui, p: &Pal, r: &mut ReportDlg) -> bool {
        let tr = self.tr();
        Frame::new().inner_margin(16).show(ui, |ui| {
            ui.label(RichText::new(tr.report_title).font(light(19.0)));
        });
        divider(ui, p);
        Frame::new().inner_margin(16).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(RichText::new(tr.report_describe).size(13.0).color(p.weak));
            ui.add(egui::TextEdit::multiline(&mut r.description).desired_rows(4).desired_width(f32::INFINITY));
            if r.video_url.is_some() {
                ui.checkbox(&mut r.include_url, tr.report_include_url);
            }
            ui.add_space(6.0);
            ui.label(RichText::new(tr.report_preview).size(13.0).color(p.weak));
            let body = report_body(r);
            egui::ScrollArea::vertical().max_height(170.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&body).monospace().size(11.5)).wrap());
            });
            ui.add_space(6.0);
            ui.label(RichText::new(tr.report_note).size(11.5).color(p.weak));
        });
        divider(ui, p);
        let mut close = false;
        Frame::new().inner_margin(16).show(ui, |ui| {
            let w = (ui.available_width() - 32.0) / 3.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                if outline_button(ui, tr.report_open_github, w, true).clicked() {
                    let title = report_title(r);
                    let url = format!(
                        "https://github.com/{}/issues/new?labels=segnalazione&title={}&body={}",
                        update::APP_REPO,
                        url_encode(&title),
                        url_encode(&truncate_chars(&report_body(r), 6000))
                    );
                    open_url(&url);
                    close = true;
                }
                if outline_button(ui, tr.report_copy, w, true).clicked() {
                    ui.ctx().copy_text(format!("{}\n\n{}", report_title(r), report_body(r)));
                    self.snack(tr.copied);
                }
                if outline_button(ui, tr.cancel_button, w, true).clicked() {
                    close = true;
                }
            });
        });
        !close
    }
}

/// Toglie i percorsi personali (nome utente compreso) da ciò che si invia.
fn sanitize(text: &str) -> String {
    let mut out = text.to_string();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let home = home.to_string_lossy().to_string();
        if !home.is_empty() {
            out = out.replace(&home, "%USERPROFILE%").replace(&home.replace(std::path::MAIN_SEPARATOR, "/"), "%USERPROFILE%");
        }
    }
    if let Ok(user) = std::env::var("USERNAME") {
        if user.len() > 2 {
            out = out.replace(&user, "<utente>");
        }
    }
    out
}

fn report_title(r: &ReportDlg) -> String {
    let first = r.description.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let first = if first.is_empty() {
        r.error.as_deref().and_then(|e| e.lines().next()).unwrap_or("Segnalazione")
    } else {
        first
    };
    sanitize(&truncate_chars(first, 90))
}

fn report_body(r: &ReportDlg) -> String {
    let mut b = String::new();
    b.push_str("### Descrizione\n");
    b.push_str(if r.description.trim().is_empty() { "(nessuna descrizione)" } else { r.description.trim() });
    b.push_str("\n\n");
    if let (true, Some(u)) = (r.include_url, &r.video_url) {
        b.push_str(&format!("### Video\n{u}\n\n"));
    }
    if let Some(e) = &r.error {
        b.push_str(&format!("### Errore\n```\n{}\n```\n\n", e.trim()));
    }
    b.push_str("### Ambiente\n");
    b.push_str(&r.environment);
    b.push('\n');
    sanitize(&b)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn windows_version() -> String {
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/C", "ver"]);
    engine::hide_console(&mut cmd);
    cmd.output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "Windows".into())
}

/// Apre un indirizzo nel browser predefinito (gli `&` dell'URL non passano da cmd).
fn open_url(url: &str) {
    let mut cmd = std::process::Command::new("rundll32");
    cmd.args(["url.dll,FileProtocolHandler", url]);
    let _ = cmd.spawn();
}

fn browser_name(b: &str) -> String {
    match b {
        "edge" => "Microsoft Edge".into(),
        "chrome" => "Google Chrome".into(),
        b => {
            let mut c = b.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// I codici lingua vanno passati come espressione regolare a `--sub-langs`.
fn escape_regex(s: &str) -> String {
    s.chars().flat_map(|c| if ".+*?()[]{}|^$\\".contains(c) { vec!['\\', c] } else { vec![c] }).collect()
}

fn message_dialog(ui: &mut egui::Ui, p: &Pal, m: &MessageDlg) -> (bool, Option<MsgAction>) {
    Frame::new().inner_margin(16).show(ui, |ui| {
        ui.label(RichText::new(&m.title).font(light(19.0)));
    });
    Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 8 }).show(ui, |ui| {
        ui.add(egui::Label::new(RichText::new(&m.text).size(14.0).color(p.text)).wrap());
    });
    match &m.cancel {
        Some(c) => {
            let (ok, cancel) = dialog_buttons(ui, &m.ok, c, true);
            if ok {
                (false, Some(m.on_ok))
            } else if cancel {
                (false, Some(m.on_cancel))
            } else {
                (true, None)
            }
        }
        None => {
            let mut close = false;
            Frame::new().inner_margin(16).show(ui, |ui| {
                close = outline_button(ui, &m.ok, ui.available_width(), true).clicked();
            });
            (!close, close.then_some(m.on_ok))
        }
    }
}

fn divider(ui: &mut egui::Ui, p: &Pal) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.divider);
}

fn outline_button(ui: &mut egui::Ui, text: &str, width: f32, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(ACCENT).font(bold(14.0)))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0_f32, ACCENT))
            .corner_radius(4)
            .min_size(Vec2::new(width, 36.0)),
    )
}

/// Due pulsanti affiancati (come la UniformGrid dell'originale).
fn dialog_buttons(ui: &mut egui::Ui, ok: &str, cancel: &str, ok_enabled: bool) -> (bool, bool) {
    let mut res = (false, false);
    Frame::new().inner_margin(16).show(ui, |ui| {
        let w = (ui.available_width() - 16.0) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            res.0 = outline_button(ui, ok, w, ok_enabled).clicked();
            res.1 = outline_button(ui, cancel, w, true).clicked();
        });
        if ok_enabled && ui.input(|i| i.key_pressed(Key::Enter)) {
            res.0 = true;
        }
    });
    res
}

fn setting_row(ui: &mut egui::Ui, label: &str, tooltip: &str, control: impl FnOnce(&mut egui::Ui)) {
    let r = ui
        .horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.label(label);
            ui.with_layout(Layout::right_to_left(Align::Center), control);
        })
        .response;
    r.on_hover_text(plain(tooltip));
}

fn path_row(ui: &mut egui::Ui, label: &str, tooltip: &str, tr: &Tr, value: &mut String, file: &str) {
    setting_row(ui, label, tooltip, |ui| {
        let browse = ui.add(egui::Button::new(icon(I_FOLDER, 16.0, ui.visuals().text_color())).frame(false)).on_hover_text(tr.browse_tooltip);
        let reset = ui.add_enabled(!value.is_empty(), egui::Button::new(icon(I_CLOSE, 16.0, ui.visuals().text_color())).frame(false)).on_hover_text(tr.reset_tooltip);
        ui.add(egui::TextEdit::singleline(value).desired_width(110.0).font(FontId::proportional(13.0)).hint_text(tr.auto_detect));
        if reset.clicked() {
            value.clear();
        }
        if browse.clicked() {
            if let Some(p) = rfd::FileDialog::new().add_filter(file, &["exe"]).pick_file() {
                *value = p.display().to_string();
            }
        }
    });
}

/// Interruttore in stile Material (il ToggleSwitch dell'originale).
fn toggle(ui: &mut egui::Ui, on: &mut bool) {
    let size = Vec2::new(36.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let track = if *on { ACCENT.gamma_multiply(0.5) } else { Color32::from_gray(0x9E) };
    ui.painter().rect_filled(rect.shrink2(Vec2::new(0.0, 3.0)), 6.0, track);
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    let knob = if *on { ACCENT } else { Color32::from_gray(0xF1) };
    ui.painter().circle(egui::pos2(x, rect.center().y), 9.0, knob, Stroke::new(1.0_f32, Color32::from_black_alpha(40)));
}

/// Cerchio di avanzamento (indeterminato se `fraction` è `None`).
fn circular_progress(ui: &mut egui::Ui, fraction: Option<f32>, time: f32, track: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
    let c = rect.center();
    let r = 8.0;
    ui.painter().circle_stroke(c, r, Stroke::new(2.5_f32, track));
    let (start, sweep) = match fraction {
        Some(f) if f > 0.0 => (-std::f32::consts::FRAC_PI_2, f * std::f32::consts::TAU),
        _ => {
            ui.ctx().request_repaint();
            (time * 5.0, std::f32::consts::FRAC_PI_2 * 1.2)
        }
    };
    let n = 32;
    let pts: Vec<egui::Pos2> = (0..=n).map(|i| {
        let a = start + sweep * i as f32 / n as f32;
        c + Vec2::new(a.cos(), a.sin()) * r
    }).collect();
    ui.painter().add(Shape::line(pts, Stroke::new(2.5_f32, ACCENT)));
    ui.add_space(4.0);
}

fn show_in_explorer(path: &PathBuf) {
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    }
    let _ = cmd.spawn();
}

fn open_file(path: &PathBuf) {
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(format!("\"{}\"", path.display()));
    }
    let _ = cmd.spawn();
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.started {
            self.started = true;
            update::cleanup_old_exe();
            self.check_tools();
            // `--no-auto-update` salta il controllo all'avvio (utile per le prove).
            let skip = std::env::args().any(|a| a == "--no-auto-update");
            if self.settings.auto_update && !skip && engine::resolve_ytdlp(&self.settings.ytdlp_path).is_some() {
                self.run_update(ctx, false);
            }
            // `--about` apre subito la finestra Informazioni (per le anteprime).
            if self.dialog.is_none() && std::env::args().any(|a| a == "--about") {
                self.dialog = Some(Dialog::About);
            }
        }
        if self.applied_theme != Some(self.settings.theme) {
            ctx.set_theme(match self.settings.theme {
                ThemePref::System => egui::ThemePreference::System,
                ThemePref::Light => egui::ThemePreference::Light,
                ThemePref::Dark => egui::ThemePreference::Dark,
            });
            self.applied_theme = Some(self.settings.theme);
        }
        self.handle_events(ctx);
        self.schedule(ctx);
        let p = pal(ctx.style().visuals.dark_mode);
        self.header(ctx, &p);
        self.body(ctx, &p);
        self.dialogs(ctx, &p);
        self.snackbar(ctx);
        if self.downloads.iter().any(|d| d.launched && d.status() == Status::Started) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let mut s = self.settings.clone();
        if !s.persist_auth {
            s.auth = AuthSource::None;
        }
        eframe::set_value(storage, eframe::APP_KEY, &s);
    }

    /// Come l'originale: chiudendo l'app si annullano i download in corso.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for d in &self.downloads {
            d.cancel.store(true, Ordering::Relaxed);
        }
        if self.downloads.iter().any(|d| d.launched && d.status() == Status::Started) {
            std::thread::sleep(Duration::from_millis(600));
        }
    }
}
