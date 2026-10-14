use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use windows_sys::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};

use super::{store, zip};
use crate::info;
use crate::release::{asset, parse_integrity, parse_version, select_release, Kind};

pub const VERSION: &str = match option_env!("IDLEBOT_VERSION") {
    Some(version) => version,
    None => "0.0.0.0-dev",
};

const API: &str = "https://api.github.com/repos/RealWhyKnot/discord-mobile-idle/releases?per_page=20";
pub const EXE: &str = "discord-mobile-idle.exe";
const TIMEOUT: Duration = Duration::from_secs(15);
const DETACHED_PROCESS: u32 = 0x0000_0008;

const SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
$log = %(log)s
$target = %(target)s
$source = %(source)s
$lock = %(lock)s
$backup = "${target}.bak"
function Note($text) { "$(Get-Date -Format s) $text" | Add-Content -LiteralPath $log -Encoding UTF8 }
Wait-Process -Id %(pid)d -Timeout 60 -ErrorAction SilentlyContinue
$free = $false
foreach ($attempt in 1..30) {
    try {
        foreach ($path in @($target, $lock)) { [System.IO.File]::Open($path, 'Open', 'Write', 'None').Dispose() }
        $free = $true
        break
    } catch {
        Start-Sleep -Seconds 2
    }
}
if (-not $free) {
    Note "gave up waiting for $target"
    exit 1
}
$moved = $false
try {
    Move-Item -LiteralPath $target -Destination $backup -Force
    $moved = $true
} catch {
    Note "could not set aside $target - $_"
}
if ($moved) {
    try {
        Copy-Item -LiteralPath $source -Destination $target -Force
        Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
        Note "installed $source over $target"
    } catch {
        Note "install failed, rolled back: $_"
        Move-Item -LiteralPath $backup -Destination $target -Force
    }
}
Start-Process -FilePath $target
"#;

pub fn checkable() -> bool {
    parse_version(VERSION).is_some_and(|(_, kind)| kind != Kind::Dev)
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn staging() -> PathBuf {
    let path = store::path_for("update");
    let _ = std::fs::create_dir_all(&path);
    path
}

async fn fetch(client: &wreq::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .header("User-Agent", format!("discord-mobile-idle/{}", VERSION))
        .read_timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {} from {}", status.as_u16(), url));
    }
    Ok(response.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

fn text(payload: Vec<u8>) -> Result<String, String> {
    let body = payload.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&payload);
    String::from_utf8(body.to_vec()).map_err(|e| e.to_string())
}

pub struct Updater {
    client: wreq::Client,
    runtime: tokio::runtime::Runtime,
}

impl Updater {
    pub fn new() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let client = wreq::Client::builder()
            .redirect(wreq::redirect::Policy::limited(10))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Updater { client, runtime })
    }

    pub fn check(&self, channel: &str, skipped: &str) -> Result<Option<Value>, String> {
        let payload = self.runtime.block_on(fetch(&self.client, API))?;
        let releases: Vec<Value> = serde_json::from_str(&text(payload)?).map_err(|e| e.to_string())?;
        Ok(select_release(&releases, channel, VERSION, skipped).cloned())
    }

    pub fn stage(&self, chosen: &Value) -> Result<PathBuf, String> {
        let tag = chosen.get("tag_name").and_then(Value::as_str).unwrap_or("");
        let (Some((zip_name, zip_url)), Some((_, tsv_url))) = (asset(chosen, ".zip"), asset(chosen, ".integrity.tsv"))
        else {
            return Err(format!("release {} is missing an asset", tag));
        };
        let rows = text(self.runtime.block_on(fetch(&self.client, &tsv_url))?)?;
        let (digest, size) = parse_integrity(&rows, &zip_name).ok_or(format!("no integrity row for {}", zip_name))?;

        let staging = staging();
        let archive = staging.join(&zip_name);
        let payload = match self.runtime.block_on(fetch(&self.client, &zip_url)) {
            Ok(payload) => payload,
            Err(_) => {
                std::thread::sleep(Duration::from_secs(5));
                self.runtime.block_on(fetch(&self.client, &zip_url))?
            }
        };
        std::fs::write(&archive, &payload).map_err(|e| e.to_string())?;

        let actual = std::fs::metadata(&archive).map_err(|e| e.to_string())?.len();
        if actual != size {
            return Err(format!("{} is {} bytes, expected {}", zip_name, actual, size));
        }
        let data = std::fs::read(&archive).map_err(|e| e.to_string())?;
        if sha256_hex(&data).as_deref() != Some(digest.as_str()) {
            return Err(format!("{} failed its sha256 check", zip_name));
        }
        let exe = zip::extract(&data, EXE)?;
        let staged = staging.join(EXE);
        std::fs::write(&staged, exe).map_err(|e| e.to_string())?;
        Ok(staged)
    }
}

pub fn sha256_hex(data: &[u8]) -> Option<String> {
    let mut out = [0u8; 32];
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            std::ptr::null(),
            0,
            data.as_ptr(),
            data.len() as u32,
            out.as_mut_ptr(),
            32,
        )
    };
    (status == 0).then(|| out.iter().map(|b| format!("{:02x}", b)).collect())
}

pub fn render(target: &Path, source: &Path, pid: u32) -> String {
    SCRIPT
        .replace("%(log)s", &quote(&store::path_for("update.log").to_string_lossy()))
        .replace("%(target)s", &quote(&target.to_string_lossy()))
        .replace("%(source)s", &quote(&source.to_string_lossy()))
        .replace("%(lock)s", &quote(&store::path_for("instance.lock").to_string_lossy()))
        .replace("%(pid)d", &pid.to_string())
}

pub fn launch(staged: &Path, target: &Path, pid: u32) -> std::io::Result<()> {
    let script = staging().join("update.ps1");
    let body = format!("\u{feff}{}", render(target, staged, pid).replace('\n', "\r\n"));
    std::fs::write(&script, body)?;
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(&script)
        .creation_flags(DETACHED_PROCESS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    info!("update helper launched for {}", target.display());
    Ok(())
}

pub fn clean() {
    let _ = std::fs::remove_dir_all(store::path_for("update"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_digests() {
        assert_eq!(
            sha256_hex(b"").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn quote_doubles_single_quotes() {
        assert_eq!(quote(r"C:\Users\O'Neil\a.exe"), r"'C:\Users\O''Neil\a.exe'");
    }

    #[test]
    fn render_fills_every_slot() {
        let body = render(Path::new(r"C:\a b\x.exe"), Path::new(r"C:\it's\y.exe"), 4242);
        assert!(!body.contains("%("));
        assert!(body.contains("$target = 'C:\\a b\\x.exe'\n"));
        assert!(body.contains("$source = 'C:\\it''s\\y.exe'\n"));
        assert!(body.contains("Wait-Process -Id 4242 -Timeout 60"));
        assert!(body.contains("$backup = \"${target}.bak\""));
        assert!(body.ends_with("Start-Process -FilePath $target\n"));
    }
}
