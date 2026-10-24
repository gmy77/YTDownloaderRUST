//! Aggiornamenti, su tre livelli:
//! 1. yt-dlp (la parte che si rompe quando YouTube cambia) → `yt-dlp -U`;
//! 2. "correzione rapida": `patch.json` nel repository, con opzioni extra per yt-dlp;
//! 3. l'app stessa, dalle release GitHub, con verifica SHA-256.
//! Più lo scaricamento iniziale di yt-dlp e FFmpeg se mancano.

use crate::engine::{self, hide_console};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const APP_REPO: &str = "gmy77/YTDownloaderRUST";
pub const APP_ASSET: &str = "YTDownloaderRUST.exe";
const PATCH_URL: &str = "https://raw.githubusercontent.com/gmy77/YTDownloaderRUST/main/patch.json";

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let mut req = ehttp::Request::get(url);
    req.headers.insert("User-Agent", "YTDownloaderRUST");
    let resp = ehttp::fetch_blocking(&req)?;
    if resp.ok {
        Ok(resp.bytes)
    } else {
        Err(format!("HTTP {} — {url}", resp.status))
    }
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Scrive accanto alla destinazione e poi rinomina, così un download interrotto
/// non lascia un eseguibile a metà.
fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("download");
    std::fs::write(&tmp, data).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

struct Release {
    tag: String,
    /// (nome, url, sha256 se GitHub lo fornisce)
    assets: Vec<(String, String, Option<String>)>,
}

fn latest_release(repo: &str) -> Result<Option<Release>, String> {
    let mut req = ehttp::Request::get(format!("https://api.github.com/repos/{repo}/releases/latest"));
    req.headers.insert("User-Agent", "YTDownloaderRUST");
    req.headers.insert("Accept", "application/vnd.github+json");
    let resp = ehttp::fetch_blocking(&req)?;
    if resp.status == 404 {
        return Ok(None);
    }
    if !resp.ok {
        return Err(format!("GitHub: HTTP {}", resp.status));
    }
    let v: Value = serde_json::from_slice(&resp.bytes).map_err(|e| e.to_string())?;
    let tag = v.get("tag_name").and_then(Value::as_str).unwrap_or("").to_string();
    let assets = v
        .get("assets")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let s = |k: &str| x.get(k).and_then(Value::as_str).map(str::to_string);
                    let digest = s("digest").and_then(|d| d.strip_prefix("sha256:").map(str::to_string));
                    Some((s("name")?, s("browser_download_url")?, digest))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(Release { tag, assets }))
}

/// Scarica un allegato e ne verifica l'hash (dal campo `digest` di GitHub o da un
/// file di checksum della release).
fn download_asset(rel: &Release, name: &str, sums_file: Option<&str>) -> Result<Vec<u8>, String> {
    let (_, url, digest) = rel.assets.iter().find(|(n, _, _)| n == name).ok_or_else(|| format!("{name} non presente nella release {}", rel.tag))?;
    let data = http_get(url)?;
    let expected = match digest {
        Some(d) => Some(d.to_ascii_lowercase()),
        None => match sums_file.and_then(|f| rel.assets.iter().find(|(n, _, _)| n == f)) {
            Some((_, sums_url, _)) => {
                let sums = String::from_utf8_lossy(&http_get(sums_url)?).to_string();
                sums.lines()
                    .find(|l| l.trim_end().ends_with(name) && l.split_whitespace().last().map(|x| x.trim_start_matches('*')) == Some(name))
                    .and_then(|l| l.split_whitespace().next().map(str::to_ascii_lowercase))
            }
            None => None,
        },
    };
    match expected {
        Some(h) if h != sha256_hex(&data) => Err(format!("{name}: hash SHA-256 non corrispondente, file scartato")),
        _ => Ok(data),
    }
}

// ------------------------------------------------------------------ yt-dlp

/// Scarica l'ultima versione ufficiale di yt-dlp accanto all'app.
pub fn download_ytdlp() -> Result<PathBuf, String> {
    let rel = latest_release("yt-dlp/yt-dlp")?.ok_or("release di yt-dlp non trovata")?;
    let data = download_asset(&rel, "yt-dlp.exe", Some("SHA2-256SUMS"))?;
    let path = engine::managed_ytdlp();
    write_atomic(&path, &data)?;
    Ok(path)
}

/// Aggiorna yt-dlp. La copia gestita dall'app si aggiorna da sola (`-U`); una
/// copia installata con pip non può farlo, quindi se ne scarica una gestita.
/// Restituisce (versione prima, versione dopo).
pub fn update_ytdlp(current: Option<&Path>) -> Result<(String, String), String> {
    let managed = engine::managed_ytdlp();
    let before = current.and_then(engine::ytdlp_version).unwrap_or_default();
    if current != Some(managed.as_path()) {
        let path = download_ytdlp()?;
        let after = engine::ytdlp_version(&path).unwrap_or_default();
        return Ok((before, after));
    }
    let mut cmd = Command::new(&managed);
    cmd.arg("-U");
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    let after = engine::ytdlp_version(&managed).unwrap_or_default();
    if !out.status.success() && after == before {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("yt-dlp -U").to_string());
    }
    Ok((before, after))
}

// ------------------------------------------------------------------ FFmpeg

/// Scarica FFmpeg dalla stessa fonte dell'originale (Tyrrrz/FFmpegBin).
pub fn download_ffmpeg() -> Result<PathBuf, String> {
    let rel = latest_release("Tyrrrz/FFmpegBin")?.ok_or("release di FFmpeg non trovata")?;
    let data = download_asset(&rel, "ffmpeg-windows-x64.zip", None)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let mut exe = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        if f.name().rsplit('/').next() == Some("ffmpeg.exe") {
            f.read_to_end(&mut exe).map_err(|e| e.to_string())?;
            break;
        }
    }
    if exe.is_empty() {
        return Err("ffmpeg.exe non trovato nell'archivio".into());
    }
    let path = engine::app_dir().join("ffmpeg.exe");
    write_atomic(&path, &exe)?;
    Ok(path)
}

// ------------------------------------------------------------------ correzione rapida

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default)]
pub struct Patch {
    /// Cresce a ogni modifica del file su GitHub.
    pub revision: u32,
    /// Spiegazione mostrata all'utente quando arriva una nuova revisione.
    pub message: String,
    /// Opzioni aggiunte a ogni chiamata di yt-dlp.
    pub ytdlp_args: Vec<String>,
    /// Se la copia di yt-dlp è più vecchia di questa versione, va aggiornata.
    pub min_ytdlp: String,
}

pub fn fetch_patch() -> Result<Patch, String> {
    let data = http_get(PATCH_URL)?;
    serde_json::from_slice(&data).map_err(|e| format!("patch.json: {e}"))
}

/// Solo opzioni innocue passano dalla correzione rapida: niente che esegua
/// comandi, scriva file altrove o cambi i percorsi (es. `--exec`, `-o`).
pub fn safe_args(args: &[String]) -> Vec<String> {
    const WITH_VALUE: [&str; 12] = [
        "--extractor-args",
        "--user-agent",
        "--format-sort",
        "--compat-options",
        "--js-runtimes",
        "--remote-components",
        "--extractor-retries",
        "--retries",
        "--fragment-retries",
        "--sleep-requests",
        "--sleep-interval",
        "--concurrent-fragments",
    ];
    const FLAGS: [&str; 3] = ["--force-ipv4", "--force-ipv6", "--no-cache-dir"];
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if WITH_VALUE.contains(&a.as_str()) {
            if let Some(v) = it.next() {
                out.push(a.clone());
                out.push(v.clone());
            }
        } else if FLAGS.contains(&a.as_str()) {
            out.push(a.clone());
        }
    }
    out
}

// ------------------------------------------------------------------ segnalazioni

/// Servizio su Cloudflare che trasforma la segnalazione in una issue GitHub
/// (la chiave GitHub sta lì, non nell'app). Codice in `worker/`.
const REPORT_URL: &str = "https://ytdr-report.gimmy077.workers.dev/report";

/// Invia una segnalazione; restituisce il numero della issue creata.
pub fn send_report(title: &str, body: &str) -> Result<u64, String> {
    let payload = serde_json::json!({ "title": title, "body": body, "version": env!("CARGO_PKG_VERSION") });
    let mut req = ehttp::Request::post(REPORT_URL, payload.to_string().into_bytes());
    req.headers.insert("Content-Type", "application/json");
    req.headers.insert("User-Agent", format!("YTDownloaderRUST/{}", env!("CARGO_PKG_VERSION")));
    let resp = ehttp::fetch_blocking(&req)?;
    let v: Value = serde_json::from_slice(&resp.bytes).unwrap_or(Value::Null);
    if resp.ok {
        return v.get("number").and_then(Value::as_u64).ok_or_else(|| "risposta non valida".to_string());
    }
    Err(match v.get("error").and_then(Value::as_str) {
        Some("rate_limited" | "daily_limit") => "troppe segnalazioni, riprova più tardi / too many reports, try again later".into(),
        Some(e) => format!("{e} (HTTP {})", resp.status),
        None => format!("HTTP {}", resp.status),
    })
}

// ------------------------------------------------------------------ app

pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.trim_start_matches('v').split('.').map(|x| x.parse().unwrap_or(0)).collect() };
    parse(candidate) > parse(current)
}

/// Versione più recente pubblicata, se più nuova di quella in uso.
pub fn check_app_update() -> Result<Option<String>, String> {
    let Some(rel) = latest_release(APP_REPO)? else { return Ok(None) };
    let has_exe = rel.assets.iter().any(|(n, _, _)| n == APP_ASSET);
    Ok((has_exe && is_newer(&rel.tag, env!("CARGO_PKG_VERSION"))).then(|| rel.tag.trim_start_matches('v').to_string()))
}

/// Scarica e installa l'aggiornamento: l'exe in uso viene rinominato in `.old`
/// (Windows lo consente anche mentre gira) e al suo posto va quello nuovo.
pub fn install_app_update() -> Result<(), String> {
    let rel = latest_release(APP_REPO)?.ok_or("nessuna release")?;
    let data = download_asset(&rel, APP_ASSET, Some("SHA256SUMS.txt"))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = exe.with_extension("exe.old");
    let new = exe.with_extension("exe.new");
    std::fs::write(&new, &data).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(e.to_string());
    }
    Ok(())
}

/// All'avvio elimina l'exe sostituito dall'ultimo aggiornamento.
pub fn cleanup_old_exe() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
        let _ = std::fs::remove_file(exe.with_extension("exe.new"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_args() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        let args: Vec<String> = ["--exec", "calc", "--extractor-args", "youtube:player_client=web", "-o", "x", "--force-ipv4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(safe_args(&args), vec!["--extractor-args", "youtube:player_client=web", "--force-ipv4"]);
    }
}
