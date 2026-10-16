use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;

use serde_json::Value;
use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS, SYSTEMTIME};
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

use super::wide;
use crate::error;
use crate::log::{self, Rotating};

pub const APP: &str = "discord-mobile-idle";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub const DEFAULT_SETTINGS: [(&str, &str); 11] = [
    ("POLL_SECONDS", "10"),
    ("RESTORE_STATUS", "online"),
    ("MANAGED_STATUSES", "online"),
    ("ON_DEBOUNCE_POLLS", "2"),
    ("OFF_DEBOUNCE_POLLS", "4"),
    ("WATCHDOG_SECONDS", "120"),
    ("GAME_DND", "true"),
    ("GAME_IGNORE", ""),
    ("UPDATE_CHANNEL", "release"),
    ("CHECK_FOR_UPDATES", "true"),
    ("SKIPPED_TAG", ""),
];

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let path = base.join(APP);
    let _ = std::fs::create_dir_all(&path);
    path
}

pub fn path_for(name: &str) -> PathBuf {
    data_dir().join(name)
}

fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

pub fn parse_settings(text: &str) -> HashMap<String, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => map.iter().map(|(k, v)| (k.clone(), py_str(v))).collect(),
        _ => HashMap::new(),
    }
}

pub fn read_settings() -> HashMap<String, String> {
    match std::fs::read_to_string(path_for("settings.json")) {
        Ok(text) => parse_settings(&text),
        Err(_) => HashMap::new(),
    }
}

pub fn default_settings_text() -> String {
    let rows: Vec<String> = DEFAULT_SETTINGS
        .iter()
        .map(|(k, v)| format!("  {}: {}", Value::from(*k), Value::from(*v)))
        .collect();
    format!("{{\r\n{}\r\n}}", rows.join(",\r\n"))
}

pub fn settings_file() -> PathBuf {
    let path = path_for("settings.json");
    if !path.exists() {
        let _ = std::fs::write(&path, default_settings_text());
    }
    path
}

fn crypt(protect: bool, data: &[u8]) -> io::Result<Vec<u8>> {
    let incoming = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut outgoing = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &incoming,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut outgoing,
            )
        } else {
            CryptUnprotectData(
                &incoming,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut outgoing,
            )
        }
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let out = unsafe { std::slice::from_raw_parts(outgoing.pbData, outgoing.cbData as usize).to_vec() };
    unsafe { LocalFree(outgoing.pbData as _) };
    Ok(out)
}

pub fn check_crypto() -> bool {
    crypt(true, b"ok")
        .and_then(|blob| crypt(false, &blob))
        .is_ok_and(|plain| plain == b"ok")
}

pub fn read_token() -> String {
    let Ok(blob) = std::fs::read(path_for("token.bin")) else {
        return String::new();
    };
    match crypt(false, &blob).map(String::from_utf8) {
        Ok(Ok(token)) => token,
        Ok(Err(e)) => {
            error!("stored token could not be decrypted: {}", e);
            String::new()
        }
        Err(e) => {
            error!("stored token could not be decrypted: {}", e);
            String::new()
        }
    }
}

pub fn write_token(token: &str) -> io::Result<()> {
    let blob = crypt(true, token.as_bytes())?;
    std::fs::write(path_for("token.bin"), blob)
}

pub fn single_instance() -> Option<File> {
    let file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path_for("instance.lock"))
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

pub fn local_stamp() -> String {
    let mut now = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    unsafe { GetLocalTime(&mut now) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02},{:03}",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds
    )
}

pub fn start_logging() {
    log::to_file(Rotating::new(path_for("idlebot.log"), 524_288, 3), local_stamp);
    std::panic::set_hook(Box::new(|panic| error!("{}", panic)));
}

pub fn autostart_command() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!("\"{}\"", exe.display())
}

fn run_value() -> Option<String> {
    let key = wide(RUN_KEY);
    let name = wide(APP);
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr() as _,
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buffer[..len]))
}

pub fn autostart_enabled() -> bool {
    run_value().is_some_and(|value| value == autostart_command())
}

pub fn set_autostart(enabled: bool) -> io::Result<()> {
    let key = wide(RUN_KEY);
    let name = wide(APP);
    let status = unsafe {
        if enabled {
            let data = wide(&autostart_command());
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr() as _,
                (data.len() * 2) as u32,
            )
        } else {
            match RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) {
                ERROR_SUCCESS => ERROR_SUCCESS,
                _ => return Ok(()),
            }
        }
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_match_the_python_dump() {
        let expected = "{\r\n  \"POLL_SECONDS\": \"10\",\r\n  \"RESTORE_STATUS\": \"online\",\r\n  \"MANAGED_STATUSES\": \"online\",\r\n  \"ON_DEBOUNCE_POLLS\": \"2\",\r\n  \"OFF_DEBOUNCE_POLLS\": \"4\",\r\n  \"WATCHDOG_SECONDS\": \"120\",\r\n  \"GAME_DND\": \"true\",\r\n  \"GAME_IGNORE\": \"\",\r\n  \"UPDATE_CHANNEL\": \"release\",\r\n  \"CHECK_FOR_UPDATES\": \"true\",\r\n  \"SKIPPED_TAG\": \"\"\r\n}";
        assert_eq!(default_settings_text(), expected);
        let parsed = parse_settings(&default_settings_text());
        assert_eq!(parsed.len(), 11);
        assert_eq!(parsed["ON_DEBOUNCE_POLLS"], "2");
    }

    #[test]
    fn settings_values_become_python_strings() {
        let parsed =
            parse_settings("\u{feff}{\"A\": 10, \"B\": true, \"C\": false, \"D\": null, \"E\": \"x\", \"F\": 1.5}");
        assert_eq!(parsed["A"], "10");
        assert_eq!(parsed["B"], "True");
        assert_eq!(parsed["C"], "False");
        assert_eq!(parsed["D"], "None");
        assert_eq!(parsed["E"], "x");
        assert_eq!(parsed["F"], "1.5");
    }

    #[test]
    fn unreadable_settings_are_empty() {
        assert!(parse_settings("").is_empty());
        assert!(parse_settings("[1, 2]").is_empty());
        assert!(parse_settings("{broken").is_empty());
    }

    #[test]
    fn dpapi_round_trips() {
        assert!(check_crypto());
        let blob = crypt(true, "tok\u{e9}n".as_bytes()).unwrap();
        assert_ne!(blob, "tok\u{e9}n".as_bytes());
        assert_eq!(crypt(false, &blob).unwrap(), "tok\u{e9}n".as_bytes());
        assert!(crypt(false, b"not a blob").is_err());
    }

    #[test]
    fn local_stamp_has_the_asctime_shape() {
        let stamp = local_stamp();
        assert_eq!(stamp.len(), 23);
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], " ");
        assert_eq!(&stamp[19..20], ",");
    }
}
