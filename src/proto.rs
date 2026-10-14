use serde_json::{json, Value};

pub const STATUS_SETTINGS: u32 = 11;
const STATUS_VALUE: u32 = 1;
const WRAPPED_VALUE: u32 = 1;
const CUSTOM_STATUS: u32 = 2;
const CUSTOM_TEXT: u32 = 1;
const CUSTOM_EMOJI_ID: u32 = 2;
const CUSTOM_EMOJI_NAME: u32 = 3;
const FIXED64: u8 = 1;
const LEN: u8 = 2;

struct Field<'a> {
    number: u32,
    wire: u8,
    raw: &'a [u8],
    payload: &'a [u8],
}

fn varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn fields(buf: &[u8]) -> Option<Vec<Field<'_>>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let start = pos;
        let key = varint(buf, &mut pos)?;
        let number = u32::try_from(key >> 3).ok()?;
        let wire = (key & 7) as u8;
        let body = pos;
        match wire {
            0 => {
                varint(buf, &mut pos)?;
            }
            1 => pos = pos.checked_add(8)?,
            2 => {
                let len = usize::try_from(varint(buf, &mut pos)?).ok()?;
                let body = pos;
                pos = pos.checked_add(len)?;
                if pos > buf.len() {
                    return None;
                }
                out.push(Field {
                    number,
                    wire,
                    raw: &buf[start..pos],
                    payload: &buf[body..pos],
                });
                continue;
            }
            5 => pos = pos.checked_add(4)?,
            _ => return None,
        }
        if pos > buf.len() {
            return None;
        }
        out.push(Field {
            number,
            wire,
            raw: &buf[start..pos],
            payload: &buf[body..pos],
        });
    }
    Some(out)
}

fn merged(buf: &[u8], number: u32) -> Option<Vec<u8>> {
    let mut found = false;
    let mut out = Vec::new();
    for field in fields(buf)? {
        if field.number == number && field.wire == LEN {
            found = true;
            out.extend_from_slice(field.payload);
        }
    }
    found.then_some(out)
}

fn last_string(buf: &[u8], number: u32) -> Option<String> {
    let field = fields(buf)?
        .into_iter()
        .rfind(|f| f.number == number && f.wire == LEN)?;
    String::from_utf8(field.payload.to_vec()).ok()
}

fn last_fixed64(buf: &[u8], number: u32) -> u64 {
    fields(buf)
        .and_then(|fields| fields.into_iter().rfind(|f| f.number == number && f.wire == FIXED64))
        .and_then(|field| <[u8; 8]>::try_from(field.payload).ok())
        .map(u64::from_le_bytes)
        .unwrap_or(0)
}

pub fn custom_activity(settings: &[u8]) -> Option<Value> {
    let custom = merged(settings, STATUS_SETTINGS).and_then(|s| merged(&s, CUSTOM_STATUS))?;
    let text = last_string(&custom, CUSTOM_TEXT).unwrap_or_default();
    let emoji_id = last_fixed64(&custom, CUSTOM_EMOJI_ID);
    let emoji_name = last_string(&custom, CUSTOM_EMOJI_NAME).unwrap_or_default();
    let mut activity = json!({
        "type": 4,
        "name": "Custom Status",
        "state": if text.is_empty() { Value::Null } else { Value::from(text) },
    });
    if emoji_id != 0 {
        activity["emoji"] = json!({"id": emoji_id, "name": emoji_name});
    } else if !emoji_name.is_empty() {
        activity["emoji"] = json!({"id": null, "name": emoji_name});
    }
    Some(activity)
}

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn push_bytes(out: &mut Vec<u8>, number: u32, payload: &[u8]) {
    push_varint(out, (u64::from(number) << 3) | u64::from(LEN));
    push_varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

pub fn status(settings: &[u8]) -> String {
    let value = merged(settings, STATUS_SETTINGS)
        .and_then(|s| merged(&s, STATUS_VALUE))
        .and_then(|w| last_string(&w, WRAPPED_VALUE))
        .unwrap_or_default();
    if value.is_empty() {
        "unknown".into()
    } else {
        value
    }
}

pub fn status_patch(settings: &[u8], status: &str) -> Vec<u8> {
    let existing = merged(settings, STATUS_SETTINGS).unwrap_or_default();
    let mut inner = Vec::new();
    for field in fields(&existing).unwrap_or_default() {
        if field.number != STATUS_VALUE {
            inner.extend_from_slice(field.raw);
        }
    }
    let mut wrapped = Vec::new();
    push_bytes(&mut wrapped, WRAPPED_VALUE, status.as_bytes());
    push_bytes(&mut inner, STATUS_VALUE, &wrapped);
    let mut out = Vec::new();
    push_bytes(&mut out, STATUS_SETTINGS, &inner);
    out
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        if c == b'=' {
            break;
        }
        let v = ALPHABET.iter().position(|&a| a == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_activity_reads_the_settings_custom_status() {
        let mut custom = Vec::new();
        push_bytes(&mut custom, CUSTOM_TEXT, b"busy");
        push_varint(&mut custom, (u64::from(CUSTOM_EMOJI_ID) << 3) | u64::from(FIXED64));
        custom.extend_from_slice(&42u64.to_le_bytes());
        push_bytes(&mut custom, CUSTOM_EMOJI_NAME, b"cat");
        let mut status = Vec::new();
        push_bytes(&mut status, CUSTOM_STATUS, &custom);
        let mut settings = Vec::new();
        push_bytes(&mut settings, STATUS_SETTINGS, &status);
        assert_eq!(
            custom_activity(&settings),
            Some(json!({"type": 4, "name": "Custom Status", "state": "busy", "emoji": {"id": 42, "name": "cat"}}))
        );

        let mut named = Vec::new();
        push_bytes(&mut named, CUSTOM_EMOJI_NAME, b"wave");
        let mut status = Vec::new();
        push_bytes(&mut status, CUSTOM_STATUS, &named);
        let mut settings = Vec::new();
        push_bytes(&mut settings, STATUS_SETTINGS, &status);
        assert_eq!(
            custom_activity(&settings),
            Some(json!({"type": 4, "name": "Custom Status", "state": null, "emoji": {"id": null, "name": "wave"}}))
        );

        let mut status = Vec::new();
        push_bytes(&mut status, STATUS_VALUE, &wrapped("online"));
        let mut settings = Vec::new();
        push_bytes(&mut settings, STATUS_SETTINGS, &status);
        assert_eq!(custom_activity(&settings), None);
        assert_eq!(custom_activity(&[]), None);
    }

    fn wrapped(status: &str) -> Vec<u8> {
        let mut w = Vec::new();
        push_bytes(&mut w, 1, status.as_bytes());
        w
    }

    fn settings_with(status: &str, extra_status_fields: &[u8], extra_top: &[u8]) -> Vec<u8> {
        let mut inner = Vec::new();
        push_bytes(&mut inner, 1, &wrapped(status));
        inner.extend_from_slice(extra_status_fields);
        let mut out = extra_top.to_vec();
        push_bytes(&mut out, 11, &inner);
        out
    }

    #[test]
    fn base64_round_trips() {
        for sample in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            &[0, 255, 128, 7],
        ] {
            let encoded = b64encode(sample);
            assert_eq!(b64decode(&encoded).unwrap(), sample);
        }
        assert_eq!(b64encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64encode(b"fo"), "Zm8=");
        assert_eq!(b64decode("Zm8").unwrap(), b"fo");
        assert!(b64decode("Zm8*").is_none());
    }

    #[test]
    fn reads_the_saved_status() {
        let mut top = Vec::new();
        push_bytes(&mut top, 2, b"other section");
        assert_eq!(status(&settings_with("dnd", &[], &top)), "dnd");
    }

    #[test]
    fn missing_or_empty_status_is_unknown() {
        assert_eq!(status(&[]), "unknown");
        assert_eq!(status(&settings_with("", &[], &[])), "unknown");
        let mut only_other = Vec::new();
        push_bytes(&mut only_other, 2, b"x");
        assert_eq!(status(&only_other), "unknown");
    }

    #[test]
    fn garbage_reads_as_unknown() {
        assert_eq!(status(&[0x5a, 0xff, 0xff]), "unknown");
        assert_eq!(status(&[0x5a, 0x10, 0x01]), "unknown");
    }

    #[test]
    fn a_partial_update_appended_to_the_settings_merges() {
        let mut both = settings_with("online", &[], &[]);
        both.extend(settings_with("idle", &[], &[]));
        assert_eq!(status(&both), "idle");
        let mut custom = Vec::new();
        push_bytes(&mut custom, 2, b"custom");
        let mut partial = Vec::new();
        push_bytes(&mut partial, 11, &custom);
        let mut kept = settings_with("dnd", &[], &[]);
        kept.extend(partial);
        assert_eq!(status(&kept), "dnd");
    }

    #[test]
    fn the_patch_keeps_every_other_status_field() {
        let mut custom = Vec::new();
        push_bytes(&mut custom, 2, b"custom status bytes");
        let mut show_game = Vec::new();
        push_bytes(&mut show_game, 3, &[0x08, 0x01]);
        let mut extras = custom.clone();
        extras.extend_from_slice(&show_game);
        let mut top = Vec::new();
        push_bytes(&mut top, 2, b"privacy");
        let settings = settings_with("online", &extras, &top);

        let patch = status_patch(&settings, "idle");

        assert_eq!(status(&patch), "idle");
        let inner = merged(&patch, 11).unwrap();
        let numbers: Vec<u32> = fields(&inner).unwrap().iter().map(|f| f.number).collect();
        assert_eq!(numbers, vec![2, 3, 1]);
        assert!(inner.windows(custom.len()).any(|w| w == custom.as_slice()));
        assert!(merged(&patch, 2).is_none());
    }

    #[test]
    fn the_patch_works_without_existing_settings() {
        let patch = status_patch(&[], "online");
        assert_eq!(status(&patch), "online");
        assert_eq!(
            patch,
            vec![0x5a, 0x0a, 0x0a, 0x08, 0x0a, 0x06, b'o', b'n', b'l', b'i', b'n', b'e']
        );
    }

    #[test]
    fn varints_round_trip() {
        for value in [0u64, 1, 127, 128, 300, 16_384, u32::MAX as u64, u64::MAX] {
            let mut buf = Vec::new();
            push_varint(&mut buf, value);
            let mut pos = 0;
            assert_eq!(varint(&buf, &mut pos), Some(value));
            assert_eq!(pos, buf.len());
        }
    }
}
