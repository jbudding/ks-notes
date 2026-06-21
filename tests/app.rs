use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use ks_notes::config::Config;
use ks_notes::{build_state, routes};
use tower::ServiceExt;

fn test_app(dir: &tempfile::TempDir) -> Router {
    let config = Config {
        bind: "127.0.0.1".into(),
        port: 0,
        db_path: dir.path().join("test.db"),
        max_upload_mb: 4,
        secure_cookies: false,
    };
    let state = build_state(config).expect("state");
    routes::router(state)
}

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

fn form_req(path: &str, cookie: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::post(path).header(
        header::CONTENT_TYPE,
        "application/x-www-form-urlencoded",
    );
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

struct Session {
    cookie: String,
    csrf: String,
}

/// Register a user and return their session cookie + CSRF token.
async fn register(app: &Router, username: &str, password: &str) -> Option<Session> {
    let resp = app
        .clone()
        .oneshot(form_req(
            "/register",
            None,
            &format!(
                "username={username}&password={password}&password_confirm={password}"
            ),
        ))
        .await
        .unwrap();
    if resp.status() != StatusCode::SEE_OTHER {
        return None;
    }
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)?
        .to_str()
        .ok()?
        .to_string();
    let cookie = set_cookie.split(';').next()?.to_string();

    // Pull the CSRF token off the home page.
    let home = app
        .clone()
        .oneshot(
            Request::get("/")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_text(home).await;
    let csrf = html
        .split("X-CSRF-Token\": \"")
        .nth(1)?
        .split('"')
        .next()?
        .to_string();
    Some(Session { cookie, csrf })
}

async fn create_memo(app: &Router, s: &Session, content: &str, visibility: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/memos")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, &s.cookie)
                .header("X-CSRF-Token", &s.csrf)
                .body(Body::from(format!(
                    "content={}&visibility={visibility}",
                    urlencode(content)
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "memo create failed");
    body_text(resp).await
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn extract_uid(card_html: &str) -> String {
    card_html
        .split("data-copy=\"/m/")
        .nth(1)
        .map(|s| s.split('"').next().unwrap().to_string())
        .expect("uid in card")
}

#[tokio::test]
async fn first_user_is_admin_and_registration_gates() {
    let dir = tempfile::tempdir().unwrap();
    let app = test_app(&dir);

    let s1 = register(&app, "alice", "password123").await.expect("first user");
    // Admin page accessible -> alice is admin.
    let admin = app
        .clone()
        .oneshot(
            Request::get("/admin")
                .header(header::COOKIE, &s1.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin.status(), StatusCode::OK);

    // Registration is closed by default after the first user.
    assert!(register(&app, "bob", "password123").await.is_none());

    // Admin opens registration; bob can join but is NOT admin.
    let resp = app
        .clone()
        .oneshot(form_req(
            "/admin/registration",
            Some(&s1.cookie),
            &format!("csrf_token={}&enabled=true", s1.csrf),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let s2 = register(&app, "bob", "password123").await.expect("bob");
    let bob_admin = app
        .clone()
        .oneshot(
            Request::get("/admin")
                .header(header::COOKIE, &s2.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bob_admin.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn csrf_is_enforced_and_markdown_renders() {
    let dir = tempfile::tempdir().unwrap();
    let app = test_app(&dir);
    let s = register(&app, "alice", "password123").await.unwrap();

    // Without the CSRF header: rejected.
    let resp = app
        .clone()
        .oneshot(form_req("/memos", Some(&s.cookie), "content=nope"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // With it: card HTML with rendered markdown, raw HTML escaped.
    let card = create_memo(
        &app,
        &s,
        "hello **bold** <script>alert(1)</script> #greeting",
        "private",
    )
    .await;
    assert!(card.contains("<strong>bold</strong>"));
    assert!(!card.contains("<script>alert(1)</script>"));
    assert!(card.contains("memo-card"));
}

#[tokio::test]
async fn search_and_tag_counts() {
    let dir = tempfile::tempdir().unwrap();
    let app = test_app(&dir);
    let s = register(&app, "alice", "password123").await.unwrap();

    create_memo(&app, &s, "grocery list bananas #errands", "private").await;
    create_memo(&app, &s, "rust borrow checker notes #dev", "private").await;
    create_memo(&app, &s, "more rust async notes #dev", "private").await;

    // FTS search via the HTMX fragment path.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/?q=bananas")
                .header(header::COOKIE, &s.cookie)
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_text(resp).await;
    assert!(html.contains("grocery list"));
    assert!(!html.contains("borrow checker"));

    // Tag counts on the full page sidebar: dev ×2, errands ×1.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/")
                .header(header::COOKIE, &s.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_text(resp).await;
    assert!(html.contains("#dev"));
    assert!(html.contains("<span class=\"count\">2</span>"));
    assert!(html.contains("#errands"));
}

#[tokio::test]
async fn share_page_visibility_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let app = test_app(&dir);
    let owner = register(&app, "alice", "password123").await.unwrap();
    // Open registration for a second user.
    app.clone()
        .oneshot(form_req(
            "/admin/registration",
            Some(&owner.cookie),
            &format!("csrf_token={}&enabled=true", owner.csrf),
        ))
        .await
        .unwrap();
    let other = register(&app, "bob", "password123").await.unwrap();

    let mut uids = std::collections::HashMap::new();
    for vis in ["private", "protected", "public"] {
        let card = create_memo(&app, &owner, &format!("{vis} memo body"), vis).await;
        uids.insert(vis, extract_uid(&card));
    }

    let fetch = |cookie: Option<String>, uid: String| {
        let app = app.clone();
        async move {
            let mut b = Request::get(format!("/m/{uid}"));
            if let Some(c) = cookie {
                b = b.header(header::COOKIE, c);
            }
            app.oneshot(b.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status()
        }
    };

    // anon: only public
    assert_eq!(fetch(None, uids["public"].clone()).await, StatusCode::OK);
    assert_eq!(fetch(None, uids["protected"].clone()).await, StatusCode::NOT_FOUND);
    assert_eq!(fetch(None, uids["private"].clone()).await, StatusCode::NOT_FOUND);
    // other signed-in user: public + protected
    let bc = Some(other.cookie.clone());
    assert_eq!(fetch(bc.clone(), uids["public"].clone()).await, StatusCode::OK);
    assert_eq!(fetch(bc.clone(), uids["protected"].clone()).await, StatusCode::OK);
    assert_eq!(fetch(bc, uids["private"].clone()).await, StatusCode::NOT_FOUND);
    // owner: everything
    let oc = Some(owner.cookie.clone());
    assert_eq!(fetch(oc.clone(), uids["private"].clone()).await, StatusCode::OK);
}

#[tokio::test]
async fn api_crud_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let app = test_app(&dir);
    let s = register(&app, "alice", "password123").await.unwrap();

    // Mint a token through the settings UI.
    let resp = app
        .clone()
        .oneshot(form_req(
            "/settings/tokens",
            Some(&s.cookie),
            &format!("csrf_token={}&name=test", s.csrf),
        ))
        .await
        .unwrap();
    let html = body_text(resp).await;
    let token = html
        .split("token-reveal\">")
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .to_string();
    assert!(token.starts_with("ksn_"));

    // Bad token -> 401.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/api/v1/me")
                .header(header::AUTHORIZATION, "Bearer ksn_bogus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Create -> get -> patch -> delete.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/v1/memos")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"content": "api memo #fromapi"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_str(&body_text(resp).await).unwrap();
    let uid = created["uid"].as_str().unwrap().to_string();
    assert_eq!(created["visibility"], "private");

    let resp = app
        .clone()
        .oneshot(
            Request::patch(format!("/api/v1/memos/{uid}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"pinned": true, "visibility": "public"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let patched: serde_json::Value =
        serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(patched["pinned"], true);
    assert_eq!(patched["visibility"], "public");

    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/v1/memos/{uid}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/memos/{uid}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn anonymous_is_redirected_to_login() {
    let dir = tempfile::tempdir().unwrap();
    let app = test_app(&dir);
    // No users at all: / -> /login -> /register chain starts at /login.
    let resp = app
        .clone()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()[header::LOCATION], "/login");
}

/// Build a single-file multipart upload body for the `/resources` route.
fn upload_req(s: &Session, size: usize) -> Request<Body> {
    let boundary = "TESTBOUNDARY";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"big.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&vec![0u8; size]);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    Request::post("/resources")
        .header(header::COOKIE, &s.cookie)
        .header("X-CSRF-Token", &s.csrf)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        // Browsers send Content-Length; with it the cap is enforced up-front
        // (413) rather than mid-stream.
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .unwrap()
}

// Uploads larger than axum's 2 MiB DefaultBodyLimit must still succeed up to the
// configured cap — Multipart enforces that limit per field, so the router raises
// it to match max_upload_mb. Regression test for the 2 MiB silent ceiling.
#[tokio::test]
async fn upload_above_default_body_limit() {
    let dir = tempfile::tempdir().unwrap();
    let app = test_app(&dir); // max_upload_mb: 4
    let s = register(&app, "bob", "password123").await.unwrap();

    // 3 MiB: over axum's 2 MiB default, under the 4 MB cap.
    let resp = app
        .clone()
        .oneshot(upload_req(&s, 3 * 1024 * 1024))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "3 MiB upload should succeed");

    // 5 MiB: over the 4 MB cap, must be rejected by RequestBodyLimitLayer.
    let resp = app
        .clone()
        .oneshot(upload_req(&s, 5 * 1024 * 1024))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "upload over the cap should be rejected"
    );
}
