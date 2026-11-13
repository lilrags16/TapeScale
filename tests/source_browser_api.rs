use std::{fs, os::unix::fs::symlink};

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use serde_json::Value;
use tapescale::api::{self, AppState};
use tower::ServiceExt;

async fn get_json(application: &axum::Router, uri: &str) -> (axum::response::Response, Value) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should complete");
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body should be readable");
    let value = serde_json::from_slice(&body).expect("response should be JSON");
    let mut rebuilt = axum::response::Response::builder().status(status);
    *rebuilt
        .headers_mut()
        .expect("response headers should be available") = headers;
    (
        rebuilt
            .body(Body::empty())
            .expect("response should rebuild"),
        value,
    )
}

#[tokio::test]
async fn source_browser_route_is_bounded_contained_and_not_cached() {
    let source = tempfile::tempdir().expect("source root should exist");
    let restore = tempfile::tempdir().expect("restore root should exist");
    let web = tempfile::tempdir().expect("web root should exist");
    let outside = tempfile::tempdir().expect("outside root should exist");
    fs::create_dir_all(source.path().join("dataset/photos")).expect("source directories");
    fs::write(source.path().join("dataset/photo.raw"), b"raw").expect("source file");
    symlink(outside.path(), source.path().join("dataset/escape")).expect("escape link");
    let application = api::router(
        AppState::new(
            None,
            true,
            false,
            false,
            false,
            source.path().to_path_buf(),
            restore.path().to_path_buf(),
        ),
        web.path().to_path_buf(),
    );

    let (response, body) = get_json(&application, "/api/v1/source-browser?path=dataset").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL),
        Some(&header::HeaderValue::from_static("no-store"))
    );
    assert_eq!(body["current_path"], "dataset");
    assert_eq!(body["entries"][0]["name"], "photos");
    assert_eq!(body["entries"][0]["kind"], "directory");
    assert_eq!(body["entries"][1]["name"], "photo.raw");
    assert_eq!(body["entries"][1]["size_bytes"], 3);
    assert_eq!(body["entries"][2]["name"], "escape");
    assert_eq!(body["entries"][2]["kind"], "symlink");
    assert_eq!(body["entries"][2]["navigable"], false);
    assert!(body["entries"][0].get("uid").is_none());
    assert!(!body
        .to_string()
        .contains(source.path().to_string_lossy().as_ref()));

    let (traversal, _) = get_json(&application, "/api/v1/source-browser?path=%2E%2E").await;
    assert_eq!(traversal.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        traversal.headers().get(header::CACHE_CONTROL),
        Some(&header::HeaderValue::from_static("no-store"))
    );
    let (escape, _) = get_json(&application, "/api/v1/source-browser?path=dataset%2Fescape").await;
    assert_eq!(escape.status(), StatusCode::BAD_REQUEST);
}
