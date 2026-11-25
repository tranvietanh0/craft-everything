//! Plane sync (optional, `PLANE_*` keys): one Plane project per port, one work item per feature
//! whose state follows the builder's, and a work item labelled `bug` each time a feature breaks.
//! REST API v1 with the `X-API-Key` header (developers.plane.so/api-reference). The HTTP calls go
//! through [`PlaneApi`], so the sync logic is tested with a fake. The ids it makes are kept in
//! [`PlaneIds`] (saved in the port's `bot.json`), so nothing is created twice.

use crate::config::PlaneSettings;
use crate::tester::{plane_identifier, plane_mapping};
use crate::text::clip;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
/// Plane's largest page.
const PER_PAGE: u32 = 100;
/// Pages read at most when listing.
const MAX_PAGES: usize = 50;
const NAME_LIMIT: usize = 255;
const ERROR_BODY_CHARS: usize = 300;
const USER_AGENT: &str = concat!("craft-anything-discord/", env!("CARGO_PKG_VERSION"));
const BUG_LABEL: &str = "bug";
const BUG_COLOUR: &str = "#e5484d";
const DONE: &str = "Done";

/// The states the bot uses: name, Plane state group and colour. A project made by Plane has all
/// but "Ready to test"; any that is missing is created.
const STATES: [(&str, &str, &str); 5] = [
    ("Backlog", "backlog", "#a3a3a3"),
    ("Todo", "unstarted", "#3f76ff"),
    ("In Progress", "started", "#f59e0b"),
    ("Ready to test", "started", "#8b5cf6"),
    (DONE, "completed", "#16a34a"),
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlaneError {
    #[error("Plane answered {status} to {method} {path}: {body}")]
    Status { method: &'static str, path: String, status: u16, body: String },
    #[error("couldn't reach Plane: {0}")]
    Network(String),
    #[error("Plane's answer was not understood: {0}")]
    Shape(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Patch,
}

impl Method {
    fn name(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Patch => "PATCH",
        }
    }
}

/// One Plane REST call. `path` is under `/api/v1/workspaces/<workspace>/`, such as `projects/`.
pub trait PlaneApi: Send + Sync {
    fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value, PlaneError>;
}

/// The real API over HTTP. The key is only ever put in the request header.
pub struct HttpPlane {
    agent: ureq::Agent,
    base: String,
    settings: PlaneSettings,
}

impl HttpPlane {
    pub fn new(settings: &PlaneSettings) -> HttpPlane {
        let agent = ureq::Agent::config_builder().http_status_as_error(false).user_agent(USER_AGENT).timeout_global(Some(TIMEOUT)).build().new_agent();
        let base = format!("{}/api/v1/workspaces/{}/", settings.url, settings.workspace);
        HttpPlane { agent, base, settings: settings.clone() }
    }
}

impl PlaneApi for HttpPlane {
    fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value, PlaneError> {
        let url = format!("{}{path}", self.base);
        let key = self.settings.api_key.expose();
        let payload = body.map(Value::to_string).unwrap_or_else(|| "{}".to_owned());
        let sent = match method {
            Method::Get => self.agent.get(&url).header("X-API-Key", key).header("Accept", "application/json").call(),
            Method::Post => self.agent.post(&url).header("X-API-Key", key).header("Content-Type", "application/json").send(payload.as_str()),
            Method::Patch => self.agent.patch(&url).header("X-API-Key", key).header("Content-Type", "application/json").send(payload.as_str()),
        };
        let response = sent.map_err(|e| PlaneError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response.into_body().read_to_string().map_err(|e| PlaneError::Network(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(PlaneError::Status { method: method.name(), path: path.to_owned(), status, body: clip(text.trim(), ERROR_BODY_CHARS) });
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| PlaneError::Shape(e.to_string()))
    }
}

/// What the bot made in Plane for one port.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaneIds {
    pub project: String,
    pub identifier: String,
    /// State name → id, as read from (or made in) the project.
    pub states: BTreeMap<String, String>,
    pub bug_label: Option<String>,
    /// Feature name → its work item.
    pub items: BTreeMap<String, String>,
    /// Feature name → its bug work items not yet closed.
    pub open_bugs: BTreeMap<String, Vec<String>>,
    /// Feature name → the state Plane was last brought to. The same state reported again (the
    /// run reports a tester's verdict back after its next report) changes nothing.
    pub synced: BTreeMap<String, String>,
    /// The instance predates `work-items/` and only has `issues/`.
    pub legacy_issues: bool,
}

/// Every item of a list endpoint, following Plane's cursor pages.
fn list(api: &dyn PlaneApi, path: &str) -> Result<Vec<Value>, PlaneError> {
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let page_path = match &cursor {
            Some(c) => format!("{path}?per_page={PER_PAGE}&cursor={c}"),
            None => format!("{path}?per_page={PER_PAGE}"),
        };
        let page = api.call(Method::Get, &page_path, None)?;
        if let Value::Array(items) = page {
            all.extend(items);
            return Ok(all);
        }
        let items = page.get("results").and_then(Value::as_array).ok_or_else(|| PlaneError::Shape(format!("{path} gave no results list")))?;
        all.extend(items.iter().cloned());
        let more = page.get("next_page_results").and_then(Value::as_bool).unwrap_or(false);
        match page.get("next_cursor").and_then(Value::as_str) {
            Some(next) if more => cursor = Some(next.to_owned()),
            _ => return Ok(all),
        }
    }
    Ok(all)
}

fn id_of(value: &Value, what: &str) -> Result<String, PlaneError> {
    value.get("id").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| PlaneError::Shape(format!("the new {what} has no id")))
}

fn text_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Makes the port's Plane project once: named after the app, with a unique identifier from the slug.
pub fn ensure_project(api: &dyn PlaneApi, ids: &mut Option<PlaneIds>, slug: &str, label: &str) -> Result<(), PlaneError> {
    if ids.is_some() {
        return Ok(());
    }
    let projects = list(api, "projects/")?;
    let taken: Vec<String> = projects.iter().filter_map(|p| text_field(p, "identifier")).map(str::to_ascii_uppercase).collect();
    let identifier = plane_identifier(slug, &taken).map_err(|e| PlaneError::Shape(e.to_string()))?;
    let name_taken = projects.iter().any(|p| text_field(p, "name").is_some_and(|n| n.eq_ignore_ascii_case(label)));
    let name = if name_taken { format!("{label} (port-{slug})") } else { label.to_owned() };
    let body = json!({
        "name": clip(&name, NAME_LIMIT),
        "identifier": identifier,
        "description": format!("Port of {label}, run by the Craft porter bot (channel port-{slug})."),
    });
    let created = api.call(Method::Post, "projects/", Some(&body))?;
    *ids = Some(PlaneIds { project: id_of(&created, "project")?, identifier, ..PlaneIds::default() });
    Ok(())
}

fn ensure_state(api: &dyn PlaneApi, ids: &mut PlaneIds, name: &str) -> Result<String, PlaneError> {
    let known = |ids: &PlaneIds| ids.states.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, id)| id.clone());
    if let Some(id) = known(ids) {
        return Ok(id);
    }
    for state in list(api, &format!("projects/{}/states/", ids.project))? {
        if let (Some(n), Some(id)) = (text_field(&state, "name"), text_field(&state, "id")) {
            ids.states.insert(n.to_owned(), id.to_owned());
        }
    }
    if let Some(id) = known(ids) {
        return Ok(id);
    }
    let (group, colour) = STATES.iter().find(|(n, _, _)| *n == name).map_or(("started", "#8b5cf6"), |(_, g, c)| (*g, *c));
    let created = api.call(Method::Post, &format!("projects/{}/states/", ids.project), Some(&json!({"name": name, "color": colour, "group": group})))?;
    let id = id_of(&created, "state")?;
    ids.states.insert(name.to_owned(), id.clone());
    Ok(id)
}

fn ensure_bug_label(api: &dyn PlaneApi, ids: &mut PlaneIds) -> Result<String, PlaneError> {
    if let Some(id) = &ids.bug_label {
        return Ok(id.clone());
    }
    let path = format!("projects/{}/labels/", ids.project);
    let found = list(api, &path)?.into_iter().find(|l| text_field(l, "name").is_some_and(|n| n.eq_ignore_ascii_case(BUG_LABEL)));
    let id = match found {
        Some(label) => id_of(&label, "label")?,
        None => id_of(&api.call(Method::Post, &path, Some(&json!({"name": BUG_LABEL, "color": BUG_COLOUR})))?, "label")?,
    };
    ids.bug_label = Some(id.clone());
    Ok(id)
}

fn items_path(ids: &PlaneIds) -> String {
    format!("projects/{}/{}/", ids.project, if ids.legacy_issues { "issues" } else { "work-items" })
}

/// Creates a work item; an instance without `work-items/` (404) is asked again at `issues/`.
fn create_item(api: &dyn PlaneApi, ids: &mut PlaneIds, body: &Value) -> Result<String, PlaneError> {
    match api.call(Method::Post, &items_path(ids), Some(body)) {
        Err(PlaneError::Status { status: 404, .. }) if !ids.legacy_issues => {
            ids.legacy_issues = true;
            id_of(&api.call(Method::Post, &items_path(ids), Some(body))?, "work item")
        }
        other => id_of(&other?, "work item"),
    }
}

fn set_item_state(api: &dyn PlaneApi, ids: &PlaneIds, item: &str, state: &str) -> Result<(), PlaneError> {
    api.call(Method::Patch, &format!("{}{item}/", items_path(ids)), Some(&json!({"state": state}))).map(|_| ())
}

/// A note as a Plane description: escaped HTML paragraphs.
pub fn description_html(text: &str) -> String {
    let escaped = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    escaped.lines().map(|line| format!("<p>{line}</p>")).collect()
}

/// Brings Plane up to date with one feature change. A state Plane is not told about (an unknown
/// name) changes nothing. `ids` keeps whatever was made, even when a later call fails.
pub fn sync_feature(
    api: &dyn PlaneApi,
    ids: &mut Option<PlaneIds>,
    port: (&str, &str),
    feature: &str,
    state: &str,
    note: Option<&str>,
) -> Result<(), PlaneError> {
    let Some(mapping) = plane_mapping(state) else {
        return Ok(());
    };
    let (slug, label) = port;
    ensure_project(api, ids, slug, label)?;
    let Some(ids) = ids.as_mut() else {
        return Err(PlaneError::Shape("no Plane project".to_owned()));
    };
    if ids.synced.get(feature).is_some_and(|s| s == state) {
        return Ok(());
    }
    let state_id = ensure_state(api, ids, mapping.state)?;
    match ids.items.get(feature).cloned() {
        Some(item) => set_item_state(api, ids, &item, &state_id)?,
        None => {
            let item = create_item(api, ids, &json!({"name": clip(feature, NAME_LIMIT), "state": state_id}))?;
            ids.items.insert(feature.to_owned(), item);
        }
    }
    if mapping.bug {
        let bug_label = ensure_bug_label(api, ids)?;
        let what = note.map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Reported broken.");
        let body = json!({
            "name": clip(&format!("Bug: {feature}"), NAME_LIMIT),
            "state": state_id,
            "labels": [bug_label],
            "description_html": description_html(what),
        });
        let bug = create_item(api, ids, &body)?;
        ids.open_bugs.entry(feature.to_owned()).or_default().push(bug);
    }
    if state == "passed"
        && let Some(bugs) = ids.open_bugs.get(feature).cloned()
    {
        let done = ensure_state(api, ids, DONE)?;
        for bug in &bugs {
            set_item_state(api, ids, bug, &done)?;
        }
        ids.open_bugs.remove(feature);
    }
    ids.synced.insert(feature.to_owned(), state.to_owned());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_platform::sync::Mutex;

    type Handler = Box<dyn Fn(Method, &str, Option<&Value>) -> Result<Value, PlaneError> + Send + Sync>;

    struct FakePlane {
        calls: Mutex<Vec<(Method, String, Option<Value>)>>,
        handler: Handler,
    }

    impl FakePlane {
        fn new(handler: impl Fn(Method, &str, Option<&Value>) -> Result<Value, PlaneError> + Send + Sync + 'static) -> FakePlane {
            FakePlane { calls: Mutex::new(Vec::new()), handler: Box::new(handler) }
        }

        fn posts(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().iter().filter(|(m, _, _)| *m == Method::Post).map(|(_, p, b)| (p.clone(), b.clone().unwrap_or(Value::Null))).collect()
        }

        fn patches(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().iter().filter(|(m, _, _)| *m == Method::Patch).map(|(_, p, b)| (p.clone(), b.clone().unwrap_or(Value::Null))).collect()
        }
    }

    impl PlaneApi for FakePlane {
        fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value, PlaneError> {
            self.calls.lock().unwrap().push((method, path.to_owned(), body.cloned()));
            (self.handler)(method, path, body)
        }
    }

    /// A workspace with one other project (`RETRO`) whose new projects get Plane's default states.
    fn workspace(method: Method, path: &str, body: Option<&Value>) -> Result<Value, PlaneError> {
        let page = |items: Value| Ok(json!({"results": items, "next_page_results": false, "next_cursor": null}));
        match (method, path.split('?').next().unwrap_or(path)) {
            (Method::Get, "projects/") => page(json!([{"id": "p0", "identifier": "RETRO", "name": "Retro"}])),
            (Method::Post, "projects/") => Ok(json!({"id": "p1"})),
            (Method::Get, "projects/p1/states/") => page(json!([
                {"id": "s-backlog", "name": "Backlog"}, {"id": "s-todo", "name": "Todo"},
                {"id": "s-progress", "name": "In Progress"}, {"id": "s-done", "name": "Done"}
            ])),
            (Method::Post, "projects/p1/states/") => Ok(json!({"id": "s-ready"})),
            (Method::Get, "projects/p1/labels/") => page(json!([])),
            (Method::Post, "projects/p1/labels/") => Ok(json!({"id": "l-bug"})),
            (Method::Post, "projects/p1/work-items/") => {
                let name = body.and_then(|b| b.get("name")).and_then(Value::as_str).unwrap_or_default();
                Ok(json!({"id": if name.starts_with("Bug:") { "bug-1" } else { "item-1" }}))
            }
            (Method::Patch, _) => Ok(json!({})),
            (m, p) => Err(PlaneError::Status { method: m.name(), path: p.to_owned(), status: 404, body: String::new() }),
        }
    }

    #[test]
    fn a_new_port_gets_a_project_with_a_free_identifier() {
        let api = FakePlane::new(workspace);
        let mut ids = None;
        ensure_project(&api, &mut ids, "retro-racer", "Retro Racer").unwrap();
        let ids = ids.unwrap();
        assert_eq!((ids.project.as_str(), ids.identifier.as_str()), ("p1", "RETR2"), "RETRO is taken");
        assert_eq!(api.posts()[0].1["identifier"], "RETR2");
        let mut again = Some(ids);
        ensure_project(&api, &mut again, "retro-racer", "Retro Racer").unwrap();
        assert_eq!(api.posts().len(), 1, "a port's project is made once");
    }

    #[test]
    fn a_ready_feature_gets_a_work_item_in_a_ready_to_test_state_made_for_it() {
        let api = FakePlane::new(workspace);
        let mut ids = None;
        sync_feature(&api, &mut ids, ("snake", "Snake"), "Map", "ready_to_test", None).unwrap();
        let posts = api.posts();
        assert!(posts.iter().any(|(p, b)| p == "projects/p1/states/" && b["name"] == "Ready to test"), "{posts:?}");
        let item = posts.iter().find(|(p, _)| p == "projects/p1/work-items/").unwrap();
        assert_eq!((item.1["name"].as_str(), item.1["state"].as_str()), (Some("Map"), Some("s-ready")));
        assert_eq!(ids.unwrap().items.get("Map").map(String::as_str), Some("item-1"));
    }

    #[test]
    fn a_broken_feature_moves_its_item_and_files_a_labelled_bug_closed_when_it_passes() {
        let api = FakePlane::new(workspace);
        let mut ids = None;
        sync_feature(&api, &mut ids, ("snake", "Snake"), "Map", "in_progress", None).unwrap();
        sync_feature(&api, &mut ids, ("snake", "Snake"), "Map", "broken", Some("The map is <blank>")).unwrap();
        let bug = api.posts().into_iter().find(|(_, b)| b["name"] == "Bug: Map").unwrap().1;
        assert_eq!(bug["labels"], json!(["l-bug"]));
        assert_eq!(bug["state"], "s-todo");
        assert!(bug["description_html"].as_str().unwrap().contains("&lt;blank&gt;"), "{bug}");
        assert!(api.patches().iter().any(|(p, b)| p == "projects/p1/work-items/item-1/" && b["state"] == "s-todo"));
        sync_feature(&api, &mut ids, ("snake", "Snake"), "Map", "passed", None).unwrap();
        assert!(api.patches().iter().any(|(p, b)| p == "projects/p1/work-items/bug-1/" && b["state"] == "s-done"), "the bug closes");
        assert!(ids.unwrap().open_bugs.is_empty());
    }

    #[test]
    fn the_same_state_reported_again_files_no_second_bug() {
        let api = FakePlane::new(workspace);
        let mut ids = None;
        sync_feature(&api, &mut ids, ("snake", "Snake"), "Map", "broken", Some("blank")).unwrap();
        sync_feature(&api, &mut ids, ("snake", "Snake"), "Map", "broken", None).unwrap();
        assert_eq!(api.posts().iter().filter(|(_, b)| b["name"] == "Bug: Map").count(), 1);
        sync_feature(&api, &mut ids, ("snake", "Snake"), "Map", "ready_to_test", None).unwrap();
        sync_feature(&api, &mut ids, ("snake", "Snake"), "Map", "broken", Some("still blank")).unwrap();
        assert_eq!(api.posts().iter().filter(|(_, b)| b["name"] == "Bug: Map").count(), 2, "a new break is a new bug");
    }

    #[test]
    fn an_instance_without_work_items_is_asked_at_issues() {
        let api = FakePlane::new(|method, path, body| match (method, path) {
            (Method::Post, "projects/p1/work-items/") => Err(PlaneError::Status { method: "POST", path: path.to_owned(), status: 404, body: String::new() }),
            (Method::Post, "projects/p1/issues/") => Ok(json!({"id": "old-1"})),
            _ => workspace(method, path, body),
        });
        let mut ids = None;
        sync_feature(&api, &mut ids, ("snake", "Snake"), "Map", "planned", None).unwrap();
        let ids = ids.unwrap();
        assert!(ids.legacy_issues);
        assert_eq!(ids.items.get("Map").map(String::as_str), Some("old-1"));
    }

    #[test]
    fn a_failed_call_keeps_what_was_already_made() {
        let api = FakePlane::new(|method, path, body| match (method, path.split('?').next().unwrap_or(path)) {
            (Method::Get, "projects/p1/states/") => Err(PlaneError::Network("down".into())),
            _ => workspace(method, path, body),
        });
        let mut ids = None;
        assert!(sync_feature(&api, &mut ids, ("snake", "Snake"), "Map", "planned", None).is_err());
        assert_eq!(ids.map(|i| i.project), Some("p1".to_owned()), "the project is not made again next time");
    }
}
