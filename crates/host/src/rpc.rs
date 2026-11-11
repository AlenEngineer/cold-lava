//! JSON-RPC 2.0 over a transport: reading and writing messages, and answering what an extension
//! asks of the host through `Host`, so the permission gate and the queued effects apply to an
//! extension in another process exactly as to one built in.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

use super::view::{Content, ViewId};
use super::{Host, PaneId, PaneSpec};
use crate::files;
use crate::gateway::{self, Answer, Reason, Request, Subscribed, Ticket};

/// The API version cold-lava speaks, sent in `initialize` and checked against a manifest's `api`.
pub const API_VERSION: u32 = 1;

/// The JSON-RPC error codes cold-lava sends.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
/// The workspace gateway refused: `data` says why (`reason`, `message`).
pub const REFUSED: i64 = -32000;

/// A JSON-RPC error object. `data` is a `gateway::Error`: `{reason, message}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, reason: Reason, message: String) -> RpcError {
        let data = serde_json::to_value(gateway::Error::new(reason, message.clone())).ok();
        RpcError {
            code,
            message,
            data,
        }
    }
}

impl From<gateway::Error> for RpcError {
    fn from(e: gateway::Error) -> RpcError {
        RpcError::new(REFUSED, e.reason, e.message)
    }
}

/// One message from an extension.
#[derive(Debug, PartialEq)]
pub enum Message {
    Request {
        id: Value,
        method: String,
        params: Option<Value>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
    Response {
        id: Value,
        outcome: Result<Value, RpcError>,
    },
    /// Something cold-lava couldn't read, answered with `error` under `id` (`null` when unknown).
    Invalid { id: Value, error: RpcError },
}

pub fn parse(bytes: &[u8]) -> Message {
    let v: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            let error = RpcError::new(PARSE_ERROR, Reason::Refused, format!("not JSON: {e}"));
            return Message::Invalid {
                id: Value::Null,
                error,
            };
        }
    };
    let id = v.get("id").cloned();
    let invalid = |why: &str| Message::Invalid {
        id: id.clone().unwrap_or(Value::Null),
        error: RpcError::new(INVALID_REQUEST, Reason::Refused, why.to_string()),
    };
    if v.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return invalid("not a JSON-RPC 2.0 message: \"jsonrpc\" must be \"2.0\"");
    }
    let params = v.get("params").cloned();
    match (v.get("method"), id.clone()) {
        (Some(Value::String(method)), Some(id)) => Message::Request {
            id,
            method: method.clone(),
            params,
        },
        (Some(Value::String(method)), None) => Message::Notification {
            method: method.clone(),
            params,
        },
        (Some(_), _) => invalid("\"method\" must be a string"),
        (None, Some(id)) => {
            let outcome = match (v.get("result"), v.get("error")) {
                (_, Some(e)) => match serde_json::from_value(e.clone()) {
                    Ok(e) => Err(e),
                    Err(_) => return invalid("\"error\" must be a JSON-RPC error object"),
                },
                (Some(r), None) => Ok(r.clone()),
                (None, None) => Ok(Value::Null),
            };
            Message::Response { id, outcome }
        }
        (None, None) => invalid("neither a request nor a response"),
    }
}

pub fn request(id: u64, method: &str, params: Value) -> Vec<u8> {
    to_bytes(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
}

/// A notification from a message of the in-process protocol: `{"method", "params"}`.
pub fn notification(message: impl Serialize) -> Vec<u8> {
    let mut v = serde_json::to_value(message).unwrap_or(Value::Null);
    if let Value::Object(m) = &mut v {
        m.insert("jsonrpc".into(), "2.0".into());
    }
    to_bytes(v)
}

pub fn response(id: Value, outcome: Result<Value, RpcError>) -> Vec<u8> {
    to_bytes(match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    })
}

fn to_bytes(v: Value) -> Vec<u8> {
    serde_json::to_vec(&v).unwrap_or_default()
}

pub fn answer<T: Serialize>(v: Result<T, gateway::Error>) -> Result<Value, RpcError> {
    Ok(serde_json::to_value(v?).unwrap_or(Value::Null))
}

/// The host methods that aren't the workspace gateway's (extension → host).
#[derive(Debug, Deserialize)]
#[serde(tag = "method", content = "params")]
enum Call {
    #[serde(rename = "ui/notify")]
    Notify { message: String },
    /// The extension's own part of the status line, kept until it sends another; `null` clears.
    #[serde(rename = "ui/status")]
    Status { text: Option<String> },
    #[serde(rename = "command/run")]
    RunCommand {
        id: String,
        #[serde(default)]
        args: Value,
    },
    #[serde(rename = "workspace/saved")]
    Saved { note: PathBuf },
    #[serde(rename = "editor/opened")]
    EditorOpened { note: PathBuf },
    #[serde(rename = "pane/open")]
    OpenPane(PaneSpec),
    #[serde(rename = "pane/input")]
    PaneInput { pane: PaneId, text: String },
    #[serde(rename = "view/open")]
    OpenView {
        content: Content,
        #[serde(default)]
        select: Option<String>,
        #[serde(default)]
        file: Option<String>,
    },
    #[serde(rename = "view/update")]
    UpdateView {
        view: ViewId,
        content: Content,
        #[serde(default)]
        select: Option<String>,
    },
    #[serde(rename = "view/close")]
    CloseView { view: ViewId },
    #[serde(rename = "view/show")]
    ShowView { view: ViewId },
    #[serde(rename = "workspace/open")]
    OpenNote {
        note: PathBuf,
        #[serde(default)]
        line: Option<usize>,
        #[serde(default)]
        item: Option<String>,
    },
}

/// The methods an extension may call, sent in `initialize` as its `capabilities`.
pub const HOST_METHODS: &[&str] = &[
    "items/query",
    "items/subscribe",
    "items/unsubscribe",
    "items/edit",
    "types/list",
    "workspace/style",
    "links/query",
    "links/subscribe",
    "links/unsubscribe",
    "workspace/read",
    "workspace/write",
    "workspace/create",
    "workspace/files",
    "ui/notify",
    "ui/status",
    "command/run",
    "workspace/saved",
    "editor/opened",
    "pane/open",
    "pane/input",
    "view/open",
    "view/update",
    "view/close",
    "view/show",
    "workspace/open",
];

const GATEWAY: &[&str] = &[
    "items/query",
    "items/subscribe",
    "items/unsubscribe",
    "items/edit",
    "types/list",
    "workspace/style",
    "links/query",
    "links/subscribe",
    "links/unsubscribe",
];

const FILES: &[&str] = &[
    "workspace/read",
    "workspace/write",
    "workspace/create",
    "workspace/files",
];

/// What `serve` answers: now, or later under a ticket, which `Extension::edited` brings back
/// with the answer (an `items/edit` of a file another owner reads).
#[derive(Debug)]
pub enum Served {
    Now(Result<Value, RpcError>),
    Later(Ticket),
}

/// Answer one request from an extension. The gateway's are answered within its manifest's
/// permissions and the kinds of file it owns, now, except an edit of a file another owner
/// reads; the rest queue effects and answer `null`.
pub fn serve(method: &str, params: Option<Value>, host: &mut Host<'_>) -> Served {
    let mut later = None;
    let now = serve_now(method, params, host, &mut later);
    match later {
        Some(ticket) => Served::Later(ticket),
        None => Served::Now(now),
    }
}

/// `serve`, setting `later` when the answer comes later.
fn serve_now(
    method: &str,
    params: Option<Value>,
    host: &mut Host<'_>,
    later: &mut Option<Ticket>,
) -> Result<Value, RpcError> {
    let mut message = json!({ "method": method });
    match params {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) if m.is_empty() => {}
        Some(p) => {
            if let Some(m) = message.as_object_mut() {
                m.insert("params".into(), p);
            }
        }
    }
    let invalid = |e: serde_json::Error| {
        RpcError::new(INVALID_PARAMS, Reason::Refused, format!("{method}: {e}"))
    };
    if GATEWAY.contains(&method) {
        return match serde_json::from_value(message).map_err(invalid)? {
            Request::Query(q) => answer(host.query_items(&q)),
            Request::Subscribe(select) => answer(
                host.subscribe_items(select)
                    .map(|subscription| Subscribed { subscription }),
            ),
            Request::Unsubscribe { subscription } => answer(host.unsubscribe_items(subscription)),
            Request::Edit(edit) => match host.edit_item(&edit) {
                Ok(Answer::Now(edited)) => answer(Ok(edited)),
                Ok(Answer::Later(ticket)) => {
                    *later = Some(ticket);
                    Ok(Value::Null)
                }
                Err(e) => Err(e.into()),
            },
            Request::TypesList => answer(host.types()),
            Request::Style => answer(host.style()),
            Request::Links => answer(host.query_links()),
            Request::LinksSubscribe => answer(
                host.subscribe_links()
                    .map(|subscription| Subscribed { subscription }),
            ),
            Request::LinksUnsubscribe { subscription } => {
                answer(host.unsubscribe_links(subscription))
            }
        };
    }
    if FILES.contains(&method) {
        return match serde_json::from_value(message).map_err(invalid)? {
            files::Request::Read { path } => answer(host.read_file(&path)),
            files::Request::Write { path, text, expect } => {
                answer(host.write_file(&path, &text, &expect))
            }
            files::Request::Create(create) => answer(host.create_file(&create)),
            files::Request::Files { extension } => answer(host.list_files(&extension)),
        };
    }
    if !HOST_METHODS.contains(&method) {
        let message = format!("cold-lava has no method {method}");
        return Err(RpcError::new(METHOD_NOT_FOUND, Reason::Refused, message));
    }
    match serde_json::from_value(message).map_err(invalid)? {
        Call::Notify { message } => host.notify(message),
        Call::Status { text } => host.status(text),
        Call::RunCommand { id, args } => host.run_command(&id, args),
        Call::Saved { note } => host.saved(note),
        Call::EditorOpened { note } => host.editor_opened(note),
        Call::OpenPane(spec) => return Ok(json!({ "pane": host.open_pane(spec) })),
        Call::PaneInput { pane, text } => host.pane_input(pane, text),
        Call::OpenView {
            content,
            select,
            file,
        } => {
            let opened = host.open_view(content, select, file);
            return answer(opened.map(|view| json!({ "view": view })));
        }
        Call::UpdateView {
            view,
            content,
            select,
        } => return answer(host.update_view(view, content, select)),
        Call::CloseView { view } => host.close_view(view),
        Call::ShowView { view } => host.show_view(view),
        Call::OpenNote { note, line, item } => host.open_note(note, line, item),
    }
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Effect, Permission};
    use cold_lava_engine::core::Core;

    #[test]
    fn reads_requests_notifications_and_responses() {
        let read = |s: &str| parse(s.as_bytes());
        assert_eq!(
            read(r#"{"jsonrpc":"2.0","id":7,"method":"types/list"}"#),
            Message::Request {
                id: json!(7),
                method: "types/list".into(),
                params: None
            }
        );
        assert_eq!(
            read(r#"{"jsonrpc":"2.0","method":"ui/notify","params":{"message":"hi"}}"#),
            Message::Notification {
                method: "ui/notify".into(),
                params: Some(json!({"message": "hi"}))
            }
        );
        assert_eq!(
            read(r#"{"jsonrpc":"2.0","id":"a","result":{}}"#),
            Message::Response {
                id: json!("a"),
                outcome: Ok(json!({}))
            }
        );
        let error = read(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-1,"message":"no"}}"#);
        assert!(matches!(error, Message::Response { outcome: Err(e), .. } if e.message == "no"));

        let invalid = |s: &str| match read(s) {
            Message::Invalid { id, error } => (id, error.code),
            m => panic!("read {m:?}"),
        };
        assert_eq!(invalid("{oops"), (Value::Null, PARSE_ERROR));
        assert_eq!(
            invalid(r#"{"id":3,"method":"x"}"#),
            (json!(3), INVALID_REQUEST)
        );
        let method = invalid(r#"{"jsonrpc":"2.0","id":3,"method":1}"#);
        assert_eq!(method, (json!(3), INVALID_REQUEST));
    }

    #[test]
    fn writes_json_rpc_2() {
        let v = |b: Vec<u8>| serde_json::from_slice::<Value>(&b).unwrap();
        assert_eq!(
            v(request(1, "initialize", json!({"apiVersion": 1}))),
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"apiVersion": 1}})
        );
        assert_eq!(
            v(notification(gateway::News::Reset {
                subscription: gateway::SubId(4)
            })),
            json!({"jsonrpc": "2.0", "method": "items/reset", "params": {"subscription": 4}})
        );
        let refused = gateway::Error::new(Reason::Forbidden, "no");
        assert_eq!(
            v(response(json!(9), Err(refused.into()))),
            json!({"jsonrpc": "2.0", "id": 9, "error": {
                "code": REFUSED, "message": "no", "data": {"reason": "forbidden", "message": "no"},
            }})
        );
    }

    fn workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cold-lava-rpc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(dir.join("Plan.md"), "- [ ] Pay rent\n").unwrap();
        dir
    }

    /// What `serve` answers an extension with these permissions, and the effects it queued.
    fn served(
        core: &mut Core,
        permissions: &[Permission],
        method: &str,
        params: Value,
    ) -> (Result<Value, RpcError>, Vec<Effect>) {
        let (mut next_pane, mut next_view) = (0, 0);
        let mut host = Host {
            effects: Vec::new(),
            next_pane: &mut next_pane,
            next_view: &mut next_view,
            name: "wc",
            permissions,
            me: None,
            owners: &[],
            core: Some(core),
        };
        let answer = match serve(method, Some(params), &mut host) {
            Served::Now(answer) => answer,
            Served::Later(_) => panic!("{method} on a note is answered at once"),
        };
        (answer, host.effects)
    }

    fn reason(e: &RpcError) -> &str {
        e.data.as_ref().unwrap()["reason"].as_str().unwrap()
    }

    #[test]
    fn the_permission_gate_holds_over_the_wire() {
        use Permission::{ItemsEdit, ItemsRead};
        let dir = workspace("gate");
        let mut core = Core::open(&dir).unwrap();
        let task = json!({"kind": "task"});

        let (none, _) = served(&mut core, &[], "items/query", task.clone());
        let none = none.unwrap_err();
        assert_eq!((none.code, reason(&none)), (REFUSED, "forbidden"));
        assert!(none.message.contains("items.read"), "{}", none.message);

        let (found, _) = served(&mut core, &[ItemsRead], "items/query", task.clone());
        let item = found.unwrap()["items"][0].clone();
        assert_eq!(item["text"], "Pay rent");
        let tick = json!({"id": item["id"], "rev": item["rev"], "op": "toggle"});
        let (read_only, _) = served(&mut core, &[ItemsRead], "items/edit", tick.clone());
        assert_eq!(reason(&read_only.unwrap_err()), "forbidden");
        let (ticked, _) = served(
            &mut core,
            &[ItemsRead, ItemsEdit],
            "items/edit",
            tick.clone(),
        );
        assert_eq!(ticked.unwrap()["changed"], true);
        let (stale, _) = served(&mut core, &[ItemsRead, ItemsEdit], "items/edit", tick);
        assert_eq!(reason(&stale.unwrap_err()), "changed");

        let (sub, _) = served(&mut core, &[ItemsRead], "items/subscribe", task);
        assert!(sub.unwrap()["subscription"].is_u64());
        let (types, _) = served(&mut core, &[ItemsRead], "types/list", json!({}));
        assert!(types.unwrap().as_array().is_some_and(|t| !t.is_empty()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_link_graph_over_the_wire_and_news_when_it_changes() {
        use Permission::ItemsRead;
        let dir = workspace("links");
        std::fs::write(dir.join("Plan.md"), "- [ ] Pay rent [[Rent]] [[Nowhere]]\n").unwrap();
        std::fs::write(dir.join("Rent.md"), "#money\n").unwrap();
        let mut core = Core::open(&dir).unwrap();
        let (none, _) = served(&mut core, &[], "links/query", json!(null));
        assert_eq!(reason(&none.unwrap_err()), "forbidden");
        let (graph, _) = served(&mut core, &[ItemsRead], "links/query", json!(null));
        assert_eq!(
            graph.unwrap(),
            json!({
                "files": [{"path": "Plan.md", "name": "Plan"},
                          {"path": "Rent.md", "name": "Rent", "tags": ["money"]}],
                "links": [{"from": "Plan.md", "to": "Rent.md"},
                          {"from": "Plan.md", "target": "Nowhere"}],
            })
        );
        let (sub, _) = served(&mut core, &[ItemsRead], "links/subscribe", json!(null));
        let sub = sub.unwrap()["subscription"].as_u64().unwrap();
        core.note_changed_now(&dir.join("Rent.md"));
        assert!(core.take_item_news().is_empty(), "Rent didn't change");
        std::fs::write(dir.join("Rent.md"), "#money [[Plan]]\n").unwrap();
        core.note_changed_now(&dir.join("Rent.md"));
        std::fs::write(dir.join("New.md"), "").unwrap();
        core.note_changed_now(&dir.join("New.md"));
        let news: Vec<Value> = (core.take_item_news().into_iter())
            .map(|(who, n)| {
                assert_eq!(who, "wc");
                serde_json::to_value(n).unwrap()
            })
            .collect();
        assert_eq!(
            news,
            [
                json!({"method": "links/changed", "params": {"subscription": sub}}),
                json!({"method": "links/changed",
                       "params": {"subscription": sub, "created": ["New.md"]}}),
            ]
        );
        let params = json!({ "subscription": sub });
        let (gone, _) = served(&mut core, &[ItemsRead], "links/unsubscribe", params);
        assert!(gone.is_ok());
        core.note_changed_now(&dir.join("New.md"));
        assert!(core.take_item_news().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_workspaces_style_is_pinned_over_the_wire() {
        let dir = workspace("style");
        std::fs::write(
            dir.join("Plan.md"),
            "---\ntitle: Plan\ntags: [\"#home\"]\nfrom: 31/10/2026\n---\n\
             * [ ] Pay rent [due:: 2026-11-12]\n* See [Bills](Bills.md)\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(".obsidian/app.json"),
            r#"{"newLinkFormat": "relative"}"#,
        )
        .unwrap();
        let mut core = Core::open(&dir).unwrap();
        let (style, _) = served(
            &mut core,
            &[Permission::ItemsRead],
            "workspace/style",
            json!({}),
        );
        let counts = |pairs: &[(&str, u32)]| {
            Value::Object(
                pairs
                    .iter()
                    .map(|(k, n)| (k.to_string(), json!(n)))
                    .collect(),
            )
        };
        let choice = |value: &str, from: &str, pairs: &[(&str, u32)]| json!({"value": value, "from": from, "counts": counts(pairs)});
        assert_eq!(
            style.unwrap(),
            json!({
                "notes": 1,
                "list-marker": choice("*", "detected", &[("-", 0), ("*", 2), ("+", 0)]),
                "links": choice("markdown", "detected", &[("wikilink", 0), ("markdown", 1)]),
                "link-path": choice(
                    "relative",
                    "obsidian newLinkFormat",
                    &[("shortest", 1), ("relative", 0), ("absolute", 0)],
                ),
                "task-fields": choice("dataview", "detected", &[("emoji", 0), ("dataview", 1)]),
                "headings": choice("atx", "default", &[("atx", 0), ("setext", 0)]),
                "frontmatter-lists": choice(
                    "flow",
                    "detected",
                    &[("indented", 0), ("flush", 0), ("flow", 1)],
                ),
                "tags": choice("hash", "detected", &[("bare", 0), ("hash", 1)]),
                "key-order": {
                    "value": ["title", "tags", "from"],
                    "from": "detected",
                    "counts": {"from": 1, "tags": 1, "title": 1},
                },
                "dates": {
                    "due": choice(
                        "YYYY-MM-DD",
                        "detected",
                        &[
                            ("YYYY-MM-DD", 1),
                            ("YYYY/MM/DD", 0),
                            ("DD/MM/YYYY", 0),
                            ("MM/DD/YYYY", 0),
                            ("DD.MM.YYYY", 0),
                        ],
                    ),
                    "from": choice(
                        "DD/MM/YYYY",
                        "detected",
                        &[
                            ("YYYY-MM-DD", 0),
                            ("YYYY/MM/DD", 0),
                            ("DD/MM/YYYY", 1),
                            ("MM/DD/YYYY", 0),
                            ("DD.MM.YYYY", 0),
                        ],
                    ),
                },
            })
        );
        let (refused, _) = served(&mut core, &[], "workspace/style", json!({}));
        assert_eq!(reason(&refused.unwrap_err()), "forbidden");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_are_read_written_created_and_listed_over_the_wire() {
        use Permission::{FilesRead, FilesWrite};
        let dir = workspace("files");
        let mut core = Core::open(&dir).unwrap();
        let both = [FilesRead, FilesWrite];
        let plan = json!({"path": "Plan.md"});
        let (read, _) = served(&mut core, &both, "workspace/read", plan);
        let read = read.unwrap();
        assert_eq!(read["text"], "- [ ] Pay rent\n");
        let write = json!({"path": "Plan.md", "text": "- [ ] Pay rent\n- [ ] Call\n",
                           "expect": read["rev"]});
        let (written, _) = served(&mut core, &both, "workspace/write", write.clone());
        assert!(written.unwrap()["rev"].is_string());
        let (stale, _) = served(&mut core, &both, "workspace/write", write);
        let stale = stale.unwrap_err();
        assert_eq!((stale.code, reason(&stale)), (REFUSED, "changed"));
        let new = json!({"name": "Untitled", "extension": "csv", "text": "a\n"});
        let (created, _) = served(&mut core, &both, "workspace/create", new);
        assert_eq!(created.unwrap()["path"], "Untitled.csv");
        let (listed, _) = served(
            &mut core,
            &both,
            "workspace/files",
            json!({"extension": "csv"}),
        );
        assert_eq!(listed.unwrap(), json!({"files": ["Untitled.csv"]}));
        let outside = json!({"path": "../x.csv", "text": ""});
        let (refused, _) = served(&mut core, &both, "workspace/create", outside);
        assert_eq!(reason(&refused.unwrap_err()), "refused");
        let (bad, _) = served(&mut core, &both, "workspace/create", json!({"text": ""}));
        assert_eq!(bad.unwrap_err().code, INVALID_PARAMS);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_requests_are_json_rpc_errors() {
        let dir = workspace("bad");
        let mut core = Core::open(&dir).unwrap();
        let read = [Permission::ItemsRead];
        let op = json!({"id": "a", "rev": "b", "op": "set-status"});
        let (e, _) = served(&mut core, &read, "items/edit", op);
        let e = e.unwrap_err();
        assert_eq!((e.code, reason(&e)), (INVALID_PARAMS, "refused"));
        assert!(
            e.message.contains("set-field"),
            "names the ops there are: {}",
            e.message
        );
        let (e, _) = served(&mut core, &read, "files/read", json!({}));
        assert_eq!(e.unwrap_err().code, METHOD_NOT_FOUND);
        let (e, _) = served(&mut core, &read, "ui/notify", json!({"text": "x"}));
        assert_eq!(e.unwrap_err().code, INVALID_PARAMS);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_methods_queue_effects() {
        let dir = workspace("effects");
        let mut core = Core::open(&dir).unwrap();
        let (ok, effects) = served(&mut core, &[], "ui/notify", json!({"message": "hi"}));
        assert_eq!(ok, Ok(Value::Null));
        assert_eq!(
            effects,
            [Effect::Notify {
                message: "hi".into()
            }]
        );
        let (_, effects) = served(&mut core, &[], "ui/status", json!({"text": "3 open"}));
        assert_eq!(
            effects,
            [Effect::Status {
                text: Some("3 open".into())
            }]
        );
        let (_, effects) = served(&mut core, &[], "command/run", json!({"id": "graph.open"}));
        assert_eq!(
            effects,
            [Effect::RunCommand {
                id: "graph.open".into(),
                args: Value::Null
            }]
        );
        let spec = json!({"program": "htop", "args": []});
        let (pane, effects) = served(&mut core, &[], "pane/open", spec);
        assert_eq!(pane, Ok(json!({"pane": 1})));
        assert!(matches!(&effects[..], [Effect::OpenPane { .. }]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn views_open_update_close_and_show_over_the_wire() {
        use crate::view::{Content, MAX_ROWS, ViewId};
        let dir = workspace("views");
        let mut core = Core::open(&dir).unwrap();
        let list = json!({"kind": "list", "title": "Open", "groups": []});
        let open = json!({"content": list, "select": "r1", "file": "Data/Eruptions.csv"});
        let (opened, effects) = served(&mut core, &[], "view/open", open);
        assert_eq!(opened, Ok(json!({"view": 1})), "no permission needed");
        let Content::List(sent) = serde_json::from_value(list.clone()).unwrap() else {
            unreachable!()
        };
        let content = Content::List(sent);
        assert_eq!(
            effects,
            [Effect::OpenView {
                view: ViewId(1),
                content: content.clone(),
                select: Some("r1".into()),
                file: Some("Data/Eruptions.csv".into()),
            }]
        );
        let update = json!({"view": 1, "content": list, "select": "r2"});
        let (updated, effects) = served(&mut core, &[], "view/update", update);
        assert_eq!(updated, Ok(Value::Null));
        assert_eq!(
            effects,
            [Effect::UpdateView {
                view: ViewId(1),
                content,
                select: Some("r2".into())
            }]
        );
        let (_, effects) = served(&mut core, &[], "view/close", json!({"view": 1}));
        assert_eq!(effects, [Effect::CloseView { view: ViewId(1) }]);
        let (_, effects) = served(&mut core, &[], "view/show", json!({"view": 1}));
        assert_eq!(effects, [Effect::ShowView { view: ViewId(1) }]);
        let open = json!({"note": "Plan.md", "line": 1});
        let (_, effects) = served(&mut core, &[], "workspace/open", open);
        let note = Effect::OpenNote {
            note: "Plan.md".into(),
            line: Some(1),
            item: None,
        };
        assert_eq!(effects, [note]);

        let rows: Vec<Value> = (0..MAX_ROWS)
            .map(|i| json!({"id": format!("r{i}"), "text": []}))
            .collect();
        let huge = json!({"content": {"kind": "list", "title": "Huge",
                          "groups": [{"id": "g", "title": "g", "rows": rows}]}});
        let (refused, effects) = served(&mut core, &[], "view/open", huge);
        let refused = refused.unwrap_err();
        assert_eq!((refused.code, reason(&refused)), (REFUSED, "refused"));
        assert!(effects.is_empty(), "nothing is shown");
        let (bad, _) = served(
            &mut core,
            &[],
            "view/open",
            json!({"content": {"kind": "map"}}),
        );
        assert_eq!(bad.unwrap_err().code, INVALID_PARAMS);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
