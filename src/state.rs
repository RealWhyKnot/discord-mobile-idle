use serde::Deserialize;
use serde_json::Value;

use crate::controller::{INVISIBLE, OFFLINE};
use crate::proto;
use crate::runner::UNKNOWN;
use crate::warning;

pub const ALL: &str = "all";

#[derive(Deserialize, Default)]
pub struct ClientInfo {
    #[serde(default)]
    pub client: String,
}

#[derive(Deserialize)]
pub struct SessionPayload {
    pub session_id: String,
    #[serde(default)]
    pub client_info: ClientInfo,
    pub status: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub activities: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub session_id: String,
    pub client: String,
    pub status: String,
    pub active: bool,
    pub activities: Vec<Value>,
}

impl Session {
    fn from_payload(p: &SessionPayload) -> Self {
        Session {
            session_id: p.session_id.clone(),
            client: p.client_info.client.clone(),
            status: p.status.clone(),
            active: p.active,
            activities: p.activities.clone(),
        }
    }

    fn update(&mut self, p: &SessionPayload) -> bool {
        let changed = self.status != p.status || self.active != p.active || self.activities != p.activities;
        self.status = p.status.clone();
        self.active = p.active;
        self.activities = p.activities.clone();
        changed
    }
}

#[derive(Default)]
pub struct State {
    pub session_id: Option<String>,
    pub sessions: Vec<Session>,
    pub settings: Option<Vec<u8>>,
    pub ws_status: String,
    pub ws_activities: Vec<Value>,
    pub ignored: Vec<(String, u64)>,
}

impl State {
    pub fn new() -> Self {
        State {
            ws_status: UNKNOWN.into(),
            ..State::default()
        }
    }

    fn find(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.session_id == id)
    }

    pub fn current_session(&self) -> Option<&Session> {
        self.session_id.as_deref().and_then(|id| self.find(id))
    }

    pub fn ready(&mut self, session_id: String, sessions: Option<Vec<SessionPayload>>, settings: Option<&str>) {
        self.sessions.clear();
        self.session_id = Some(session_id);
        self.settings = Some(settings.and_then(proto::b64decode).unwrap_or_default());
        if let Some(sessions) = sessions {
            self.sessions_replace(&sessions, true);
        }
    }

    pub fn sessions_replace(&mut self, payload: &[SessionPayload], from_ready: bool) -> bool {
        if payload.is_empty() {
            warning!(
                "User has no sessions (from READY: {}). Discarding.",
                if from_ready { "True" } else { "False" }
            );
            return false;
        }
        let mut changed = false;
        for p in payload {
            match self.sessions.iter_mut().find(|s| s.session_id == p.session_id) {
                Some(existing) => changed |= existing.update(p) && !from_ready,
                None => {
                    self.sessions.push(Session::from_payload(p));
                    changed |= !from_ready;
                }
            }
        }
        let mut old_all = None;
        if !from_ready {
            let present = |id: &str| payload.iter().any(|p| p.session_id == id);
            let mut kept = Vec::with_capacity(self.sessions.len());
            for session in self.sessions.drain(..) {
                if present(&session.session_id) {
                    kept.push(session);
                } else if session.session_id == ALL {
                    old_all = Some(session);
                } else {
                    changed = true;
                }
            }
            self.sessions = kept;
        }
        if self.find(ALL).is_none() {
            let ours = self.session_id.as_deref();
            let fake = payload
                .iter()
                .find(|p| Some(p.session_id.as_str()) == ours)
                .unwrap_or(&payload[0]);
            let all = match old_all {
                Some(mut old) => {
                    changed |= old.update(fake);
                    old
                }
                None => {
                    let mut all = Session::from_payload(fake);
                    all.session_id = ALL.into();
                    all.client = UNKNOWN.into();
                    all
                }
            };
            self.sessions.push(all);
        }
        if from_ready && self.ws_status == UNKNOWN {
            if let Some(ours) = self.current_session() {
                let (status, activities) = (ours.status.clone(), ours.activities.clone());
                self.ws_status = status;
                self.ws_activities = activities;
            }
        }
        changed
    }

    pub fn settings_update(&mut self, kind: u64, encoded: &str, partial: bool) -> Option<(String, String)> {
        if kind != 1 {
            if kind > 3 {
                warning!("USER_SETTINGS_PROTO_UPDATE referencing an unknown type: {}", kind);
            }
            return None;
        }
        let settings = self.settings.as_mut()?;
        let before = proto::status(settings);
        let data = proto::b64decode(encoded).unwrap_or_default();
        if partial {
            settings.extend_from_slice(&data);
        } else {
            *settings = data;
        }
        Some((proto::status(settings), before))
    }

    pub fn is_on_mobile(&self) -> bool {
        self.sessions.iter().any(|s| s.client == "mobile")
    }

    pub fn other_sessions(&self) -> usize {
        let ours = self.session_id.as_deref();
        self.sessions
            .iter()
            .filter(|s| s.session_id != ALL && Some(s.session_id.as_str()) != ours)
            .count()
    }

    pub fn client_status(&self) -> String {
        if let Some(ours) = self.current_session() {
            return ours.status.clone();
        }
        if self.ws_status != UNKNOWN {
            return self.ws_status.clone();
        }
        match &self.settings {
            Some(settings) => proto::status(settings),
            None => OFFLINE.into(),
        }
    }

    pub fn status(&self) -> String {
        match self.find(ALL) {
            Some(all) => all.status.clone(),
            None => self.client_status(),
        }
    }

    pub fn own_status(&self) -> Option<String> {
        self.current_session().map(|s| s.status.clone())
    }

    pub fn saved_status(&self) -> String {
        match &self.settings {
            Some(settings) => proto::status(settings),
            None => UNKNOWN.into(),
        }
    }

    pub fn client_activities(&self) -> Vec<Value> {
        match self.current_session() {
            Some(ours) => ours.activities.clone(),
            None => self
                .settings
                .as_deref()
                .and_then(proto::custom_activity)
                .into_iter()
                .collect(),
        }
    }

    pub fn identify_presence(&self) -> (String, Vec<Value>) {
        match self.current_session() {
            Some(ours) if ours.status == OFFLINE => (INVISIBLE.into(), ours.activities.clone()),
            Some(ours) => (ours.status.clone(), ours.activities.clone()),
            None => (UNKNOWN.into(), vec![]),
        }
    }

    pub fn note_ignored(&mut self, event: &str) {
        match self.ignored.iter_mut().find(|(name, _)| name == event) {
            Some((_, count)) => *count += 1,
            None => self.ignored.push((event.to_string(), 1)),
        }
    }

    pub fn ignored_summary(&self, limit: usize) -> String {
        let mut top: Vec<&(String, u64)> = self.ignored.iter().collect();
        top.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let parts: Vec<String> = top
            .iter()
            .take(limit)
            .map(|(name, count)| format!("{}={}", name, count))
            .collect();
        if parts.is_empty() {
            "none".into()
        } else {
            parts.join(" ")
        }
    }

    pub fn describe_sessions(&self) -> Vec<String> {
        let ours = self.session_id.as_deref();
        self.sessions
            .iter()
            .map(|s| {
                let label = if s.session_id == ALL {
                    "rollup".to_string()
                } else if Some(s.session_id.as_str()) == ours {
                    "this bot".to_string()
                } else {
                    s.client.clone()
                };
                format!(
                    "session {} status={} active={}",
                    label,
                    s.status,
                    if s.active { "True" } else { "False" }
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log;

    #[test]
    fn client_activities_fall_back_to_the_settings_custom_status() {
        let mut state = State::new();
        let settings = [0x5a, 8, 0x12, 6, 0x0a, 4, b'b', b'u', b's', b'y'];
        state.ready(
            "me".into(),
            Some(vec![p("other", "desktop", "online")]),
            Some(&proto::b64encode(&settings)),
        );
        assert_eq!(
            state.client_activities(),
            vec![serde_json::json!({"type": 4, "name": "Custom Status", "state": "busy"})]
        );
        let mut ours = p("me", "web", "online");
        ours.activities = vec![serde_json::json!({"type": 0, "name": "game"})];
        state.sessions_replace(&[ours, p("other", "desktop", "online")], false);
        assert_eq!(
            state.client_activities(),
            vec![serde_json::json!({"type": 0, "name": "game"})]
        );
    }

    fn p(id: &str, client: &str, status: &str) -> SessionPayload {
        SessionPayload {
            session_id: id.into(),
            client_info: ClientInfo { client: client.into() },
            status: status.into(),
            active: false,
            activities: vec![],
        }
    }

    fn ready(sessions: Vec<SessionPayload>) -> State {
        let mut state = State::new();
        let settings = proto::b64encode(&proto::status_patch(&[], "dnd"));
        state.ready("me".into(), Some(sessions), Some(&settings));
        state
    }

    fn ids(state: &State) -> Vec<&str> {
        state.sessions.iter().map(|s| s.session_id.as_str()).collect()
    }

    #[test]
    fn ready_seeds_sessions_settings_and_our_status() {
        let state = ready(vec![
            p("all", "unknown", "online"),
            p("me", "web", "idle"),
            p("phone", "mobile", "online"),
        ]);
        assert_eq!(ids(&state), vec!["all", "me", "phone"]);
        assert!(state.is_on_mobile());
        assert_eq!(state.other_sessions(), 1);
        assert_eq!(state.status(), "online");
        assert_eq!(state.own_status().as_deref(), Some("idle"));
        assert_eq!(state.saved_status(), "dnd");
        assert_eq!(state.ws_status, "idle");
    }

    #[test]
    fn a_missing_rollup_is_faked_from_our_session() {
        let state = ready(vec![p("desk", "desktop", "dnd"), p("me", "web", "idle")]);
        let all = state.find(ALL).unwrap();
        assert_eq!(all.status, "idle");
        assert_eq!(all.client, "unknown");
        assert_eq!(state.other_sessions(), 1);
        assert!(!state.is_on_mobile());
    }

    #[test]
    fn a_missing_rollup_without_us_uses_the_first_session() {
        let state = ready(vec![p("desk", "desktop", "dnd"), p("web2", "web", "idle")]);
        assert_eq!(state.status(), "dnd");
        assert_eq!(state.own_status(), None);
    }

    #[test]
    fn replace_reports_creates_updates_and_deletes() {
        let mut state = ready(vec![p("all", "unknown", "online"), p("me", "web", "online")]);
        assert!(!state.sessions_replace(&[p("all", "unknown", "online"), p("me", "web", "online")], false));
        assert!(state.sessions_replace(
            &[
                p("all", "unknown", "online"),
                p("me", "web", "online"),
                p("phone", "mobile", "online")
            ],
            false
        ));
        assert!(state.is_on_mobile());
        assert!(state.sessions_replace(
            &[
                p("all", "unknown", "idle"),
                p("me", "web", "online"),
                p("phone", "mobile", "online")
            ],
            false
        ));
        assert_eq!(state.status(), "idle");
        assert!(state.sessions_replace(&[p("all", "unknown", "idle"), p("me", "web", "online")], false));
        assert!(!state.is_on_mobile());
        assert_eq!(ids(&state), vec!["all", "me"]);
    }

    #[test]
    fn a_dropped_rollup_is_rebuilt_from_our_session() {
        let mut state = ready(vec![
            p("all", "unknown", "online"),
            p("me", "web", "online"),
            p("x", "desktop", "dnd"),
        ]);
        assert!(state.sessions_replace(&[p("me", "web", "idle")], false));
        assert_eq!(ids(&state), vec!["me", "all"]);
        assert_eq!(state.status(), "idle");
    }

    #[test]
    fn an_empty_replace_is_discarded() {
        let mut state = ready(vec![p("me", "web", "online"), p("phone", "mobile", "online")]);
        log::take();
        assert!(!state.sessions_replace(&[], false));
        assert!(state.is_on_mobile());
        assert!(log::take().iter().any(|(_, m)| m.contains("no sessions")));
    }

    #[test]
    fn ready_clears_old_sessions() {
        let mut state = ready(vec![p("me", "web", "online"), p("phone", "mobile", "online")]);
        state.ready("me2".into(), Some(vec![p("me2", "web", "online")]), None);
        assert!(!state.is_on_mobile());
        assert_eq!(state.saved_status(), "unknown");
        assert_eq!(state.other_sessions(), 0);
    }

    #[test]
    fn settings_updates_report_before_and_after() {
        let mut state = ready(vec![p("me", "web", "online")]);
        let idle = proto::b64encode(&proto::status_patch(&[], "idle"));
        assert_eq!(
            state.settings_update(1, &idle, false),
            Some(("idle".into(), "dnd".into()))
        );
        let online = proto::b64encode(&proto::status_patch(&[], "online"));
        assert_eq!(
            state.settings_update(1, &online, true),
            Some(("online".into(), "idle".into()))
        );
        assert_eq!(state.saved_status(), "online");
        assert_eq!(state.settings_update(2, &idle, false), None);
        assert_eq!(state.saved_status(), "online");
    }

    #[test]
    fn settings_updates_before_ready_are_ignored() {
        let mut state = State::new();
        let idle = proto::b64encode(&proto::status_patch(&[], "idle"));
        assert_eq!(state.settings_update(1, &idle, false), None);
        assert_eq!(state.saved_status(), "unknown");
    }

    #[test]
    fn status_falls_back_through_ws_status_then_settings() {
        let mut state = State::new();
        assert_eq!(state.status(), "offline");
        state.settings = Some(proto::status_patch(&[], "dnd"));
        assert_eq!(state.status(), "dnd");
        state.ws_status = "invisible".into();
        assert_eq!(state.status(), "invisible");
    }

    #[test]
    fn identify_presence_follows_our_session() {
        let mut state = State::new();
        assert_eq!(state.identify_presence().0, "unknown");
        state = ready(vec![p("me", "web", "offline")]);
        assert_eq!(state.identify_presence().0, "invisible");
        state = ready(vec![p("me", "web", "dnd")]);
        assert_eq!(state.identify_presence().0, "dnd");
    }

    #[test]
    fn ignored_events_summarise_most_common_first() {
        let mut state = State::new();
        assert_eq!(state.ignored_summary(8), "none");
        for event in ["A", "B", "B", "C", "B", "A"] {
            state.note_ignored(event);
        }
        assert_eq!(state.ignored_summary(8), "B=3 A=2 C=1");
        assert_eq!(state.ignored_summary(1), "B=3");
    }

    #[test]
    fn session_payloads_parse_from_gateway_json() {
        let json = r#"[{"session_id":"abc","client_info":{"client":"mobile","os":"ios","version":0},
            "status":"online","activities":[{"type":4,"state":"hi"}],"active":true,"hidden_activities":[]}]"#;
        let parsed: Vec<SessionPayload> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed[0].client_info.client, "mobile");
        assert!(parsed[0].active);
        assert_eq!(parsed[0].activities.len(), 1);
    }

    #[test]
    fn describe_labels_like_the_python_bot() {
        let state = ready(vec![
            p("all", "unknown", "online"),
            p("me", "web", "online"),
            p("p", "mobile", "idle"),
        ]);
        assert_eq!(
            state.describe_sessions(),
            vec![
                "session rollup status=online active=False",
                "session this bot status=online active=False",
                "session mobile status=idle active=False"
            ]
        );
    }
}
