//! Motore: tutto il dialogo con YouTube passa da yt-dlp, lanciato come processo.
//! La logica di risoluzione delle query e di scelta dei formati ricalca quella di
//! YoutubeDownloader (Tyrrrz), adattata ai dati che restituisce yt-dlp.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------ strumenti

/// Da dove leggere i cookie per l'autenticazione.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub enum AuthSource {
    #[default]
    None,
    Browser(String),
    CookiesFile(String),
}

impl AuthSource {
    pub fn is_set(&self) -> bool {
        !matches!(self, AuthSource::None)
    }
}

pub const BROWSERS: [&str; 6] = ["firefox", "edge", "chrome", "brave", "opera", "vivaldi"];

/// Percorsi e opzioni comuni a ogni chiamata di yt-dlp.
#[derive(Clone)]
pub struct Tools {
    pub ytdlp: PathBuf,
    pub ffmpeg: Option<PathBuf>,
    pub auth: AuthSource,
    /// Opzioni extra della "correzione rapida" scaricata da GitHub (già filtrate).
    pub extra_args: Vec<String>,
}

impl Tools {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.ytdlp);
        cmd.env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .args(["--encoding", "utf-8", "--no-config", "--remote-components", "ejs:github"]);
        if let Some(ff) = &self.ffmpeg {
            cmd.arg("--ffmpeg-location").arg(ffmpeg_location(ff));
        }
        match &self.auth {
            AuthSource::None => {}
            AuthSource::Browser(b) => {
                cmd.args(["--cookies-from-browser", b]);
            }
            AuthSource::CookiesFile(f) => {
                cmd.arg("--cookies").arg(f);
            }
        }
        cmd.args(&self.extra_args);
        hide_console(&mut cmd);
        cmd
    }
}

/// yt-dlp vuole la cartella se accanto a ffmpeg c'è anche ffprobe, altrimenti il file.
fn ffmpeg_location(ff: &Path) -> PathBuf {
    match ff.parent() {
        Some(dir) if dir.join("ffprobe.exe").is_file() => dir.to_path_buf(),
        _ => ff.to_path_buf(),
    }
}

pub fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
}

pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Cerca un eseguibile come fa l'originale per FFmpeg: cartella dell'app,
/// cartella corrente, poi le cartelle del PATH.
pub fn find_tool(file: &str) -> Option<PathBuf> {
    let mut dirs = vec![app_dir()];
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter().map(|d| d.join(file)).find(|p| p.is_file())
}

pub fn managed_ytdlp() -> PathBuf {
    app_dir().join("yt-dlp.exe")
}

pub fn resolve_ytdlp(custom: &str) -> Option<PathBuf> {
    if !custom.trim().is_empty() {
        return Some(PathBuf::from(custom.trim()));
    }
    let managed = managed_ytdlp();
    if managed.is_file() {
        return Some(managed);
    }
    find_tool("yt-dlp.exe")
}

pub fn resolve_ffmpeg(custom: &str) -> Option<PathBuf> {
    if !custom.trim().is_empty() {
        return Some(PathBuf::from(custom.trim()));
    }
    find_tool("ffmpeg.exe")
}

pub fn ytdlp_version(ytdlp: &Path) -> Option<String> {
    let mut cmd = Command::new(ytdlp);
    cmd.arg("--version");
    hide_console(&mut cmd);
    let out = cmd.output().ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// Esegue yt-dlp e restituisce il JSON stampato, oppure l'errore che ha scritto.
fn run_json(tools: &Tools, args: &[&str]) -> Result<Value, String> {
    let mut cmd = tools.command();
    cmd.args(["-J", "--no-warnings"]).args(args);
    let out = cmd.output().map_err(|e| format!("yt-dlp: {e}"))?;
    if !out.status.success() {
        return Err(error_text(&String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("JSON: {e}"))
}

/// Dall'output di errore tiene le righe "ERROR:", che sono quelle utili.
fn error_text(stderr: &str) -> String {
    let errors: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("ERROR:"))
        .map(|l| l.trim_start_matches("ERROR:").trim())
        .collect();
    if errors.is_empty() {
        stderr.trim().lines().last().unwrap_or("yt-dlp error").to_string()
    } else {
        errors.join("\n")
    }
}

// ------------------------------------------------------------------ video e query

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub author: String,
    pub duration: Option<f64>,
    pub upload_date: Option<String>,
    pub thumbnail: Option<String>,
}

impl Video {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }
    /// Miniatura piccola (120×90), come la "lowest quality" dell'originale.
    pub fn thumb_low(&self) -> String {
        format!("https://i.ytimg.com/vi/{}/default.jpg", self.id)
    }
    pub fn thumb_high(&self) -> String {
        self.thumbnail
            .clone()
            .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", self.id))
    }
    /// Durata come la mostra .NET (hh:mm:ss); `None` per le dirette.
    pub fn duration_text(&self) -> Option<String> {
        let s = self.duration? as u64;
        Some(format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60))
    }

    fn from_json(v: &Value) -> Option<Video> {
        let id = v.get("id")?.as_str()?.to_string();
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let live = text("live_status").as_deref() == Some("is_live");
        let author = text("channel").or_else(|| text("uploader")).unwrap_or_default();
        Some(Video {
            title: text("title").unwrap_or_else(|| id.clone()),
            author: clean_author(&author),
            duration: if live { None } else { v.get("duration").and_then(Value::as_f64) },
            upload_date: text("upload_date"),
            thumbnail: text("thumbnail"),
            id,
        })
    }
}

/// Nei risultati di ricerca YouTube a volte accoda al canale visualizzazioni e data.
fn clean_author(raw: &str) -> String {
    let first = raw.lines().next().unwrap_or("");
    let cut = first.find(['•', '·']).map(|i| &first[..i]).unwrap_or(first);
    cut.trim().to_string()
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum QueryKind {
    Video,
    Playlist,
    Channel,
    Search,
    Aggregate,
}

pub struct QueryResult {
    pub kind: QueryKind,
    /// Nome del video, della playlist, del canale o testo cercato (senza prefisso).
    pub name: String,
    pub videos: Vec<Video>,
    /// Per un singolo video, il JSON completo con i formati (evita una seconda chiamata).
    pub info: Option<Value>,
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Trasforma una riga della casella di ricerca in qualcosa che yt-dlp capisce.
fn query_target(query: &str) -> (String, bool) {
    let q = query.trim();
    if let Some(text) = q.strip_prefix('?') {
        return (format!("ytsearch20:{}", text.trim()), true);
    }
    let lower = q.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.contains("youtube.com/") || lower.contains("youtu.be/") {
        let url = if lower.starts_with("http") { q.to_string() } else { format!("https://{q}") };
        return (channel_uploads(&url), false);
    }
    if q.len() == 11 && q.chars().all(is_id_char) {
        return (format!("https://www.youtube.com/watch?v={q}"), false);
    }
    let playlist_prefixes = ["PL", "OL", "UU", "FL", "RD", "OLAK5uy_"];
    if q.len() > 12 && q.chars().all(is_id_char) && playlist_prefixes.iter().any(|p| q.starts_with(p)) {
        return (format!("https://www.youtube.com/playlist?list={q}"), false);
    }
    if q == "WL" || q == "LL" || q == "LM" {
        return (format!("https://www.youtube.com/playlist?list={q}"), false);
    }
    if q.len() == 24 && q.starts_with("UC") && q.chars().all(is_id_char) {
        return (format!("https://www.youtube.com/channel/{q}/videos"), false);
    }
    if q.starts_with('@') && q.len() > 1 && q[1..].chars().all(|c| is_id_char(c) || c == '.') {
        return (format!("https://www.youtube.com/{q}/videos"), false);
    }
    (format!("ytsearch20:{q}"), true)
}

/// Un canale senza scheda indicata mostra le schede (Video, Shorts…): come
/// l'originale, prendiamo i video caricati.
fn channel_uploads(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let path = trimmed.split("youtube.com").nth(1).unwrap_or("");
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let is_root = match parts.as_slice() {
        [h] => h.starts_with('@'),
        [k, _] => matches!(*k, "channel" | "c" | "user"),
        _ => false,
    };
    if is_root && !trimmed.contains('?') {
        format!("{trimmed}/videos")
    } else {
        url.to_string()
    }
}

pub fn resolve(tools: &Tools, query: &str) -> Result<QueryResult, String> {
    let (target, is_search) = query_target(query);
    let info = run_json(tools, &["--flat-playlist", &target])?;
    let kind_of = info.get("_type").and_then(Value::as_str).unwrap_or("video");
    if kind_of == "playlist" {
        let videos: Vec<Video> = info
            .get("entries")
            .and_then(Value::as_array)
            .map(|e| e.iter().filter_map(Video::from_json).collect())
            .unwrap_or_default();
        let text = |k: &str| info.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let (kind, name) = if is_search {
            (QueryKind::Search, query.trim().trim_start_matches('?').trim().to_string())
        } else if target.contains("/@") || target.contains("/channel/") || target.contains("/c/") || target.contains("/user/") {
            let ch = text("channel");
            (QueryKind::Channel, if ch.is_empty() { text("title") } else { ch })
        } else {
            (QueryKind::Playlist, text("title"))
        };
        Ok(QueryResult { kind, name, videos, info: None })
    } else {
        let video = Video::from_json(&info).ok_or("risposta di yt-dlp senza ID")?;
        Ok(QueryResult { kind: QueryKind::Video, name: video.title.clone(), videos: vec![video], info: Some(info) })
    }
}

/// Unisce i risultati di più righe, come `QueryResult.Aggregate` dell'originale.
pub fn aggregate(mut results: Vec<QueryResult>) -> QueryResult {
    if results.len() == 1 {
        return results.remove(0);
    }
    let count = results.len();
    let mut videos: Vec<Video> = Vec::new();
    for r in results {
        for v in r.videos {
            if !videos.iter().any(|x| x.id == v.id) {
                videos.push(v);
            }
        }
    }
    QueryResult { kind: QueryKind::Aggregate, name: count.to_string(), videos, info: None }
}

pub fn video_info(tools: &Tools, video: &Video) -> Result<Value, String> {
    run_json(tools, &["--no-playlist", &video.url()])
}

// ------------------------------------------------------------------ formati

/// Contenitori per i download multipli: video, solo audio, solo sottotitoli.
pub const CONTAINERS: [&str; 11] = ["mp4", "webm", "mp3", "m4a", "ogg", "flac", "wav", "srt", "vtt", "txt", "raw"];

/// Formati solo audio: (estensione, nome per `--audio-format`, flusso sorgente preferito).
/// MP3 universale, M4A senza ricompressione, OGG (Vorbis) libero, FLAC senza
/// perdita, WAV per il montaggio.
pub const AUDIO_FORMATS: [(&str, &str, &str); 5] = [
    ("mp3", "mp3", "webm"),
    ("m4a", "m4a", "mp4"),
    ("ogg", "vorbis", "webm"),
    ("flac", "flac", "webm"),
    ("wav", "wav", "webm"),
];

/// `raw` = il file così come lo fornisce YouTube, senza conversione.
pub const SUBTITLE_FORMATS: [&str; 4] = ["srt", "vtt", "txt", "raw"];

pub fn container_is_audio_only(c: &str) -> bool {
    AUDIO_FORMATS.iter().any(|(ext, _, _)| *ext == c)
}

pub fn container_is_subtitles(c: &str) -> bool {
    SUBTITLE_FORMATS.contains(&c)
}

/// Lingue dei sottotitoli: codice per yt-dlp e se sono quelli generati da YouTube.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubSpec {
    /// Espressione `--sub-langs` (es. `it.*`, `all,-live_chat`).
    pub langs: String,
    pub auto: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DownloadOption {
    pub container: String,
    pub audio_only: bool,
    pub format_ids: Vec<String>,
    pub height: Option<u32>,
    pub fps: Option<u32>,
    pub upscaled: bool,
    /// Solo sottotitoli: niente video né audio.
    #[serde(default)]
    pub subs: Option<SubSpec>,
}

impl DownloadOption {
    pub fn subtitles(format: &str, spec: SubSpec) -> Self {
        DownloadOption { container: format.into(), audio_only: false, format_ids: Vec::new(), height: None, fps: None, upscaled: false, subs: Some(spec) }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SubKind {
    /// Caricati dall'autore del video.
    Manual,
    /// Generati da YouTube nella lingua parlata.
    Auto,
    /// Generati da YouTube e tradotti automaticamente.
    Translated,
}

#[derive(Clone, Debug)]
pub struct SubLanguage {
    pub code: String,
    pub name: String,
    pub kind: SubKind,
}

/// Lingue dei sottotitoli disponibili per un video, nell'ordine: dell'autore,
/// automatici originali, traduzioni automatiche.
pub fn subtitle_languages(info: &Value) -> Vec<SubLanguage> {
    let mut out = Vec::new();
    let mut collect = |key: &str, auto: bool| {
        if let Some(map) = info.get(key).and_then(Value::as_object) {
            for (code, tracks) in map {
                if code == "live_chat" {
                    continue;
                }
                let name = tracks
                    .as_array()
                    .and_then(|t| t.iter().find_map(|x| x.get("name").and_then(Value::as_str)))
                    .unwrap_or(code)
                    .to_string();
                let kind = match (auto, code.ends_with("-orig")) {
                    (false, _) => SubKind::Manual,
                    (true, true) => SubKind::Auto,
                    (true, false) => SubKind::Translated,
                };
                out.push(SubLanguage { code: code.clone(), name, kind });
            }
        }
    };
    collect("subtitles", false);
    collect("automatic_captions", true);
    let rank = |k: SubKind| match k {
        SubKind::Manual => 0,
        SubKind::Auto => 1,
        SubKind::Translated => 2,
    };
    out.sort_by(|a, b| rank(a.kind).cmp(&rank(b.kind)).then(a.name.cmp(&b.name)));
    out
}

impl DownloadOption {
    pub fn is_subtitles(&self) -> bool {
        self.subs.is_some()
    }
    pub fn quality_label(&self) -> String {
        match (self.height, self.fps) {
            (Some(h), Some(f)) if f > 30 => format!("{h}p{f}"),
            (Some(h), _) => format!("{h}p"),
            _ => String::new(),
        }
    }
    fn quality_key(&self) -> (u32, u32) {
        (self.height.unwrap_or(0), self.fps.unwrap_or(0))
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum QualityPreference {
    Lowest,
    UpTo360p,
    UpTo480p,
    UpTo720p,
    UpTo1080p,
    Highest,
}

impl QualityPreference {
    /// Nell'ordine del menu dell'originale (dalla migliore alla peggiore).
    pub const MENU: [QualityPreference; 6] = [
        QualityPreference::Highest,
        QualityPreference::UpTo1080p,
        QualityPreference::UpTo720p,
        QualityPreference::UpTo480p,
        QualityPreference::UpTo360p,
        QualityPreference::Lowest,
    ];
    pub fn max_height(self) -> Option<u32> {
        match self {
            QualityPreference::UpTo1080p => Some(1080),
            QualityPreference::UpTo720p => Some(720),
            QualityPreference::UpTo480p => Some(480),
            QualityPreference::UpTo360p => Some(360),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preference {
    pub container: String,
    pub quality: QualityPreference,
    /// Usato solo se `container` è un formato di sottotitoli.
    #[serde(default)]
    pub subs: Option<SubSpec>,
}

impl Preference {
    /// Port di `VideoDownloadPreference.TryGetBestOption`.
    pub fn best(&self, options: &[DownloadOption]) -> Option<DownloadOption> {
        if let Some(spec) = &self.subs {
            return Some(DownloadOption::subtitles(&self.container, spec.clone()));
        }
        if container_is_audio_only(&self.container) {
            return options.iter().find(|o| o.container == self.container).cloned();
        }
        let mut ordered: Vec<&DownloadOption> = options.iter().filter(|o| o.container == self.container).collect();
        ordered.sort_by_key(|o| o.quality_key());
        let pick = match self.quality {
            QualityPreference::Highest => ordered.last().copied(),
            QualityPreference::Lowest => ordered.first().copied(),
            q => {
                let max = q.max_height().unwrap_or(u32::MAX);
                ordered.iter().rev().find(|o| o.height.is_some_and(|h| h <= max)).copied()
            }
        };
        pick.or(ordered.first().copied()).cloned()
    }
}

struct Fmt<'a> {
    id: &'a str,
    ext: &'a str,
    has_video: bool,
    has_audio: bool,
    height: Option<u32>,
    fps: Option<u32>,
    abr: f64,
    codec_rank: u8,
    language: Option<&'a str>,
    is_default_lang: bool,
    upscaled: bool,
}

fn parse_formats(info: &Value) -> Vec<Fmt<'_>> {
    let Some(list) = info.get("formats").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|f| {
            let s = |k: &str| f.get(k).and_then(Value::as_str);
            let id = s("format_id")?;
            let ext = s("ext")?;
            let proto = s("protocol").unwrap_or("");
            let note = s("format_note").unwrap_or("").to_ascii_lowercase();
            // Niente storyboard, flussi HLS duplicati o versioni "DRC" (volume compresso).
            if note.contains("storyboard") || proto.contains("m3u8") || id.ends_with("-drc") || ext == "mhtml" {
                return None;
            }
            let vcodec = s("vcodec").unwrap_or("none");
            let acodec = s("acodec").unwrap_or("none");
            let has_video = vcodec != "none";
            // Prima i codec che il PC decodifica meglio: H.264, poi VP9, poi AV1.
            let codec_rank = if vcodec.starts_with("avc1") { 3 } else if vcodec.starts_with("vp") { 2 } else { 1 };
            let pref = f.get("language_preference").and_then(Value::as_i64).unwrap_or(-1);
            Some(Fmt {
                id,
                ext,
                has_video,
                has_audio: acodec != "none",
                height: f.get("height").and_then(Value::as_u64).map(|h| h as u32),
                fps: f.get("fps").and_then(Value::as_f64).map(|x| x.round() as u32),
                abr: f.get("abr").and_then(Value::as_f64).or_else(|| f.get("tbr").and_then(Value::as_f64)).unwrap_or(0.0),
                codec_rank,
                language: s("language"),
                is_default_lang: pref >= 10 || note.contains("original") || note.contains("default"),
                upscaled: note.contains("upscal"),
            })
        })
        .collect()
}

/// Port di `VideoDownloadOption.ResolveAll`: un'opzione per ogni qualità video
/// (con l'audio migliore abbinato) più le opzioni solo audio.
pub fn download_options(info: &Value, include_languages: bool) -> Vec<DownloadOption> {
    let formats = parse_formats(info);
    let audio_for = |container: &str| -> Vec<&Fmt> {
        let wanted = if container == "mp4" { "m4a" } else { container };
        let mut a: Vec<&Fmt> = formats.iter().filter(|f| !f.has_video && f.has_audio).collect();
        a.sort_by(|x, y| {
            (y.ext == wanted, y.is_default_lang || y.language.is_none())
                .cmp(&(x.ext == wanted, x.is_default_lang || x.language.is_none()))
                .then(y.abr.total_cmp(&x.abr))
        });
        a
    };

    let mut videos: Vec<&Fmt> = formats
        .iter()
        .filter(|f| f.has_video && f.height.is_some() && (f.ext == "mp4" || f.ext == "webm"))
        .collect();
    videos.sort_by(|a, b| (b.height, b.fps, b.codec_rank).cmp(&(a.height, a.fps, a.codec_rank)));

    let mut options: Vec<DownloadOption> = Vec::new();
    let push = |o: DownloadOption, options: &mut Vec<DownloadOption>| {
        let dup = options
            .iter()
            .any(|x| x.container == o.container && x.audio_only == o.audio_only && x.quality_key() == o.quality_key() && x.upscaled == o.upscaled);
        if !dup {
            options.push(o);
        }
    };
    for v in &videos {
        let mut ids = vec![v.id.to_string()];
        if !v.has_audio {
            let audios = audio_for(v.ext);
            let mut langs: Vec<&Fmt> = Vec::new();
            if include_languages {
                for a in audios.iter().filter(|a| a.language.is_some() && (a.ext == if v.ext == "mp4" { "m4a" } else { "webm" })) {
                    if !langs.iter().any(|l| l.language == a.language) {
                        langs.push(a);
                    }
                }
                langs.sort_by_key(|a| !a.is_default_lang);
            }
            if langs.len() > 1 {
                ids.extend(langs.iter().map(|a| a.id.to_string()));
            } else if let Some(a) = audios.first() {
                ids.push(a.id.to_string());
            } else {
                continue;
            }
        }
        push(
            DownloadOption {
                container: v.ext.to_string(),
                audio_only: false,
                format_ids: ids,
                height: v.height,
                fps: v.fps,
                upscaled: v.upscaled,
                subs: None,
            },
            &mut options,
        );
    }
    let audio_only = |container: &str, source: &str, options: &mut Vec<DownloadOption>| {
        if let Some(a) = audio_for(source).first() {
            push(
                DownloadOption {
                    container: container.into(),
                    audio_only: true,
                    format_ids: vec![a.id.to_string()],
                    height: None,
                    fps: None,
                    upscaled: false,
                    subs: None,
                },
                options,
            );
        }
    };
    for (ext, _, source) in AUDIO_FORMATS {
        audio_only(ext, source, &mut options);
    }
    options
}

// ------------------------------------------------------------------ nomi file

/// Port di `FileNameTemplate.Apply` + `Path.EscapeFileName`.
pub fn apply_template(template: &str, video: &Video, container: &str, number: Option<&str>) -> String {
    let date = video
        .upload_date
        .as_deref()
        .filter(|d| d.len() == 8)
        .map(|d| format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]))
        .unwrap_or_default();
    let name = template
        .replace("$numc", number.unwrap_or(""))
        .replace("$num", &number.map(|n| format!("[{n}]")).unwrap_or_default())
        .replace("$id", &video.id)
        .replace("$title", &video.title)
        .replace("$author", &video.author)
        .replace("$uploadDate", &date);
    let clean: String = name
        .trim()
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    let clean = clean.trim_end_matches(['.', ' ']).to_string();
    format!("{}.{container}", if clean.is_empty() { video.id.clone() } else { clean })
}

/// Se il file esiste aggiunge " (1)", " (2)"…, come `Path.EnsureUniqueFilePath`.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new("."));
    (1..)
        .map(|i| dir.join(format!("{stem} ({i}).{ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

// ------------------------------------------------------------------ download

pub struct DownloadFlags {
    pub subtitles: bool,
    pub tags: bool,
}

/// Avanzamento condiviso con l'interfaccia.
#[derive(Default)]
pub struct Progress {
    /// 0..1, oppure `None` se indeterminato (avvio, unione dei flussi).
    pub fraction: Option<f32>,
    pub processing: bool,
}

pub enum DownloadError {
    Canceled,
    Failed(String),
}

/// Scarica `option` di `video` in `final_path`. Il file viene prima scritto con un
/// nome provvisorio accanto alla destinazione e rinominato solo a fine lavoro.
/// Restituisce il file prodotto (per i sottotitoli può differire da `final_path`).
pub fn download(
    tools: &Tools,
    video: &Video,
    option: &DownloadOption,
    final_path: &Path,
    flags: &DownloadFlags,
    progress: &Arc<Mutex<Progress>>,
    cancel: &Arc<AtomicBool>,
) -> Result<PathBuf, DownloadError> {
    let dir = final_path.parent().unwrap_or(Path::new("."));
    let stem = final_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| video.id.clone());
    let temp_stem = format!("{stem}.ytdr");
    let template = dir.join(format!("{}.%(ext)s", temp_stem.replace('%', "%%")));

    let mut cmd = tools.command();
    cmd.args(["--no-playlist", "--no-mtime", "--newline", "--quiet", "--progress", "--no-part"])
        .args(["--progress-template", "download:YTDR_PROG %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(info.format_id)s"])
        .args(["--progress-template", "postprocess:YTDR_PP %(progress.status)s"])
        .arg("-o")
        .arg(&template);
    let c = option.container.as_str();
    if let Some(spec) = &option.subs {
        cmd.args(["--skip-download", "--write-subs", "--sub-format", "best", "--sub-langs", &spec.langs]);
        if spec.auto {
            cmd.arg("--write-auto-subs");
        }
        // Il TXT si ricava da un SRT, togliendo numeri e orari; il RAW resta com'è.
        match c {
            "raw" => {}
            "txt" => {
                cmd.args(["--convert-subs", "srt"]);
            }
            _ => {
                cmd.args(["--convert-subs", c]);
            }
        }
    } else {
        cmd.args(["--print", "after_move:YTDR_FILE %(filepath)s"]).arg("-f").arg(option.format_ids.join("+"));
    }
    if option.is_subtitles() {
        // niente opzioni audio/video
    } else if option.audio_only {
        if let Some((_, codec, _)) = AUDIO_FORMATS.iter().find(|(ext, _, _)| *ext == c) {
            cmd.args(["-x", "--audio-format", codec, "--audio-quality", "0"]);
        }
    } else {
        if option.format_ids.len() > 1 {
            cmd.args(["--merge-output-format", c]);
        }
        if option.format_ids.len() > 2 {
            cmd.arg("--audio-multistreams");
        }
        if flags.subtitles {
            cmd.args(["--write-subs", "--embed-subs", "--sub-langs", "all,-live_chat"]);
        }
    }
    if flags.tags && !option.is_subtitles() {
        cmd.arg("--embed-metadata");
        // La copertina si può incorporare solo in questi contenitori.
        if matches!(c, "mp4" | "mp3" | "m4a") {
            cmd.args(["--embed-thumbnail", "--convert-thumbnails", "jpg"]);
        }
    }
    cmd.arg(video.url()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let cleanup = || {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                if e.file_name().to_string_lossy().starts_with(&temp_stem) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    };

    let mut child = cmd.spawn().map_err(|e| DownloadError::Failed(format!("yt-dlp: {e}")))?;
    let pid = child.id();
    let stderr = child.stderr.take();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut e) = stderr {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    // Un thread controlla l'annullamento e chiude yt-dlp con i suoi figli (ffmpeg).
    let done = Arc::new(AtomicBool::new(false));
    {
        let (done, cancel) = (done.clone(), cancel.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    let mut kill = Command::new("taskkill");
                    kill.args(["/PID", &pid.to_string(), "/T", "/F"]);
                    hide_console(&mut kill);
                    let _ = kill.output();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        });
    }

    let mut produced: Option<PathBuf> = None;
    let mut seen: Vec<String> = Vec::new();
    let n = option.format_ids.len().max(1) as f32;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(rest) = line.strip_prefix("YTDR_PROG ") {
                let p: Vec<&str> = rest.split_whitespace().collect();
                let num = |i: usize| p.get(i).and_then(|x| x.parse::<f64>().ok());
                let fid = p.get(3).unwrap_or(&"").to_string();
                if !seen.contains(&fid) {
                    seen.push(fid.clone());
                }
                let idx = option.format_ids.iter().position(|x| *x == fid).unwrap_or(seen.len() - 1) as f32;
                let total = num(1).or(num(2)).filter(|t| *t > 0.0);
                let mut pr = progress.lock().unwrap();
                pr.processing = false;
                pr.fraction = match (num(0), total) {
                    (Some(d), Some(t)) => Some(((idx + (d / t) as f32) / n).clamp(0.0, 1.0)),
                    _ => None,
                };
                // Finito l'ultimo flusso restano unione, sottotitoli e copertina:
                // meglio "Elaborazione..." che un 100% fermo.
                if pr.fraction.is_some_and(|f| f >= 0.999) {
                    pr.processing = true;
                    pr.fraction = None;
                }
            } else if line.starts_with("YTDR_PP ") {
                let mut pr = progress.lock().unwrap();
                pr.processing = true;
                pr.fraction = None;
            } else if let Some(path) = line.strip_prefix("YTDR_FILE ") {
                produced = Some(PathBuf::from(path.trim()));
            }
        }
    }
    let status = child.wait();
    done.store(true, Ordering::Relaxed);
    let stderr = err_thread.join().unwrap_or_default();

    if cancel.load(Ordering::Relaxed) {
        cleanup();
        return Err(DownloadError::Canceled);
    }
    let ok = status.map(|s| s.success()).unwrap_or(false);
    if !ok {
        cleanup();
        return Err(DownloadError::Failed(error_text(&stderr)));
    }
    if option.is_subtitles() {
        return finish_subtitles(dir, &stem, &temp_stem, c).map_err(|e| {
            cleanup();
            DownloadError::Failed(e)
        });
    }
    match produced.filter(|p| p.is_file()) {
        Some(file) => {
            let _ = std::fs::remove_file(final_path);
            std::fs::rename(&file, final_path).map_err(|e| {
                cleanup();
                DownloadError::Failed(format!("{}: {e}", final_path.display()))
            })?;
            cleanup();
            Ok(final_path.to_path_buf())
        }
        None => {
            cleanup();
            Err(DownloadError::Failed(error_text(&stderr)))
        }
    }
}

/// I sottotitoli arrivano come `<nome>.ytdr.<lingua>.<ext>`: li rinomina in
/// `<nome>.<lingua>.<formato>` e, per il TXT, li riduce a testo semplice.
fn finish_subtitles(dir: &Path, stem: &str, temp_stem: &str, format: &str) -> Result<PathBuf, String> {
    let source_ext = if format == "txt" { "srt" } else { format };
    let prefix = format!("{temp_stem}.");
    let mut produced = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix(&prefix) else { continue };
        let Some((lang, ext)) = rest.rsplit_once('.') else { continue };
        if format != "raw" && ext != source_ext {
            // Sottotitoli rimasti nel formato originale (conversione non riuscita).
            let _ = std::fs::remove_file(e.path());
            continue;
        }
        let out_ext = if format == "raw" { ext } else { format };
        let target = unique_path(&dir.join(format!("{stem}.{lang}.{out_ext}")));
        if format == "txt" {
            let text = srt_to_text(&std::fs::read_to_string(e.path()).map_err(|e| e.to_string())?);
            std::fs::write(&target, text).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(e.path());
        } else {
            std::fs::rename(e.path(), &target).map_err(|e| e.to_string())?;
        }
        produced.push(target);
    }
    produced.sort();
    produced.into_iter().next().ok_or_else(|| "Nessun sottotitolo disponibile per le lingue scelte / No subtitles available for the selected languages".into())
}

/// Da SRT a testo: via numeri, orari e tag; le righe ripetute dei sottotitoli
/// automatici (che "scorrono") vengono tenute una volta sola.
pub fn srt_to_text(srt: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in srt.lines() {
        let l = line.trim();
        if l.is_empty() || l.chars().all(|c| c.is_ascii_digit()) || l.contains("-->") {
            continue;
        }
        let mut clean = String::new();
        let mut in_tag = false;
        for ch in l.chars() {
            match ch {
                '<' | '{' => in_tag = true,
                '>' | '}' => in_tag = false,
                _ if !in_tag => clean.push(ch),
                _ => {}
            }
        }
        let clean = clean.trim().to_string();
        if !clean.is_empty() && lines.last() != Some(&clean) {
            lines.push(clean);
        }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prove con YouTube vero (rete + yt-dlp nel PATH): `cargo test -- --ignored`.
    fn live_tools() -> Tools {
        Tools { ytdlp: find_tool("yt-dlp.exe").expect("yt-dlp nel PATH"), ffmpeg: find_tool("ffmpeg.exe"), auth: AuthSource::None, extra_args: Vec::new() }
    }

    /// Video MP4 con sottotitoli e tag incorporati (le impostazioni predefinite).
    #[test]
    #[ignore]
    fn live_video_mp4() {
        let tools = live_tools();
        let r = resolve(&tools, "jNQXAC9IVRw").expect("video");
        let options = download_options(r.info.as_ref().unwrap(), true);
        let best = options.iter().find(|o| o.container == "mp4" && !o.audio_only).unwrap();
        println!("formato scelto: {:?}", best.format_ids);
        let dir = std::env::temp_dir().join("ytdr-test-mp4");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let progress = Arc::new(Mutex::new(Progress::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let flags = DownloadFlags { subtitles: true, tags: true };
        match download(&tools, &r.videos[0], best, &dir.join("zoo.mp4"), &flags, &progress, &cancel) {
            Ok(p) => println!("OK {} ({} byte)", p.display(), std::fs::metadata(&p).unwrap().len()),
            Err(DownloadError::Failed(e)) => panic!("ERRORE: {e}"),
            Err(DownloadError::Canceled) => panic!("annullato"),
        }
    }

    #[test]
    #[ignore]
    fn live_resolve_and_download() {
        let tools = live_tools();
        let r = resolve(&tools, "jNQXAC9IVRw").expect("video");
        assert_eq!(r.kind, QueryKind::Video);
        assert_eq!(r.videos[0].title, "Me at the zoo");
        let info = r.info.as_ref().unwrap();
        let options = download_options(info, true);
        println!("opzioni: {:?}", options.iter().map(|o| format!("{} {}", o.quality_label(), o.container)).collect::<Vec<_>>());
        assert!(options.iter().any(|o| o.container == "mp4" && !o.audio_only));
        let langs = subtitle_languages(info);
        println!("sottotitoli: {:?}", langs.iter().take(5).map(|l| (&l.code, &l.name, l.kind)).collect::<Vec<_>>());
        assert!(langs.iter().any(|l| l.kind == SubKind::Manual));

        let s = resolve(&tools, "?me at the zoo").expect("ricerca");
        assert_eq!(s.kind, QueryKind::Search);
        assert!(s.videos.len() > 3);
        let c = resolve(&tools, "@jawed").expect("canale");
        assert_eq!(c.kind, QueryKind::Channel);
        assert!(c.videos.iter().any(|v| v.id == "jNQXAC9IVRw"));

        let dir = std::env::temp_dir().join("ytdr-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let video = &r.videos[0];
        let flags = DownloadFlags { subtitles: false, tags: true };
        let progress = Arc::new(Mutex::new(Progress::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let mp3 = options.iter().find(|o| o.container == "mp3").unwrap();
        let out = download(&tools, video, mp3, &dir.join("zoo.mp3"), &flags, &progress, &cancel).map_err(|e| match e {
            DownloadError::Canceled => "annullato".to_string(),
            DownloadError::Failed(m) => m,
        });
        let out = out.expect("download mp3");
        assert!(std::fs::metadata(&out).unwrap().len() > 50_000, "mp3 troppo piccolo");
        let subs = DownloadOption::subtitles("txt", SubSpec { langs: "en".into(), auto: false });
        let txt = download(&tools, video, &subs, &dir.join("zoo.txt"), &flags, &progress, &cancel).map_err(|e| match e {
            DownloadError::Canceled => "annullato".to_string(),
            DownloadError::Failed(m) => m,
        });
        let txt = txt.expect("sottotitoli");
        let text = std::fs::read_to_string(&txt).unwrap();
        println!("{} →\n{text}", txt.display());
        assert!(text.to_lowercase().contains("elephant"));
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).filter(|n| n.to_string_lossy().contains(".ytdr")).collect();
        assert!(leftovers.is_empty(), "file temporanei rimasti: {leftovers:?}");
    }

    #[test]
    fn query_targets() {
        assert_eq!(query_target("?gatti").0, "ytsearch20:gatti");
        assert_eq!(query_target("jNQXAC9IVRw").0, "https://www.youtube.com/watch?v=jNQXAC9IVRw");
        assert_eq!(query_target("@jawed").0, "https://www.youtube.com/@jawed/videos");
        assert_eq!(query_target("https://www.youtube.com/@jawed").0, "https://www.youtube.com/@jawed/videos");
        assert_eq!(query_target("https://www.youtube.com/@jawed/shorts").0, "https://www.youtube.com/@jawed/shorts");
        assert_eq!(query_target("youtu.be/jNQXAC9IVRw").0, "https://youtu.be/jNQXAC9IVRw");
        assert!(query_target("musica rilassante").1);
    }

    #[test]
    fn template_and_author() {
        let v = Video { id: "abc".into(), title: "A/B: test?".into(), author: "Me".into(), duration: Some(3725.0), upload_date: Some("20050424".into()), thumbnail: None };
        assert_eq!(apply_template("$title", &v, "mp4", None), "A_B_ test_.mp4");
        assert_eq!(apply_template("$num $author - $uploadDate", &v, "mp3", Some("07")), "[07] Me - 2005-04-24.mp3");
        assert_eq!(v.duration_text().unwrap(), "01:02:05");
        assert_eq!(clean_author("TopX English • 6.5B views\n\n..."), "TopX English");
    }

    #[test]
    fn options_from_formats() {
        let info: Value = serde_json::from_str(r#"{"formats":[
            {"format_id":"140","ext":"m4a","vcodec":"none","acodec":"mp4a","abr":129.0,"protocol":"https"},
            {"format_id":"251","ext":"webm","vcodec":"none","acodec":"opus","abr":106.0,"protocol":"https"},
            {"format_id":"18","ext":"mp4","vcodec":"avc1","acodec":"mp4a","height":360,"fps":30,"protocol":"https"},
            {"format_id":"137","ext":"mp4","vcodec":"avc1.640028","acodec":"none","height":1080,"fps":30,"protocol":"https"},
            {"format_id":"399","ext":"mp4","vcodec":"av01","acodec":"none","height":1080,"fps":30,"protocol":"https"},
            {"format_id":"248","ext":"webm","vcodec":"vp9","acodec":"none","height":1080,"fps":30,"protocol":"https"}
        ]}"#).unwrap();
        let o = download_options(&info, true);
        assert_eq!(o[0].format_ids, vec!["137", "140"]); // H.264 preferito ad AV1
        assert!(o.iter().any(|x| x.container == "webm" && x.format_ids == vec!["248", "251"]));
        assert!(o.iter().any(|x| x.container == "mp4" && x.format_ids == vec!["18"]));
        assert_eq!(o.iter().filter(|x| x.audio_only).count(), 5);
        assert_eq!(srt_to_text("1\n00:00:01,000 --> 00:00:02,000\n<i>Ciao</i>\n\n2\n00:00:02,000 --> 00:00:03,000\nCiao\nmondo\n"), "Ciao\nmondo\n");
        let p = Preference { container: "mp4".into(), quality: QualityPreference::UpTo720p, subs: None };
        assert_eq!(p.best(&o).unwrap().height, Some(360));
        let p = Preference { container: "mp3".into(), quality: QualityPreference::Highest, subs: None };
        assert_eq!(p.best(&o).unwrap().format_ids, vec!["251"]);
    }
}
