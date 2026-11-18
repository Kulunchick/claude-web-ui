use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use claude_web_ui_server::fakes::{FakeCliRunner, FakeMessengerTransport};
use claude_web_ui_server::{GatewayConfig, GatewayServer};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

async fn gateway() -> (TempDir, GatewayServer) {
    let tmp = TempDir::new().expect("tempdir");
    let mut config = GatewayConfig::for_data_dir(&tmp.path().join("data"));
    config.jwt_secret = "test-jwt-secret-key-at-least-32-bytes-long".to_string();
    let server = GatewayServer::new(
        config,
        Arc::new(FakeCliRunner::new()),
        Arc::new(FakeMessengerTransport::new()),
    )
    .await
    .expect("gateway server init");
    (tmp, server)
}

async fn get(server: &GatewayServer, uri: &str) -> (StatusCode, String, Vec<u8>) {
    let resp = server
        .router()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, content_type, bytes)
}

/// The sidebar SYSTEM block shows the Gateway version without signing in again,
/// so the endpoint is public and reports the server crate version.
#[tokio::test]
async fn version_endpoint_reports_gateway_version_without_auth() {
    let (_tmp, server) = gateway().await;

    let (status, content_type, body) = get(&server, "/api/version").await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("application/json"), "content type {content_type}");
    let json: Value = serde_json::from_slice(&body).expect("version is JSON");
    assert_eq!(json, serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }));

    // The health probe used by deployments is unchanged.
    let (status, _, body) = get(&server, "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"ok");
}
