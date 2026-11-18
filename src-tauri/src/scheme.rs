//! The `clip` URI scheme: thumbnails and full images reach the webview as plain
//! URLs (`<img src>`), never as base64 over IPC (Section 3).
//!
//! * `thumb/<id>`: the item's WebP thumbnail, straight from RAM. Immediate.
//! * `blob/<id>`: the full PNG through the store's cache. May read and decrypt a
//!   file, so it runs on a worker thread and never blocks the UI thread.
//!
//! # URLs the frontend must use
//!
//! Tauri serves a custom scheme at a different origin per platform:
//!
//! | Platform | URL |
//! |---|---|
//! | Windows (WebView2), Android | `http://clip.localhost/thumb/<id>` |
//! | macOS, Linux | `clip://localhost/thumb/<id>` |
//!
//! (`blob/<id>` the same way.) `src/lib/ipc.ts` has `thumbUrl` and `blobUrl`
//! that build the right form. The parser also accepts the plan's literal
//! `clip://thumb/<id>` and a percent encoded slash (`thumb%2F12`, what
//! `convertFileSrc` produces), so a hand built URL still works.
//!
//! # Caching
//!
//! The content behind an id never changes, so responses carry
//! `Cache-Control: max-age=31536000, immutable`. Ids are not reused while the
//! app runs, but the next id is `max(id) + 1` after a restart, so deleting the
//! newest item and restarting hands its id to a different item. The frontend
//! therefore appends `?v=<createdAt>` (ignored here) to every URL: a reused id
//! has a different creation time, hence a different URL, hence no stale image.

use std::borrow::Cow;
use std::sync::Arc;

use tauri::http::{Method, Response, StatusCode, Uri, header};
use tauri::{Manager, Runtime};

use crate::model::ClipKind;
use crate::store::Store;

/// The scheme name (`clip://...`).
pub const SCHEME: &str = "clip";

/// One year, and `immutable` so the webview never revalidates.
pub const CACHE_CONTROL_IMMUTABLE: &str = "max-age=31536000, immutable";

pub type Reply = Response<Cow<'static, [u8]>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Thumb,
    Blob,
}

/// A parsed, valid request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipRequest {
    pub resource: Resource,
    pub id: u64,
    /// `HEAD`: same headers, no body.
    pub head: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// Not a `thumb/<id>` or `blob/<id>` URL, or the id is not a number.
    NotFound,
    /// Only GET and HEAD are served.
    MethodNotAllowed,
}

/// Split the URL into its resource and id. Pure, no store access.
///
/// Accepted shapes (query and fragment are ignored):
/// `http://clip.localhost/thumb/12`, `clip://localhost/thumb/12`,
/// `clip://thumb/12` and the same with `thumb%2F12`.
pub fn parse_request(method: &Method, uri: &Uri) -> Result<ClipRequest, RequestError> {
    let head = match *method {
        Method::GET => false,
        Method::HEAD => true,
        _ => return Err(RequestError::MethodNotAllowed),
    };
    // `convertFileSrc` encodes the slash of "thumb/12".
    let path = uri.path().replace("%2F", "/").replace("%2f", "/");
    let mut segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    // `clip://thumb/12` puts the resource in the host position.
    if matches!(uri.host(), Some("thumb" | "blob")) {
        segments.insert(0, uri.host().unwrap_or_default());
    }
    let [resource, id] = segments[..] else {
        return Err(RequestError::NotFound);
    };
    let resource = match resource {
        "thumb" => Resource::Thumb,
        "blob" => Resource::Blob,
        _ => return Err(RequestError::NotFound),
    };
    // `u64::from_str` would also take a leading `+`; ids are plain digits.
    if id.is_empty() || id.len() > 20 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(RequestError::NotFound);
    }
    let id = id.parse().map_err(|_| RequestError::NotFound)?;
    Ok(ClipRequest { resource, id, head })
}

/// Headers every response carries. `Access-Control-Allow-Origin: *` lets the
/// app page (`http://tauri.localhost`, `tauri://localhost` or the dev server) use
/// the images with `fetch` or a canvas. Only this app's own webview can reach
/// the scheme, so there is no outside origin to keep out.
fn base(status: StatusCode) -> tauri::http::response::Builder {
    Response::builder()
        .status(status)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header("Cross-Origin-Resource-Policy", "cross-origin")
}

fn text_reply(status: StatusCode, text: &'static str) -> Reply {
    base(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        // An error must not stick: the item may show up a moment later.
        .header(header::CACHE_CONTROL, "no-store")
        .body(Cow::Borrowed(text.as_bytes()))
        .expect("static response parts are valid")
}

/// The reply for a request that could not be parsed.
pub fn error_reply(e: RequestError) -> Reply {
    match e {
        RequestError::NotFound => text_reply(StatusCode::NOT_FOUND, "not found"),
        RequestError::MethodNotAllowed => {
            let mut r = text_reply(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
            r.headers_mut()
                .insert(header::ALLOW, "GET, HEAD".parse().expect("valid header"));
            r
        }
    }
}

/// The reply when the history failed to open at startup.
pub fn unavailable_reply() -> Reply {
    text_reply(StatusCode::SERVICE_UNAVAILABLE, "history unavailable")
}

fn image_reply(req: &ClipRequest, content_type: &'static str, bytes: Vec<u8>) -> Reply {
    let len = bytes.len();
    let body = if req.head { Vec::new() } else { bytes };
    base(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, len)
        .header(header::CACHE_CONTROL, CACHE_CONTROL_IMMUTABLE)
        .body(Cow::Owned(body))
        .expect("static response parts are valid")
}

/// Answer a valid request. Thumbnails never touch the disk; a blob may, so call
/// this for [`Resource::Blob`] off the UI thread (see [`register`]).
///
/// 404 for an unknown id, a thumbnail request for an item without one, or a blob
/// request for a non image. 500 when the PNG could not be read from disk.
pub fn serve(store: &Store, req: &ClipRequest) -> Reply {
    let Some(item) = store.get(req.id) else {
        return text_reply(StatusCode::NOT_FOUND, "not found");
    };
    match req.resource {
        Resource::Thumb => match &item.thumb {
            Some(t) => image_reply(req, "image/webp", t.to_vec()),
            None => text_reply(StatusCode::NOT_FOUND, "not found"),
        },
        Resource::Blob => {
            if item.kind != ClipKind::Image {
                return text_reply(StatusCode::NOT_FOUND, "not found");
            }
            match store.get_blob(req.id) {
                Ok(png) => image_reply(req, "image/png", png.to_vec()),
                Err(_) => text_reply(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not read the image",
                ),
            }
        }
    }
}

/// Register the `clip` scheme on the app builder.
pub fn register<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol(SCHEME, |ctx, request, responder| {
        let req = match parse_request(request.method(), request.uri()) {
            Ok(r) => r,
            Err(e) => return responder.respond(error_reply(e)),
        };
        let Some(store) = ctx
            .app_handle()
            .try_state::<Arc<Store>>()
            .map(|s| Arc::clone(s.inner()))
        else {
            return responder.respond(unavailable_reply());
        };
        match req.resource {
            // RAM only: answer right here.
            Resource::Thumb => responder.respond(serve(&store, &req)),
            // May read and decrypt a file: never on the UI thread.
            Resource::Blob => {
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(serve(&store, &req));
                });
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::{Fx, fake_files};

    fn get(uri: &str) -> Result<ClipRequest, RequestError> {
        parse_request(&Method::GET, &uri.parse().unwrap())
    }

    fn req(resource: Resource, id: u64) -> ClipRequest {
        ClipRequest {
            resource,
            id,
            head: false,
        }
    }

    #[test]
    fn parses_every_url_form() {
        let thumb = Ok(req(Resource::Thumb, 12));
        // Windows (WebView2) and macOS/Linux forms, and the plan's literal form.
        assert_eq!(get("http://clip.localhost/thumb/12"), thumb);
        assert_eq!(get("https://clip.localhost/thumb/12"), thumb);
        assert_eq!(get("clip://localhost/thumb/12"), thumb);
        assert_eq!(get("clip://thumb/12"), thumb);
        // `convertFileSrc` style, query and fragment.
        assert_eq!(get("clip://localhost/thumb%2F12"), thumb);
        assert_eq!(get("http://clip.localhost/thumb%2f12"), thumb);
        assert_eq!(get("http://clip.localhost/thumb/12?v=1700000000000"), thumb);
        assert_eq!(get("http://clip.localhost/thumb/12#x"), thumb);
        let blob = Ok(req(Resource::Blob, 7));
        assert_eq!(get("http://clip.localhost/blob/7"), blob);
        assert_eq!(get("clip://blob/7"), blob);
        assert_eq!(get("clip://blob/7?v=3"), blob);
        assert_eq!(
            get("clip://thumb/18446744073709551615"),
            Ok(req(Resource::Thumb, u64::MAX))
        );
    }

    #[test]
    fn rejects_bad_ids_and_shapes() {
        for bad in [
            "clip://localhost/thumb/",
            "clip://localhost/thumb",
            "clip://localhost/thumb/abc",
            "clip://localhost/thumb/-1",
            "clip://localhost/thumb/+1",
            "clip://localhost/thumb/1.5",
            "clip://localhost/thumb/0x10",
            "clip://localhost/thumb/12/",
            "clip://localhost/thumb/12/extra",
            "clip://localhost/thumb/18446744073709551616",
            "clip://localhost/thumb/000000000000000000001",
            "clip://localhost/other/12",
            "clip://localhost/Thumb/12",
            "clip://localhost/",
            "clip://localhost",
            "clip://other/12",
            "clip://thumb/",
            "clip://thumb",
            "http://clip.localhost/../thumb/12",
        ] {
            assert_eq!(get(bad), Err(RequestError::NotFound), "{bad}");
        }
    }

    #[test]
    fn only_get_and_head_are_served() {
        let uri: Uri = "clip://localhost/thumb/1".parse().unwrap();
        assert_eq!(
            parse_request(&Method::HEAD, &uri),
            Ok(ClipRequest {
                resource: Resource::Thumb,
                id: 1,
                head: true
            })
        );
        for m in [Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS] {
            assert_eq!(
                parse_request(&m, &uri),
                Err(RequestError::MethodNotAllowed),
                "{m}"
            );
        }
        let r = error_reply(RequestError::MethodNotAllowed);
        assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(r.headers()[header::ALLOW], "GET, HEAD");
    }

    fn header_of<'a>(r: &'a Reply, name: &str) -> &'a str {
        r.headers()[name].to_str().unwrap()
    }

    #[test]
    fn a_thumbnail_comes_from_ram_with_long_cache_headers() {
        let fx = Fx::new();
        let id = fx.image(1, 500);
        let r = serve(&fx.store, &req(Resource::Thumb, id));
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(header_of(&r, "content-type"), "image/webp");
        assert_eq!(
            header_of(&r, "cache-control"),
            "max-age=31536000, immutable"
        );
        assert_eq!(header_of(&r, "access-control-allow-origin"), "*");
        assert_eq!(header_of(&r, "x-content-type-options"), "nosniff");
        assert_eq!(header_of(&r, "content-length"), "300");
        assert_eq!(r.body().as_ref(), vec![1u8; 300].as_slice());
    }

    #[test]
    fn a_blob_is_the_full_png_through_the_cache() {
        let fx = Fx::new();
        let id = fx.image(2, 4096);
        fx.flush();
        let r = serve(&fx.store, &req(Resource::Blob, id));
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(header_of(&r, "content-type"), "image/png");
        assert_eq!(
            header_of(&r, "cache-control"),
            "max-age=31536000, immutable"
        );
        assert_eq!(header_of(&r, "access-control-allow-origin"), "*");
        assert_eq!(r.body().len(), 4096);
        assert_eq!(fx.store.cache_bytes(), 4096, "now cached in RAM");
    }

    #[test]
    fn unknown_ids_and_wrong_kinds_are_404_and_never_cached() {
        let fx = Fx::new();
        let text = fx.text("just text");
        let files = match fx.store.insert(fake_files(3, fx.now())) {
            crate::store::Inserted::New(id) | crate::store::Inserted::Bumped(id) => id,
        };
        for r in [
            serve(&fx.store, &req(Resource::Thumb, 999)),
            serve(&fx.store, &req(Resource::Blob, 999)),
            // A text has no thumbnail and no image.
            serve(&fx.store, &req(Resource::Thumb, text)),
            serve(&fx.store, &req(Resource::Blob, text)),
            // A file item with no thumbnail (odd n has none).
            serve(&fx.store, &req(Resource::Thumb, files)),
            serve(&fx.store, &req(Resource::Blob, files)),
        ] {
            assert_eq!(r.status(), StatusCode::NOT_FOUND);
            assert_eq!(header_of(&r, "cache-control"), "no-store");
            assert_eq!(header_of(&r, "access-control-allow-origin"), "*");
        }
    }

    #[test]
    fn a_file_item_with_a_thumbnail_serves_it() {
        let fx = Fx::new();
        let id = match fx.store.insert(fake_files(2, fx.now())) {
            crate::store::Inserted::New(id) | crate::store::Inserted::Bumped(id) => id,
        };
        let r = serve(&fx.store, &req(Resource::Thumb, id));
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.body().len(), 200);
    }

    #[test]
    fn a_blob_that_is_not_on_disk_is_a_500_not_a_panic() {
        let fx = Fx::new();
        let id = fx.image(4, 100);
        fx.flush();
        // Remove the file behind the item's back.
        let removed = std::fs::read_dir(fx.dir.path().join("blobs"))
            .unwrap()
            .flatten()
            .flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten())
            .map(|f| std::fs::remove_file(f.path()).unwrap())
            .count();
        assert_eq!(removed, 1);
        let r = serve(&fx.store, &req(Resource::Blob, id));
        assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(header_of(&r, "cache-control"), "no-store");
    }

    #[test]
    fn head_has_the_headers_and_an_empty_body() {
        let fx = Fx::new();
        let id = fx.image(5, 800);
        fx.flush();
        let head = ClipRequest {
            resource: Resource::Blob,
            id,
            head: true,
        };
        let r = serve(&fx.store, &head);
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(header_of(&r, "content-length"), "800");
        assert!(r.body().is_empty());
    }

    #[test]
    fn the_history_unavailable_reply_is_503() {
        assert_eq!(
            unavailable_reply().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
