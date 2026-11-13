//! Browser API CORS, mirroring Effect's `HttpMiddleware.cors` as configured by
//! `apps/server/src/http.ts` and `httpCors.ts`: every `OPTIONS` request is answered with 204 and
//! the preflight headers, and every other response (failures included) carries the origin headers.
use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::Response,
};

const ALLOWED_METHODS: &str = "GET, POST, OPTIONS";
const ALLOWED_HEADERS: &str =
    "authorization,b3,traceparent,content-type,dpop,x-t3-orchestration-protocol";
#[cfg(feature = "local-services")]
const DESKTOP_RENDERER_ORIGINS: [&str; 2] = ["t3code://app", "t3code-dev://app"];

#[derive(Clone)]
pub struct Cors {
    /// `None` is the production mode: wildcard origin, no credentials.
    dev_origins: Option<Vec<String>>,
}
impl Cors {
    #[cfg(not(feature = "local-services"))]
    pub fn production() -> Self {
        Self { dev_origins: None }
    }
    #[cfg(feature = "local-services")]
    pub fn new(dev_origin: Option<String>, dev_allowed_origins: &[String]) -> Self {
        Self {
            dev_origins: dev_origin.map(|origin| {
                let mut list = vec![origin];
                list.extend(DESKTOP_RENDERER_ORIGINS.map(String::from));
                list.extend(dev_allowed_origins.iter().cloned());
                list
            }),
        }
    }
    fn origin_headers(&self, request: &HeaderMap, out: &mut HeaderMap) {
        let Some(allowed) = &self.dev_origins else {
            out.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
            return;
        };
        out.append(header::VARY, HeaderValue::from_static("Origin"));
        if let Some(origin) = request.get(header::ORIGIN)
            && origin
                .to_str()
                .is_ok_and(|o| allowed.iter().any(|a| a == o))
        {
            out.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
        }
        out.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }
}

pub async fn layer(
    axum::extract::State(cors): axum::extract::State<Cors>,
    request: Request,
    next: Next,
) -> Response {
    let mut extra = HeaderMap::new();
    cors.origin_headers(request.headers(), &mut extra);
    if request.method() == Method::OPTIONS {
        let mut response = Response::new(axum::body::Body::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        let headers = response.headers_mut();
        for (name, value) in extra {
            if let Some(name) = name {
                headers.append(name, value);
            }
        }
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(ALLOWED_METHODS),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(ALLOWED_HEADERS),
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        return response;
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (name, value) in extra {
        let Some(name) = name else { continue };
        if name == header::VARY {
            headers.append(name, value);
        } else {
            headers.insert(name, value);
        }
    }
    response
}

#[cfg(all(test, feature = "local-services"))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};

    async fn serve(cors: Cors) -> String {
        let app = Router::new()
            .route("/ok", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(cors, layer));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn dev_mode_reflects_only_listed_origins_with_credentials() {
        let base = serve(Cors::new(
            Some("http://localhost:5733".into()),
            &["https://extra.example".into()],
        ))
        .await;
        let http = reqwest::Client::new();
        for (origin, allowed) in [
            ("http://localhost:5733", true),
            ("t3code://app", true),
            ("t3code-dev://app", true),
            ("https://extra.example", true),
            ("https://evil.example", false),
        ] {
            let response = http
                .get(format!("{base}/ok"))
                .header("origin", origin)
                .send()
                .await
                .unwrap();
            let headers = response.headers();
            assert_eq!(headers["vary"], "Origin", "{origin}");
            assert_eq!(headers["access-control-allow-credentials"], "true");
            match headers.get("access-control-allow-origin") {
                Some(value) => assert!(allowed && value == origin, "{origin}"),
                None => assert!(!allowed, "{origin}"),
            }
        }
        let preflight = http
            .request(reqwest::Method::OPTIONS, format!("{base}/ok"))
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(preflight.status(), 204);
        assert!(
            preflight
                .headers()
                .get("access-control-allow-origin")
                .is_none()
        );
        assert_eq!(preflight.headers()["access-control-max-age"], "600");
    }
}
