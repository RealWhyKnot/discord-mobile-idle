use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Stable,
    Beta,
    Dev,
}

pub type Version = ([u64; 4], Kind);

fn kind(suffix: &str) -> Option<Kind> {
    match suffix {
        "" => Some(Kind::Stable),
        "beta" => Some(Kind::Beta),
        "dev" => Some(Kind::Dev),
        _ if suffix.len() == 4 && suffix.bytes().all(|b| b.is_ascii_hexdigit()) => Some(Kind::Dev),
        _ => None,
    }
}

fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

pub fn parse_version(text: &str) -> Option<Version> {
    let mut body = text.trim();
    if body.starts_with(['v', 'V']) {
        body = &body[1..];
    }
    let (head, suffix) = body.split_once('-').unwrap_or((body, ""));
    let kind = kind(&suffix.to_ascii_lowercase())?;
    let parts: Vec<&str> = head.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut numbers = [0u64; 4];
    for (slot, part) in numbers.iter_mut().zip(parts) {
        if !digits(part) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    Some((numbers, kind))
}

fn key(version: &Version) -> ([u64; 4], u8) {
    let rank = match version.1 {
        Kind::Stable => 1,
        _ => 0,
    };
    (version.0, rank)
}

fn visible(channel: &str) -> &'static [Kind] {
    match channel {
        "release" => &[Kind::Stable],
        "beta" => &[Kind::Stable, Kind::Beta],
        _ => &[],
    }
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

pub fn select_release<'a>(releases: &'a [Value], channel: &str, current: &str, skipped: &str) -> Option<&'a Value> {
    let running = parse_version(current)?;
    if running.1 == Kind::Dev {
        return None;
    }
    let visible = visible(channel);
    let mut best = None;
    let mut best_key = key(&running);
    for release in releases {
        if truthy(release.get("draft")) {
            continue;
        }
        let tag = text(release, "tag_name");
        if !skipped.is_empty() && tag == skipped {
            continue;
        }
        let Some(parsed) = parse_version(tag) else {
            continue;
        };
        if !visible.contains(&parsed.1) {
            continue;
        }
        let candidate = key(&parsed);
        if candidate <= best_key {
            continue;
        }
        best = Some(release);
        best_key = candidate;
    }
    best
}

pub fn asset(release: &Value, suffix: &str) -> Option<(String, String)> {
    let items = release.get("assets").and_then(Value::as_array)?;
    items.iter().find_map(|item| {
        let name = text(item, "name");
        name.ends_with(suffix)
            .then(|| (name.to_string(), text(item, "browser_download_url").to_string()))
    })
}

pub fn parse_integrity(text: &str, name: &str) -> Option<(String, u64)> {
    for line in text.split(['\n', '\r']) {
        let row = line.trim();
        if row.is_empty() || row.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = row.split('\t').collect();
        if fields.len() != 3 || fields[2] != name {
            continue;
        }
        let digest = fields[0].trim().to_ascii_lowercase();
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let size = fields[1].trim();
        if !digits(size) {
            return None;
        }
        return Some((digest, size.parse().ok()?));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rel(tag: &str) -> Value {
        json!({"tag_name": tag, "draft": false, "assets": []})
    }

    fn pick<'a>(releases: &'a [Value], channel: &str, current: &str) -> Option<&'a Value> {
        select_release(releases, channel, current, "")
    }

    #[test]
    fn parse_version_accepts() {
        let cases = [
            ("2026.9.7.0", Kind::Stable, [2026, 9, 7, 0]),
            ("v2026.9.7.0", Kind::Stable, [2026, 9, 7, 0]),
            ("V2026.9.7.0", Kind::Stable, [2026, 9, 7, 0]),
            ("  v2026.9.7.0  ", Kind::Stable, [2026, 9, 7, 0]),
            ("v2026.9.7.0-beta", Kind::Beta, [2026, 9, 7, 0]),
            ("v2026.9.7.0-BETA", Kind::Beta, [2026, 9, 7, 0]),
            ("v2026.9.7.0-A528", Kind::Dev, [2026, 9, 7, 0]),
            ("2026.9.7.0-a528", Kind::Dev, [2026, 9, 7, 0]),
            ("0.0.0.0-dev", Kind::Dev, [0, 0, 0, 0]),
        ];
        for (text, kind, numbers) in cases {
            assert_eq!(parse_version(text), Some((numbers, kind)), "{}", text);
        }
    }

    #[test]
    fn parse_version_rejects() {
        for text in [
            "",
            "   ",
            "v2026.9.7",
            "v2026.9.7.0.1",
            "v2026..7.0",
            "v2026.x.7.0",
            "v2026.9.7.0-ZZZZ",
            "v2026.9.7.0-alpha",
            "v2026.9.7.0-beta-1",
            "v2026.9.7.+1",
            "v2026.9.7.99999999999999999999999",
        ] {
            assert_eq!(parse_version(text), None, "{}", text);
        }
    }

    #[test]
    fn a_beta_tag_never_raises_and_is_hidden_from_release() {
        let releases = [rel("v2026.9.8.0-beta")];
        assert_eq!(pick(&releases, "release", "2026.9.7.0"), None);
        assert_eq!(pick(&releases, "beta", "2026.9.7.0"), Some(&releases[0]));
    }

    #[test]
    fn a_hex_stamped_tag_is_never_offered() {
        for channel in ["release", "beta"] {
            assert_eq!(pick(&[rel("v2026.9.8.0-A528")], channel, "2026.9.7.0"), None);
        }
    }

    #[test]
    fn newer_stable_is_offered() {
        let releases = [rel("v2026.9.8.0")];
        assert_eq!(pick(&releases, "release", "2026.9.7.0"), Some(&releases[0]));
    }

    #[test]
    fn same_or_older_is_not_offered() {
        for current in ["2026.9.8.0", "2026.9.9.0"] {
            assert_eq!(pick(&[rel("v2026.9.8.0")], "release", current), None);
        }
    }

    #[test]
    fn an_unusable_running_version_checks_nothing() {
        for current in ["not-a-version", "0.0.0.0-dev", "2026.9.7.0-A528"] {
            assert_eq!(pick(&[rel("v2026.9.8.0")], "release", current), None);
        }
    }

    #[test]
    fn an_unknown_channel_sees_nothing() {
        for channel in ["dev", "", "nonsense"] {
            assert_eq!(pick(&[rel("v2026.9.8.0")], channel, "2026.9.7.0"), None);
        }
    }

    #[test]
    fn drafts_are_skipped() {
        let releases = [json!({"tag_name": "v2026.9.8.0", "draft": true})];
        assert_eq!(pick(&releases, "release", "2026.9.7.0"), None);
    }

    #[test]
    fn the_skipped_tag_is_suppressed() {
        let releases = [rel("v2026.9.8.0")];
        assert_eq!(select_release(&releases, "release", "2026.9.7.0", "v2026.9.8.0"), None);
        assert_eq!(
            select_release(&releases, "release", "2026.9.7.0", "v2026.9.9.0"),
            Some(&releases[0])
        );
    }

    #[test]
    fn a_tie_prefers_the_stable() {
        let stable = rel("v2026.9.8.0");
        let beta = rel("v2026.9.8.0-beta");
        for releases in [[stable.clone(), beta.clone()], [beta.clone(), stable.clone()]] {
            assert_eq!(pick(&releases, "beta", "2026.9.7.0"), Some(&stable));
        }
    }

    #[test]
    fn the_highest_wins_regardless_of_order() {
        let releases = [rel("v2026.9.9.0"), rel("v2026.9.8.0")];
        assert_eq!(pick(&releases, "release", "2026.9.7.0"), Some(&releases[0]));
    }

    #[test]
    fn unparseable_tags_are_ignored() {
        let releases = [rel("nightly"), json!({"draft": false}), rel("v2026.9.8.0")];
        assert_eq!(pick(&releases, "release", "2026.9.7.0"), Some(&releases[2]));
    }

    #[test]
    fn no_releases_at_all() {
        assert_eq!(pick(&[], "release", "2026.9.7.0"), None);
    }

    #[test]
    fn asset_matches_by_suffix() {
        let release = json!({
            "tag_name": "v2026.9.8.0",
            "assets": [
                {"name": "a.integrity.tsv", "browser_download_url": "https://x/tsv"},
                {"name": "a.zip", "browser_download_url": "https://x/zip"},
            ],
        });
        assert_eq!(asset(&release, ".zip"), Some(("a.zip".into(), "https://x/zip".into())));
        assert_eq!(
            asset(&release, ".integrity.tsv"),
            Some(("a.integrity.tsv".into(), "https://x/tsv".into()))
        );
        assert_eq!(asset(&release, ".exe"), None);
    }

    #[test]
    fn asset_tolerates_a_bare_release() {
        assert_eq!(asset(&json!({"tag_name": "v2026.9.8.0"}), ".zip"), None);
        assert_eq!(
            asset(&json!({"assets": [{"name": "a.zip"}]}), ".zip"),
            Some(("a.zip".into(), String::new()))
        );
    }

    #[test]
    fn parse_integrity_reads_the_matching_row() {
        let text = format!(
            "# comment\n\n{}\t123\tother.zip\n{}\t456\ta.zip\n",
            "a".repeat(64),
            "B".repeat(64)
        );
        assert_eq!(parse_integrity(&text, "a.zip"), Some(("b".repeat(64), 456)));
    }

    #[test]
    fn parse_integrity_tolerates_crlf() {
        let text = format!("{}\t7\ta.zip\r\n", "a".repeat(64));
        assert_eq!(parse_integrity(&text, "a.zip"), Some(("a".repeat(64), 7)));
    }

    #[test]
    fn parse_integrity_finds_nothing() {
        let a = "a".repeat(64);
        for text in [
            String::new(),
            format!("{}\t9\tother.zip\n", a),
            format!("{}\t9\n", a),
            format!("{}\t9\ta.zip\textra\n", a),
        ] {
            assert_eq!(parse_integrity(&text, "a.zip"), None, "{:?}", text);
        }
    }

    #[test]
    fn parse_integrity_rejects_a_bad_row() {
        for (digest, size) in [
            ("a".repeat(63), "9"),
            ("z".repeat(64), "9"),
            ("a".repeat(64), ""),
            ("a".repeat(64), "9x"),
        ] {
            let text = format!("{}\t{}\ta.zip\n", digest, size);
            assert_eq!(parse_integrity(&text, "a.zip"), None, "{:?}", text);
        }
    }
}
