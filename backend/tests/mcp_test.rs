//! End-to-end tests for the MCP server.
//!
//! These drive a real MCP client against a real `PhosMcp` over an in-memory
//! duplex, so what is asserted is the protocol — the tool list a client would
//! actually see, the resource links a search actually emits, the image block a
//! model actually receives — rather than the Rust functions underneath. A test
//! that called `PhosMcp::search_shots` directly would still pass if the tool
//! were never registered.
//!
//! The library is built by the scanner from a real fixture photo, with no AI
//! pipeline: face detection is not what is under test here, and the tools have
//! to behave on a library where nothing has been clustered yet, because that is
//! what a fresh install looks like.

use std::collections::HashMap;
use std::sync::Arc;

use phos_backend::api::AppState;
use phos_backend::mcp::PhosMcp;
use rmcp::model::{CallToolRequestParams, ContentBlock, ReadResourceRequestParams};
use rmcp::service::{RunningService, ServiceExt};
use rmcp::RoleClient;
use serde_json::json;

/// A library holding one real photo, and an `AppState` over it.
fn fixture_library() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("keanu1.jpg");
    std::fs::copy(&fixture, root.join("keanu1.jpg")).expect("copy fixture");

    let db_path = root.join(".phos.db");
    phos_backend::db::init_and_migrate(&db_path).expect("migrate");

    // No AI pipeline: the scanner still hashes and records the file, which is
    // everything these tools read.
    let scanner = Arc::new(phos_backend::scanner::Scanner::new(db_path.clone(), None));
    scanner.scan(&root).expect("scan");

    let pool = phos_backend::db::establish_pool(&db_path).expect("pool");

    let state = AppState {
        pool,
        scanner,
        library_root: root,
        multi_user: false,
        user_pools: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        shutdown_flag: Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())),
        organizer: phos_backend::organizer::Organizer::new(),
        ingest: phos_backend::ingest::IngestQueue::new(),
    };

    (dir, state)
}

/// Connect a client to a server over an in-memory pipe.
async fn connect(state: AppState, writable: bool) -> RunningService<RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(1024 * 1024);
    tokio::spawn(async move {
        if let Ok(service) = PhosMcp::new(state, writable).serve(server_io).await {
            let _ = service.waiting().await;
        }
    });
    ().serve(client_io).await.expect("client handshake")
}

/// `with_arguments` wants a JSON object, not any JSON value.
fn args(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    value
        .as_object()
        .expect("arguments must be an object")
        .clone()
}

fn tool_names(tools: &[rmcp::model::Tool]) -> Vec<String> {
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    names
}

#[tokio::test]
async fn read_only_server_exposes_only_read_tools() {
    let (_dir, state) = fixture_library();
    let client = connect(state, false).await;

    let tools = client.list_tools(None).await.expect("list_tools");
    assert_eq!(
        tool_names(&tools.tools),
        vec![
            "get_image",
            "get_shot",
            "library_stats",
            "list_people",
            "person_timeline",
            "search_shots",
        ],
        "a read-only server must not advertise a verb it will not honour"
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn writable_server_adds_the_reviewer_verbs_and_no_deletes() {
    let (_dir, state) = fixture_library();
    let client = connect(state, true).await;

    let tools = client.list_tools(None).await.expect("list_tools");
    let names = tool_names(&tools.tools);

    for expected in [
        "assign_face",
        "confirm_shots",
        "merge_people",
        "rename_person",
        "trigger_scan",
    ] {
        assert!(names.contains(&expected.to_string()), "missing {expected}");
    }
    assert!(
        !names.iter().any(|n| n.contains("delete")),
        "v1 deliberately exposes no delete tool, found: {names:?}"
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn search_answers_with_resource_links_that_can_be_read() {
    let (_dir, state) = fixture_library();
    let client = connect(state, false).await;

    let result = client
        .call_tool(CallToolRequestParams::new("search_shots").with_arguments(args(json!({}))))
        .await
        .expect("search_shots");

    let uri = result
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::ResourceLink(resource) => Some(resource.uri.clone()),
            _ => None,
        })
        .expect("the scanned photo should come back as a resource link");
    assert!(uri.starts_with("phos://shot/"), "unexpected uri: {uri}");

    // The link is not decoration: reading it has to work.
    let read = client
        .read_resource(ReadResourceRequestParams::new(&uri))
        .await
        .expect("read_resource");
    let text = match read.contents.first().expect("contents") {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("a shot should read as text, got {other:?}"),
    };
    let shot: serde_json::Value = serde_json::from_str(&text).expect("shot json");
    assert!(
        shot["files"].as_array().is_some_and(|f| !f.is_empty()),
        "a shot record must list its files, or get_image has no id to use: {text}"
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn get_image_returns_an_image_block_for_a_file_id() {
    let (_dir, state) = fixture_library();
    let client = connect(state, false).await;

    // Walk the path a model walks: search, read the shot, take a file id.
    let search = client
        .call_tool(CallToolRequestParams::new("search_shots").with_arguments(args(json!({}))))
        .await
        .expect("search_shots");
    let uri = search
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::ResourceLink(resource) => Some(resource.uri.clone()),
            _ => None,
        })
        .expect("resource link");
    let shot_id = uri.trim_start_matches("phos://shot/").to_string();

    let detail = client
        .call_tool(
            CallToolRequestParams::new("get_shot")
                .with_arguments(args(json!({ "shot_id": shot_id }))),
        )
        .await
        .expect("get_shot");
    let ContentBlock::Text(text) = detail.content.first().expect("content") else {
        panic!("get_shot should answer with text");
    };
    let shot: serde_json::Value = serde_json::from_str(&text.text).expect("shot json");
    let file_id = shot["files"][0]["id"]
        .as_str()
        .expect("file id")
        .to_string();

    let image = client
        .call_tool(
            CallToolRequestParams::new("get_image")
                .with_arguments(args(json!({ "file_id": file_id, "width": 256 }))),
        )
        .await
        .expect("get_image");

    let block = image
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Image(image) => Some(image),
            _ => None,
        })
        .expect("get_image must return an image block, not a description of one");
    assert_eq!(block.mime_type, "image/jpeg");
    assert!(!block.data.is_empty());

    client.cancel().await.ok();
}

#[tokio::test]
async fn a_bad_id_is_an_invalid_argument_not_a_crash() {
    let (_dir, state) = fixture_library();
    let client = connect(state, false).await;

    let err = client
        .call_tool(
            CallToolRequestParams::new("get_shot")
                .with_arguments(args(json!({ "shot_id": "no-such-shot" }))),
        )
        .await;

    // Either shape is fine for a model — what matters is that it is told the id
    // is wrong rather than being handed a generic failure it will retry.
    match err {
        Err(e) => assert!(
            format!("{e}").contains("no such id"),
            "unhelpful error: {e}"
        ),
        Ok(result) => assert_eq!(
            result.is_error,
            Some(true),
            "a missing shot must not read as success"
        ),
    }

    client.cancel().await.ok();
}

#[tokio::test]
async fn the_three_resource_templates_are_advertised() {
    let (_dir, state) = fixture_library();
    let client = connect(state, false).await;

    let templates = client
        .list_resource_templates(None)
        .await
        .expect("list_resource_templates");
    let mut uris: Vec<String> = templates
        .resource_templates
        .iter()
        .map(|t| t.uri_template.clone())
        .collect();
    uris.sort();
    assert_eq!(
        uris,
        vec!["phos://file/{id}", "phos://person/{id}", "phos://shot/{id}"]
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn trigger_scan_refuses_a_path_outside_the_library() {
    let (_dir, state) = fixture_library();
    let library_root = state.library_root.clone();
    let client = connect(state, true).await;

    // A directory the server can see but this token's library does not contain.
    // Indexing it would write absolute paths into this database, which
    // `get_image` would then serve, and the scanner may delete a file out there
    // that hashes the same as one already indexed.
    let outsider = tempfile::tempdir().expect("tempdir");
    let result = client
        .call_tool(
            CallToolRequestParams::new("trigger_scan")
                .with_arguments(args(json!({ "path": outsider.path().to_string_lossy() }))),
        )
        .await;

    let rejected = match &result {
        Err(e) => format!("{e}").contains("inside this library"),
        Ok(result) => result.is_error == Some(true),
    };
    assert!(
        rejected,
        "an out-of-library scan must be refused: {result:?}"
    );

    // A `..` walk back out is the same request wearing a disguise.
    let escaped = library_root.join("..").to_string_lossy().to_string();
    let result = client
        .call_tool(
            CallToolRequestParams::new("trigger_scan")
                .with_arguments(args(json!({ "path": escaped }))),
        )
        .await;
    let rejected = match &result {
        Err(e) => format!("{e}").contains("inside this library"),
        Ok(result) => result.is_error == Some(true),
    };
    assert!(
        rejected,
        "`..` must not escape the library root: {result:?}"
    );

    // The library itself still scans.
    client
        .call_tool(
            CallToolRequestParams::new("trigger_scan")
                .with_arguments(args(json!({ "path": library_root.to_string_lossy() }))),
        )
        .await
        .expect("scanning the library root must still work");

    client.cancel().await.ok();
}
