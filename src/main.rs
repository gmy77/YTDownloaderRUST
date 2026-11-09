#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! YTDownloaderRUST: scarica video, audio e sottotitoli da YouTube.
//! Interfaccia e comportamento ricalcano YoutubeDownloader di Oleksii Holub
//! (Tyrrrz, licenza MIT); il motore è yt-dlp, lanciato come processo esterno.

mod engine;
mod fluent;
mod i18n;
mod update;

use eframe::egui::{self, text::LayoutJob, Align, Align2, FontId, Frame, Key, Layout, Margin, RichText, Sense, Stroke, StrokeKind, TextFormat, Vec2};
use fluent::{
    bold, icon, Accent, Ico, Kind, Pal, I_AUTH, I_AUTH_OK, I_BUG, I_CANCEL, I_CHECK, I_CHEVRON_DOWN, I_CHEVRON_UP, I_CLOCK, I_CLOSE, I_EMPTY, I_ERROR, I_FIND,
    I_FOLDER, I_INFO, I_PERSON, I_PLAY, I_RESTART, I_SEARCH, I_SETTINGS, I_UPDATE,
};
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

fn main() -> eframe::Result {
    let icon = image::load_from_memory(include_bytes!("../assets/icon256.png")).map(|i| {
        let i = i.to_rgba8();
        egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() }
    });
    // Finestra trasparente: se Windows lo permette, dietro c'è il materiale Mica.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("{APP_NAME} v{VERSION}"))
        .with_inner_size([760.0, 620.0])
        .with_min_inner_size([600.0, 400.0])
        .with_transparent(true);
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

// ---------------------------------------------------------------- testo

/// `**grassetto**` → testo con i pezzi in semibold.
fn md_job(text: &str, size: f32, color: egui::Color32, base: FontId) -> LayoutJob {
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
    ReportSent(Result<u64, String>),
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
    sending_report: bool,
    /// Accento scelto dall'utente in Windows.
    accent: Accent,
    /// Sfondo Mica attivo (Windows 11 22H2+).
    mica: bool,
    hwnd: Option<isize>,
    applied_dark: Option<bool>,
}

/// Handle Win32 della finestra (per gli attributi DWM).
fn window_hwnd(w: &impl raw_window_handle::HasWindowHandle) -> Option<isize> {
    match w.window_handle().ok()?.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        fluent::setup_fonts(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let accent = Accent::from_system();
        let hwnd = window_hwnd(cc);
        // `--no-mica` lascia lo sfondo pieno (per schede video che non gestiscono la trasparenza).
        let try_mica = !std::env::args().any(|a| a == "--no-mica");
        let dark = cc.egui_ctx.style().visuals.dark_mode;
        let mica = hwnd.is_some_and(|h| fluent::apply_window_style(h, dark, try_mica));
        cc.egui_ctx.set_style_of(egui::Theme::Dark, fluent::style(true, &accent, mica));
        cc.egui_ctx.set_style_of(egui::Theme::Light, fluent::style(false, &accent, mica));
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
            sending_report: false,
            accent,
            mica,
            hwnd,
            applied_dark: Some(dark),
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
                Event::ReportSent(r) => {
                    self.sending_report = false;
                    let tr = self.tr();
                    match r {
                        Ok(n) => self.snack(fmt(tr.report_sent, &[&n.to_string()])),
                        Err(e) => self.message(tr.error_title, &fmt(tr.report_failed, &[&e])),
                    }
                }
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
            .frame(Frame::new().fill(p.window).inner_margin(Margin { left: 16, right: 12, top: 12, bottom: 10 }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let box_w = (ui.available_width() - 5.0 * 44.0 - 8.0).max(200.0);
                    let enabled = !self.busy;
                    // Invio = elabora, Maiusc+Invio = nuova riga (come l'originale).
                    let shift = ui.input(|i| i.modifiers.shift);
                    let mut submit = false;
                    if self.query_focused && !shift && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                        submit = true;
                    }
                    // Casella di ricerca come l'AutoSuggestBox di WinUI: icona di ricerca a destra.
                    let query = &mut self.query;
                    let (resp, go) = fluent::text_box(ui, p, enabled, |ui| {
                        ui.set_width(box_w - 16.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let resp = ui.add_enabled(
                                enabled,
                                egui::TextEdit::multiline(query)
                                    .frame(false)
                                    .desired_rows(1)
                                    .desired_width(ui.available_width() - 32.0)
                                    .margin(Margin::symmetric(0, 1))
                                    .hint_text(RichText::new(tr.query_placeholder).color(p.text2)),
                            );
                            let can = enabled && !query.trim().is_empty();
                            let go = fluent::icon_button(ui, p, I_SEARCH, Vec2::new(28.0, 22.0), can).on_hover_text(tr.process_query_tooltip);
                            (resp, go.clicked())
                        })
                        .inner
                    });
                    self.query_focused = resp.has_focus();
                    resp.on_hover_text(tr.query_tooltip);
                    if submit || go {
                        self.process_query(ui.ctx());
                    }
                    ui.add_space(8.0);
                    // Barra dei comandi: pulsanti "subtle" con sola icona.
                    let cmd = |ui: &mut egui::Ui, ico: Ico, tip: &str, on: bool| fluent::icon_button(ui, p, ico, Vec2::new(40.0, 36.0), on).on_hover_text(tip).clicked();
                    if cmd(ui, I_INFO, tr.about_tooltip, true) {
                        self.dialog = Some(Dialog::About);
                    }
                    if cmd(ui, I_BUG, tr.report_tooltip, true) {
                        self.open_report(None, None);
                    }
                    if cmd(ui, I_UPDATE, tr.update_tooltip, !self.updating) {
                        self.run_update(ui.ctx(), true);
                    }
                    if cmd(ui, I_AUTH, tr.auth_tooltip, enabled) {
                        self.dialog = Some(Dialog::Auth);
                    }
                    if cmd(ui, I_SETTINGS, tr.settings_tooltip, enabled) {
                        self.dialog = Some(Dialog::Settings);
                    }
                });
            });

        // ProgressBar WinUI sotto l'intestazione (visibile solo quando c'è attività).
        egui::TopBottomPanel::top("progress")
            .exact_height(3.0)
            .frame(Frame::new().fill(p.window).inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 0 }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let t = ui.input(|i| i.time) as f32;
                let fractions: Vec<f32> = self
                    .downloads
                    .iter()
                    .filter(|d| d.launched && d.status() == Status::Started)
                    .map(|d| d.progress.lock().unwrap().fraction.unwrap_or(0.0))
                    .collect();
                if self.busy || self.updating {
                    fluent::progress_bar(ui, p, rect, None, t);
                } else if !fractions.is_empty() {
                    let f = fractions.iter().sum::<f32>() / fractions.len() as f32;
                    fluent::progress_bar(ui, p, rect, Some(f), t);
                }
            });
    }

    fn body(&mut self, ctx: &egui::Context, p: &Pal) {
        let tr = self.tr();
        egui::CentralPanel::default().frame(Frame::new().fill(p.window).inner_margin(Margin { left: 16, right: 16, top: 10, bottom: 12 })).show(ctx, |ui| {
            if self.downloads.is_empty() {
                ui.add_space((ui.available_height() * 0.22).max(16.0));
                ui.vertical_centered(|ui| {
                    ui.label(icon(I_EMPTY, 64.0, p.text3));
                    ui.add_space(24.0);
                    let mut job = md_job(tr.dashboard_placeholder, 18.0, p.text2, FontId::proportional(18.0));
                    job.halign = Align::Center;
                    job.wrap.max_width = ui.available_width() - 32.0;
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
            // Colonne: [16][miniatura 64][12][nome][12][stato][12][azioni][16]
            const THUMB: f32 = 64.0;
            const STATUS_W: f32 = 150.0;
            const ACTIONS_W: f32 = 76.0;
            const FIXED: f32 = 16.0 + THUMB + 12.0 + 12.0 + STATUS_W + 12.0 + ACTIONS_W + 16.0;
            const ROW_H: f32 = 64.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(16.0 + THUMB + 12.0);
                let name_w = (ui.available_width() + 16.0 + THUMB + 12.0 - FIXED).max(60.0);
                for (col, label, w) in [(SortCol::File, tr.col_file, name_w + 12.0), (SortCol::Status, tr.col_status, STATUS_W)] {
                    let sorted = match self.sort {
                        Some((c, asc)) if c == col => Some(asc),
                        _ => None,
                    };
                    if column_header(ui, p, label, sorted, w).clicked() {
                        sort_click = Some(col);
                    }
                }
            });
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            let (h_body, h_caption) = ui.fonts(|f| (f.row_height(&FontId::proportional(14.0)), f.row_height(&FontId::proportional(12.0))));
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, order.len(), |ui, range| {
                for k in range {
                    let i = order[k];
                    let d = &self.downloads[i];
                    let (status, error) = {
                        let s = d.state.lock().unwrap();
                        (s.status, s.error.clone())
                    };
                    // Ogni download è una card WinUI (riga da 64, raggio 4, bordo da 1px).
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
                    let hovered = ui.rect_contains_pointer(rect);
                    ui.painter().rect(rect, 4, if hovered { p.control_hover } else { p.card }, Stroke::new(1.0_f32, p.card_stroke), StrokeKind::Inside);
                    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(16.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
                    row.spacing_mut().item_spacing.x = 12.0;
                    // Miniatura 4:3 di YouTube ritagliata a 16:9 (senza bande nere).
                    row.add(
                        egui::Image::new(d.video.thumb_low())
                            .uv(egui::Rect::from_min_max(egui::pos2(0.0, 0.125), egui::pos2(1.0, 0.875)))
                            .fit_to_exact_size(Vec2::new(THUMB, 36.0))
                            .maintain_aspect_ratio(false)
                            .corner_radius(4),
                    );
                    let name_w = (rect.width() - FIXED).max(60.0);
                    row.allocate_ui_with_layout(Vec2::new(name_w, ROW_H), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(name_w);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add_space(((ROW_H - h_body - h_caption - 2.0) / 2.0).max(0.0));
                        let name = d.file_name();
                        ui.add(egui::Label::new(RichText::new(&name)).truncate()).on_hover_text(&name);
                        let info = match d.video.duration_text() {
                            Some(t) => format!("{}  ·  {t}", d.video.author),
                            None => d.video.author.clone(),
                        };
                        ui.add(egui::Label::new(RichText::new(info).size(12.0).color(p.text2)).truncate());
                    });
                    row.allocate_ui_with_layout(Vec2::new(STATUS_W, ROW_H), Layout::left_to_right(Align::Center), |ui| {
                        ui.set_width(STATUS_W);
                        ui.spacing_mut().item_spacing.x = 8.0;
                        // Stato: colore di sistema sempre accompagnato da icona e parola.
                        let (ico, text, color, sense) = match status {
                            Status::Enqueued => (Some(I_CLOCK), tr.status_enqueued.to_string(), p.text2, false),
                            Status::Started => {
                                let pr = d.progress.lock().unwrap();
                                fluent::progress_ring(ui, p, if pr.processing { None } else { pr.fraction }, time);
                                let t = if pr.processing {
                                    tr.status_processing.to_string()
                                } else {
                                    pr.fraction.map(|f| format!("{:.1}%", f * 100.0)).unwrap_or_default()
                                };
                                (None, t, p.text, false)
                            }
                            Status::Completed => (Some(I_CHECK), tr.status_completed.to_string(), p.success, false),
                            Status::Canceled => (Some(I_CANCEL), tr.status_canceled.to_string(), p.caution, false),
                            Status::Failed => (Some(I_ERROR), tr.status_failed.to_string(), p.critical, true),
                        };
                        if let Some(ico) = ico {
                            ui.label(icon(ico, 16.0, color));
                        }
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
                    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let btn = |ui: &mut egui::Ui, ico: Ico, tip: &str| fluent::icon_button(ui, p, ico, Vec2::splat(32.0), true).on_hover_text(tip).clicked();
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
                }
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

    /// Notifica temporanea in basso, nello stile di un InfoBar/flyout WinUI.
    fn snackbar(&mut self, ctx: &egui::Context, p: &Pal) {
        let now = Instant::now();
        while self.snacks.front().is_some_and(|s| s.until < now) {
            self.snacks.pop_front();
        }
        let Some(s) = self.snacks.front() else { return };
        let (text, action) = (s.text.clone(), s.action);
        let tr = self.tr();
        let mut clicked = false;
        egui::Area::new(egui::Id::new("snackbar")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -24.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            Frame::new()
                .fill(p.flyout)
                .stroke(Stroke::new(1.0_f32, p.flyout_stroke))
                .corner_radius(8)
                .shadow(ctx.style().visuals.popup_shadow)
                .inner_margin(Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_max_width(540.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        ui.label(icon(I_INFO, 16.0, p.accent));
                        ui.label(RichText::new(&text).color(p.text));
                        if let Some(SnackAction::Restart) = action {
                            clicked = fluent::button(ui, p, tr.update_install_now, Kind::Accent, 0.0, true).clicked();
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
            Dialog::Settings => 440.0,
            Dialog::Auth => 460.0,
            Dialog::Message(_) => 440.0,
            Dialog::Report(_) => 548.0,
            Dialog::About => 520.0,
            _ => 520.0,
        };
        let mut keep = true;
        let mut action: Option<MsgAction> = None;
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        // ContentDialog: velo "smoke", raggio 8, bordo di superficie, ombra profonda.
        egui::Modal::new(egui::Id::new("dialog"))
            .backdrop_color(p.smoke)
            .frame(Frame::new().fill(p.base).stroke(Stroke::new(1.0_f32, p.surface_stroke)).corner_radius(8).shadow(ctx.style().visuals.window_shadow).inner_margin(0))
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
        fluent::dialog_body(ui, p, None, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let t = ui.add(egui::Label::new(RichText::new(&d.video.title).font(bold(20.0))).truncate().sense(Sense::click())).on_hover_text(&d.video.title);
            t.context_menu(|ui| {
                if ui.button(tr.copy).clicked() {
                    ui.ctx().copy_text(d.video.title.clone());
                    ui.close_menu();
                }
            });
            ui.add_space(8.0);
            video_meta(ui, p, &d.video.author, d.video.duration_text().unwrap_or_else(|| tr.live.into()), 14.0);
            ui.add_space(16.0);
            let w = ui.available_width();
            ui.add(egui::Image::new(d.video.thumb_high()).fit_to_exact_size(Vec2::new(w, w * 9.0 / 16.0)).maintain_aspect_ratio(true).corner_radius(8));
            ui.add_space(16.0);
            field_label(ui, tr.format);
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
            fluent::combo(ui, p, "format", w, RichText::new(format!("{title}  ·  {sub}")), |ui| {
                for (i, o) in d.options.iter().enumerate() {
                    let (a, b) = label(o);
                    let mut job = LayoutJob::default();
                    job.append(&a, 0.0, TextFormat { font_id: FontId::proportional(14.0), color: p.text, ..Default::default() });
                    job.append(&format!("\n{b}"), 0.0, TextFormat { font_id: FontId::proportional(12.0), color: p.text2, ..Default::default() });
                    if fluent::choice(ui, p, d.selected == i, job).clicked() {
                        d.selected = i;
                    }
                }
            });
            if d.options[d.selected].is_subtitles() {
                ui.add_space(12.0);
                field_label(ui, tr.sub_language);
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
                fluent::combo(ui, p, "sublang", w, name(d.sub_lang), |ui| {
                    for i in 0..=d.sub_langs.len() {
                        fluent::choice_value(ui, p, &mut d.sub_lang, i, name(i));
                    }
                });
            }
        });
        let (ok, cancel) = dialog_buttons(ui, p, tr.download_button, tr.cancel_button, true);
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
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::A)) {
            let all = d.selected.iter().all(|s| *s);
            d.selected.iter_mut().for_each(|s| *s = !all);
        }
        fluent::dialog_body(ui, p, None, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let t = ui.add(egui::Label::new(RichText::new(&d.title).font(bold(20.0))).truncate().sense(Sense::click()));
            t.context_menu(|ui| {
                if ui.button(tr.copy).clicked() {
                    ui.ctx().copy_text(d.title.clone());
                    ui.close_menu();
                }
            });
            ui.add_space(12.0);
            // ListView a selezione multipla: casella a sinistra, fondo "subtle" sulle voci scelte.
            egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (i, v) in d.videos.iter().enumerate() {
                    let sel = d.selected[i];
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 60.0), Sense::click());
                    let fill = if resp.is_pointer_button_down_on() {
                        p.subtle_pressed
                    } else if sel || resp.hovered() {
                        p.subtle_hover
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(rect, 4, fill);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(12.0, 8.0))).layout(Layout::left_to_right(Align::Center)));
                    child.spacing_mut().item_spacing.x = 12.0;
                    let (cb, _) = child.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                    fluent::check_mark(&child, p, cb, sel);
                    child.add(
                        egui::Image::new(v.thumb_low())
                            .uv(egui::Rect::from_min_max(egui::pos2(0.0, 0.125), egui::pos2(1.0, 0.875)))
                            .fit_to_exact_size(Vec2::new(64.0, 36.0))
                            .maintain_aspect_ratio(false)
                            .corner_radius(4),
                    );
                    let text_w = child.available_width();
                    child.allocate_ui_with_layout(Vec2::new(text_w, 44.0), Layout::top_down(Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.add(egui::Label::new(RichText::new(&v.title)).truncate()).on_hover_text(&v.title);
                        video_meta(ui, p, &v.author, v.duration_text().unwrap_or_else(|| tr.live.into()), 12.0);
                    });
                    if resp.clicked() {
                        d.selected[i] = !sel;
                    }
                }
            });
            ui.add_space(16.0);
            ui.columns(2, |cols| {
                cols[0].spacing_mut().item_spacing.y = 0.0;
                cols[1].spacing_mut().item_spacing.y = 0.0;
                field_label(&mut cols[0], tr.container);
                let cname = |c: &str| -> String {
                    if engine::container_is_subtitles(c) {
                        format!("{c} ({})", tr.subtitles.to_lowercase())
                    } else if engine::container_is_audio_only(c) {
                        format!("{c} ({})", tr.audio.to_lowercase())
                    } else {
                        c.to_string()
                    }
                };
                let w0 = cols[0].available_width();
                fluent::combo(&mut cols[0], p, "container", w0, cname(&d.container), |ui| {
                    for c in engine::CONTAINERS {
                        fluent::choice_value(ui, p, &mut d.container, c.to_string(), cname(c));
                    }
                });
                let w1 = cols[1].available_width();
                if engine::container_is_subtitles(&d.container) {
                    field_label(&mut cols[1], tr.sub_language);
                    let lname = |code: &str| -> String {
                        MULTI_SUB_LANGS.iter().find(|(c, _)| *c == code).map(|(c, n)| if *c == "all" { tr.sub_all.to_string() } else { n.to_string() }).unwrap_or_else(|| code.to_string())
                    };
                    fluent::combo(&mut cols[1], p, "mlang", w1, lname(&d.sub_lang), |ui| {
                        for (c, _) in MULTI_SUB_LANGS {
                            fluent::choice_value(ui, p, &mut d.sub_lang, c.to_string(), lname(c));
                        }
                    });
                    cols[1].add_space(8.0);
                    fluent::check_box(&mut cols[1], p, &mut d.sub_auto, tr.sub_include_auto);
                } else {
                    field_label(&mut cols[1], tr.video_quality);
                    let audio = engine::container_is_audio_only(&d.container);
                    let qname = |q: QualityPreference| -> String {
                        match q {
                            QualityPreference::Highest => tr.quality_highest.into(),
                            QualityPreference::Lowest => tr.quality_lowest.into(),
                            q => format!("≤ {}p", q.max_height().unwrap_or(0)),
                        }
                    };
                    cols[1].add_enabled_ui(!audio, |ui| {
                        fluent::combo(ui, p, "quality", w1, qname(d.quality), |ui| {
                            for q in QualityPreference::MENU {
                                fluent::choice_value(ui, p, &mut d.quality, q, qname(q));
                            }
                        });
                    });
                }
            });
            ui.add_space(12.0);
            ui.label(RichText::new(tr.select_all_hint).size(12.0).color(p.text2));
        });
        let count = d.selected.iter().filter(|s| **s).count();
        let (ok, cancel) = dialog_buttons(ui, p, &format!("{} ({count})", tr.download_button), tr.cancel_button, count > 0);
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
        let s = &mut self.settings;
        fluent::dialog_body(ui, p, Some(tr.settings_title), |ui| {
            egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let theme_name = |t: ThemePref| match t {
                    ThemePref::System => tr.theme_system,
                    ThemePref::Light => tr.theme_light,
                    ThemePref::Dark => tr.theme_dark,
                };
                setting_row(ui, tr.theme_label, tr.theme_tooltip, |ui| {
                    fluent::combo(ui, p, "theme", 170.0, theme_name(s.theme), |ui| {
                        for t in [ThemePref::System, ThemePref::Light, ThemePref::Dark] {
                            fluent::choice_value(ui, p, &mut s.theme, t, theme_name(t));
                        }
                    });
                });
                setting_row(ui, tr.language_label, tr.language_tooltip, |ui| {
                    fluent::combo(ui, p, "lang", 170.0, s.language.name(), |ui| {
                        for l in Language::ALL {
                            fluent::choice_value(ui, p, &mut s.language, l, l.name());
                        }
                    });
                });
                setting_row(ui, tr.auto_update_label, tr.auto_update_tooltip, |ui| {
                    fluent::toggle(ui, p, &mut s.auto_update);
                });
                setting_row(ui, tr.persist_auth_label, tr.persist_auth_tooltip, |ui| {
                    fluent::toggle(ui, p, &mut s.persist_auth);
                });
                path_row(ui, p, tr.ffmpeg_label, tr.ffmpeg_tooltip, tr, &mut s.ffmpeg_path, "ffmpeg.exe");
                path_row(ui, p, tr.ytdlp_label, tr.ytdlp_tooltip, tr, &mut s.ytdlp_path, "yt-dlp.exe");
                setting_row(ui, tr.inject_langs_label, tr.inject_langs_tooltip, |ui| {
                    fluent::toggle(ui, p, &mut s.inject_langs);
                });
                setting_row(ui, tr.inject_subs_label, tr.inject_subs_tooltip, |ui| {
                    fluent::toggle(ui, p, &mut s.inject_subs);
                });
                setting_row(ui, tr.inject_tags_label, tr.inject_tags_tooltip, |ui| {
                    fluent::toggle(ui, p, &mut s.inject_tags);
                });
                setting_row(ui, tr.skip_existing_label, tr.skip_existing_tooltip, |ui| {
                    fluent::toggle(ui, p, &mut s.skip_existing);
                });
                setting_row(ui, tr.template_label, tr.template_tooltip, |ui| {
                    fluent::line_edit(ui, p, &mut s.template, 170.0, None);
                });
                setting_row(ui, tr.parallel_label, tr.parallel_tooltip, |ui| {
                    ui.label(RichText::new(s.parallel.to_string()).color(p.text2));
                    fluent::slider(ui, p, &mut s.parallel, 1..=10, 150.0);
                });
            });
        });
        let close = fluent::dialog_footer(ui, p, &[(tr.close_button, Kind::Standard, true)], 2) == Some(0);
        !close
    }

    fn auth_dialog(&mut self, ui: &mut egui::Ui, p: &Pal) -> bool {
        let tr = self.tr();
        let auth = &mut self.settings.auth;
        fluent::dialog_body(ui, p, Some(tr.auth_title), |ui| {
            ui.set_min_height(300.0);
            let source_name = |a: &AuthSource| -> String {
                match a {
                    AuthSource::None => tr.auth_none.into(),
                    AuthSource::Browser(b) => browser_name(b),
                    AuthSource::CookiesFile(_) => tr.auth_cookies_file.into(),
                }
            };
            if auth.is_set() && !matches!(&*auth, AuthSource::CookiesFile(f) if f.is_empty()) {
                ui.vertical_centered(|ui| {
                    ui.add_space(24.0);
                    ui.label(icon(I_AUTH_OK, 64.0, p.success));
                    ui.add_space(20.0);
                    ui.label(RichText::new(tr.authenticated_text).size(18.0));
                    ui.add_space(4.0);
                    let detail = match &*auth {
                        AuthSource::CookiesFile(f) => f.clone(),
                        a => source_name(a),
                    };
                    ui.label(RichText::new(detail).color(p.text2));
                    ui.add_space(20.0);
                    if fluent::button(ui, p, tr.log_out, Kind::Standard, 0.0, true).clicked() {
                        *auth = AuthSource::None;
                    }
                });
            } else {
                ui.add(egui::Label::new(tr.auth_intro).wrap());
                ui.add_space(16.0);
                field_label(ui, tr.auth_source);
                let mut choice = auth.clone();
                let w = ui.available_width();
                fluent::combo(ui, p, "auth", w, source_name(&choice), |ui| {
                    fluent::choice_value(ui, p, &mut choice, AuthSource::None, tr.auth_none);
                    for b in engine::BROWSERS {
                        fluent::choice_value(ui, p, &mut choice, AuthSource::Browser(b.into()), browser_name(b));
                    }
                    let file = matches!(choice, AuthSource::CookiesFile(_));
                    if fluent::choice(ui, p, file, tr.auth_cookies_file).clicked() {
                        choice = AuthSource::CookiesFile(String::new());
                    }
                });
                if let AuthSource::CookiesFile(f) = &mut choice {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let w = ui.available_width() - 40.0;
                        fluent::line_edit(ui, p, f, w, Some("cookies.txt"));
                        if fluent::icon_button(ui, p, I_FOLDER, Vec2::splat(32.0), true).on_hover_text(tr.browse_tooltip).clicked() {
                            if let Some(path) = rfd::FileDialog::new().add_filter("cookies.txt", &["txt"]).pick_file() {
                                *f = path.display().to_string();
                            }
                        }
                    });
                }
                *auth = choice;
                ui.add_space(16.0);
                ui.add(egui::Label::new(RichText::new(tr.auth_browser_note).size(12.0).color(p.text2)).wrap());
            }
        });
        let close = fluent::dialog_footer(ui, p, &[(tr.close_button, Kind::Standard, true)], 2) == Some(0);
        !close
    }

    fn about_dialog(&mut self, ui: &mut egui::Ui, p: &Pal) -> bool {
        let tr = self.tr();
        fluent::dialog_body(ui, p, None, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.add(egui::Image::new(egui::include_image!("../assets/icon256.png")).fit_to_exact_size(Vec2::splat(56.0)));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(APP_NAME).font(bold(20.0)));
                    ui.label(RichText::new(format!("v{VERSION}")).color(p.text2));
                });
            });
            ui.add_space(16.0);
            ui.label(RichText::new(tr.about_tagline).font(bold(14.0)).color(p.accent_text));
            ui.add_space(12.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.add(egui::Label::new(tr.about_intro).wrap());
                ui.add_space(4.0);
                for line in tr.about_features.lines() {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.add_space(2.0);
                        ui.label(icon(I_CHECK, 14.0, p.accent_text));
                        let job = md_job(line, 14.0, p.text, FontId::proportional(14.0));
                        ui.add(egui::Label::new(job).wrap());
                    });
                }
                ui.add_space(6.0);
                let job = md_job(tr.about_credits, 12.0, p.text2, FontId::proportional(12.0));
                ui.add(egui::Label::new(job).wrap());
                ui.add(egui::Label::new(RichText::new(tr.about_legal).size(12.0).italics().color(p.text2)).wrap());
            });
        });
        match fluent::dialog_footer(ui, p, &[(tr.about_github, Kind::Standard, true), (tr.close_button, Kind::Standard, true)], 2) {
            Some(0) => {
                open_url(&format!("https://github.com/{}", update::APP_REPO));
                true
            }
            Some(_) => false,
            None => true,
        }
    }

    fn send_report(&mut self, ctx: &egui::Context, title: String, body: String) {
        self.sending_report = true;
        self.snack(self.tr().report_sending);
        let (tx, ctx2) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Event::ReportSent(update::send_report(&title, &body)));
            ctx2.request_repaint();
        });
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
        let mut close = false;
        fluent::dialog_body(ui, p, Some(tr.report_title), |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(tr.report_describe);
            fluent::text_box(ui, p, true, |ui| {
                let resp = ui.add(egui::TextEdit::multiline(&mut r.description).frame(false).desired_rows(4).desired_width(f32::INFINITY).margin(Margin::ZERO));
                (resp, ())
            });
            if r.video_url.is_some() {
                fluent::check_box(ui, p, &mut r.include_url, tr.report_include_url);
            }
            ui.add_space(4.0);
            ui.label(RichText::new(tr.report_preview).size(12.0).color(p.text2));
            let body = report_body(r);
            Frame::new().fill(p.card2).stroke(Stroke::new(1.0_f32, p.card_stroke)).corner_radius(4).inner_margin(8).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(RichText::new(&body).monospace().size(11.5).color(p.text2)).wrap());
                });
            });
            ui.add(egui::Label::new(RichText::new(tr.report_note).size(12.0).color(p.text2)).wrap());
            // Riserva: pagina GitHub già compilata (serve un account). HyperlinkButton WinUI.
            let link = ui.add(egui::Button::new(RichText::new(tr.report_open_github).size(12.0).color(p.accent_text)).frame(false));
            if link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
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
        });
        // Invio diretto tramite il Worker: nessun account richiesto.
        let buttons = [(tr.report_send, Kind::Accent, !self.sending_report), (tr.report_copy, Kind::Standard, true), (tr.cancel_button, Kind::Standard, true)];
        match fluent::dialog_footer(ui, p, &buttons, 3) {
            Some(0) => {
                self.send_report(ui.ctx(), report_title(r), report_body(r));
                close = true;
            }
            Some(1) => {
                ui.ctx().copy_text(format!("{}\n\n{}", report_title(r), report_body(r)));
                self.snack(tr.copied);
            }
            Some(_) => close = true,
            None => {}
        }
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
    fluent::dialog_body(ui, p, Some(&m.title), |ui| {
        ui.add(egui::Label::new(RichText::new(&m.text).color(p.text)).wrap());
    });
    match &m.cancel {
        Some(c) => {
            let (ok, cancel) = dialog_buttons(ui, p, &m.ok, c, true);
            if ok {
                (false, Some(m.on_ok))
            } else if cancel {
                (false, Some(m.on_cancel))
            } else {
                (true, None)
            }
        }
        None => {
            let close = fluent::dialog_footer(ui, p, &[(&m.ok, Kind::Accent, true)], 2) == Some(0);
            (!close, close.then_some(m.on_ok))
        }
    }
}

/// Pulsante primario (accento) e Annulla, come i due pulsanti di un ContentDialog.
fn dialog_buttons(ui: &mut egui::Ui, p: &Pal, ok: &str, cancel: &str, ok_enabled: bool) -> (bool, bool) {
    let clicked = fluent::dialog_footer(ui, p, &[(ok, Kind::Accent, ok_enabled), (cancel, Kind::Standard, true)], 2);
    let enter = ok_enabled && ui.input(|i| i.key_pressed(Key::Enter));
    (clicked == Some(0) || enter, clicked == Some(1))
}

/// Intestazione di un campo (Header di ComboBox/TextBox WinUI).
fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(text);
    ui.add_space(8.0);
}

/// Autore e durata con le icone Fluent.
fn video_meta(ui: &mut egui::Ui, p: &Pal, author: &str, duration: String, size: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(icon(I_PERSON, size, p.text2));
        ui.add(egui::Label::new(RichText::new(author).size(size).color(p.text2)).truncate());
        ui.add_space(10.0);
        ui.label(icon(I_CLOCK, size, p.text2));
        ui.label(RichText::new(duration).size(size).color(p.text2));
    });
}

/// Intestazione di colonna cliccabile (ordina), con chevron Fluent.
fn column_header(ui: &mut egui::Ui, p: &Pal, label: &str, sorted: Option<bool>, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::click());
    let color = if resp.hovered() { p.text } else { p.text2 };
    let g = egui::WidgetText::from(RichText::new(label).size(12.0)).into_galley(ui, Some(egui::TextWrapMode::Truncate), (width - 20.0).max(10.0), egui::TextStyle::Body);
    let text_w = g.size().x;
    ui.painter().galley(egui::pos2(rect.left(), rect.center().y - g.size().y / 2.0), g, color);
    if let Some(asc) = sorted {
        let ig = egui::WidgetText::from(icon(if asc { I_CHEVRON_UP } else { I_CHEVRON_DOWN }, 10.0, color)).into_galley(ui, None, f32::INFINITY, egui::TextStyle::Body);
        ui.painter().galley(egui::pos2(rect.left() + text_w + 6.0, rect.center().y - ig.size().y / 2.0), ig, color);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn setting_row(ui: &mut egui::Ui, label: &str, tooltip: &str, control: impl FnOnce(&mut egui::Ui)) {
    let r = ui
        .horizontal(|ui| {
            ui.set_min_height(36.0);
            ui.label(label);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                control(ui);
            });
        })
        .response;
    r.on_hover_text(plain(tooltip));
}

fn path_row(ui: &mut egui::Ui, p: &Pal, label: &str, tooltip: &str, tr: &Tr, value: &mut String, file: &str) {
    setting_row(ui, label, tooltip, |ui| {
        let browse = fluent::icon_button(ui, p, I_FOLDER, Vec2::splat(32.0), true).on_hover_text(tr.browse_tooltip);
        let reset = fluent::icon_button(ui, p, I_CLOSE, Vec2::splat(32.0), !value.is_empty()).on_hover_text(tr.reset_tooltip);
        fluent::line_edit(ui, p, value, 130.0, Some(tr.auto_detect));
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
    /// Sopra Mica lo sfondo resta trasparente; altrimenti è `SolidBackgroundFillColorBase`.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.mica {
            [0.0; 4]
        } else {
            visuals.panel_fill.to_normalized_gamma_f32()
        }
    }

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
        // Barra del titolo chiara/scura insieme al tema dell'app.
        let dark = ctx.style().visuals.dark_mode;
        if self.applied_dark != Some(dark) {
            if let Some(h) = self.hwnd {
                fluent::apply_window_style(h, dark, false);
            }
            self.applied_dark = Some(dark);
        }
        self.handle_events(ctx);
        self.schedule(ctx);
        let p = fluent::pal(dark, &self.accent, self.mica);
        self.header(ctx, &p);
        self.body(ctx, &p);
        self.dialogs(ctx, &p);
        self.snackbar(ctx, &p);
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
