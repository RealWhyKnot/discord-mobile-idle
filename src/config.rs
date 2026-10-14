use std::collections::BTreeSet;

use crate::controller::{ADOPTABLE, ALL_STATUSES};

#[derive(Debug)]
pub struct Config {
    pub token: String,
    pub poll_seconds: u64,
    pub restore: String,
    pub managed: BTreeSet<String>,
    pub on_polls: u32,
    pub off_polls: u32,
    pub watchdog_seconds: u64,
}

pub fn py_list<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let mut sorted: Vec<&str> = items.into_iter().collect();
    sorted.sort_unstable();
    let quoted: Vec<String> = sorted.iter().map(|s| format!("'{}'", s)).collect();
    format!("[{}]", quoted.join(", "))
}

fn int(env: &dyn Fn(&str) -> Option<String>, name: &str, default: u64) -> Result<u64, String> {
    let raw = env(name).unwrap_or_default();
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(default);
    }
    let value: i64 = raw
        .parse()
        .map_err(|_| format!("{} must be an integer, got '{}'", name, raw))?;
    if value < 1 {
        return Err(format!("{} must be >= 1, got {}", name, value));
    }
    Ok(value as u64)
}

pub fn load_config(env: &dyn Fn(&str) -> Option<String>) -> Result<Config, String> {
    let token = env("DISCORD_TOKEN").unwrap_or_default().trim().to_string();
    if token.is_empty() {
        return Err("DISCORD_TOKEN is required".into());
    }

    let restore = env("RESTORE_STATUS")
        .unwrap_or_else(|| "online".into())
        .trim()
        .to_lowercase();
    if !ADOPTABLE.contains(&restore.as_str()) {
        return Err(format!(
            "RESTORE_STATUS must be one of {}, got '{}'",
            py_list(ADOPTABLE),
            restore
        ));
    }

    let raw = env("MANAGED_STATUSES").unwrap_or_else(|| "online".into());
    let managed: BTreeSet<String> = raw
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if managed.is_empty() {
        return Err("MANAGED_STATUSES must name at least one status".into());
    }
    let unknown: Vec<&str> = managed
        .iter()
        .map(String::as_str)
        .filter(|s| !ALL_STATUSES.contains(s))
        .collect();
    if !unknown.is_empty() {
        return Err(format!("MANAGED_STATUSES has unknown values: {}", py_list(unknown)));
    }
    let unmanageable: Vec<&str> = managed
        .iter()
        .map(String::as_str)
        .filter(|s| !ADOPTABLE.contains(s))
        .collect();
    if !unmanageable.is_empty() {
        return Err(format!(
            "MANAGED_STATUSES may only contain {}, got {}",
            py_list(ADOPTABLE),
            py_list(unmanageable)
        ));
    }

    let poll_seconds = int(env, "POLL_SECONDS", 10)?;
    let watchdog_seconds = int(env, "WATCHDOG_SECONDS", 120)?;
    if watchdog_seconds <= poll_seconds {
        return Err(format!(
            "WATCHDOG_SECONDS ({}) must exceed POLL_SECONDS ({})",
            watchdog_seconds, poll_seconds
        ));
    }

    Ok(Config {
        token,
        poll_seconds,
        restore,
        managed,
        on_polls: int(env, "ON_DEBOUNCE_POLLS", 1)? as u32,
        off_polls: int(env, "OFF_DEBOUNCE_POLLS", 4)? as u32,
        watchdog_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn load(pairs: &[(&str, &str)]) -> Result<Config, String> {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("DISCORD_TOKEN".into(), "token".into());
        for (k, v) in pairs {
            map.insert(k.to_string(), v.to_string());
        }
        load_config(&move |name: &str| map.get(name).cloned())
    }

    fn load_raw(pairs: &[(&str, &str)]) -> Result<Config, String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        load_config(&move |name: &str| map.get(name).cloned())
    }

    #[test]
    fn defaults() {
        let c = load(&[]).unwrap();
        assert_eq!(c.token, "token");
        assert_eq!(c.poll_seconds, 10);
        assert_eq!(c.restore, "online");
        assert_eq!(c.managed, ["online".to_string()].into_iter().collect());
        assert_eq!(c.on_polls, 1);
        assert_eq!(c.off_polls, 4);
        assert_eq!(c.watchdog_seconds, 120);
    }

    #[test]
    fn token_is_required() {
        assert!(load_raw(&[]).is_err());
        assert!(load_raw(&[("DISCORD_TOKEN", "   ")]).is_err());
    }

    #[test]
    fn restore_status_must_be_adoptable() {
        for value in ["idle", "invisible", "offline", "bogus"] {
            assert!(load(&[("RESTORE_STATUS", value)]).is_err(), "{}", value);
        }
        assert_eq!(
            load(&[("RESTORE_STATUS", "idle")]).unwrap_err(),
            "RESTORE_STATUS must be one of ['dnd', 'online'], got 'idle'"
        );
    }

    #[test]
    fn restore_status_accepts_dnd() {
        assert_eq!(load(&[("RESTORE_STATUS", "DND")]).unwrap().restore, "dnd");
    }

    #[test]
    fn managed_rejects_unknown_and_unmanageable() {
        assert!(load(&[("MANAGED_STATUSES", "online,bogus")]).is_err());
        for value in ["invisible", "offline", "idle"] {
            assert!(load(&[("MANAGED_STATUSES", &format!("online,{}", value))]).is_err());
        }
        assert!(load(&[("MANAGED_STATUSES", " , ")]).is_err());
    }

    #[test]
    fn managed_accepts_dnd() {
        let c = load(&[("MANAGED_STATUSES", "online, dnd")]).unwrap();
        assert_eq!(
            c.managed,
            ["dnd".to_string(), "online".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn integers_are_checked() {
        assert!(load(&[("POLL_SECONDS", "ten")]).is_err());
        assert!(load(&[("POLL_SECONDS", "0")]).is_err());
        assert_eq!(load(&[("POLL_SECONDS", "  ")]).unwrap().poll_seconds, 10);
        assert!(load(&[("POLL_SECONDS", "120"), ("WATCHDOG_SECONDS", "120")]).is_err());
    }

    #[test]
    fn debounce_polls_are_configurable() {
        let c = load(&[("ON_DEBOUNCE_POLLS", "5"), ("OFF_DEBOUNCE_POLLS", "9")]).unwrap();
        assert_eq!(c.on_polls, 5);
        assert_eq!(c.off_polls, 9);
    }
}
