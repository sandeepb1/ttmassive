//! A stand-in web service for the provider tests: answers each path with a set status, headers and body, and notes
//! the key and body it was sent.

use std::sync::{Arc, Mutex};

use axum::{Router, extract::State, http::{HeaderMap, StatusCode}, routing::post};
use serde_json::Value;

#[derive(Default)]
pub struct Seen {
    pub auth: Mutex<Vec<String>>,
    pub bodies: Mutex<Vec<(String, Value)>>,
}

/// A stand-in service answering each path with the given status, headers and body, and noting what it was sent.
pub async fn serve(routes: Vec<(&'static str, u16, Vec<(&'static str, &'static str)>, Value)>) -> (String, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let mut app = Router::new();
    for (path, status, headers, body) in routes {
        let handler = move |State(s): State<Arc<Seen>>, h: HeaderMap, req: Option<axum::Json<Value>>| {
            let (headers, body) = (headers.clone(), body.clone());
            async move {
                s.auth.lock().unwrap().push(h.get("authorization").or_else(|| h.get("x-api-key")).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned());
                s.bodies.lock().unwrap().push((path.to_owned(), req.map(|r| r.0).unwrap_or(Value::Null)));
                let mut map = HeaderMap::new();
                for (k, v) in headers {
                    map.insert(k, v.parse().unwrap());
                }
                (StatusCode::from_u16(status).unwrap(), map, axum::Json(body))
            }
        };
        app = app.route(path, post(handler.clone()).get(handler));
    }
    let app = app.with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen)
}

pub fn ok(path: &'static str, body: Value) -> (&'static str, u16, Vec<(&'static str, &'static str)>, Value) {
    (path, 200, vec![], body)
}
