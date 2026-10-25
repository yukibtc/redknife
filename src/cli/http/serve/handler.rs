use std::io;
use std::net::SocketAddr;
use std::path::{Component, PathBuf};
use std::pin::Pin;

use bytes::Bytes;
use futures_util::TryStreamExt;
use http::{Method, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use tokio::fs;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

use crate::http::server::HttpServerHandler;

pub(super) struct Service {
    dir: PathBuf,
    upload_dir: Option<PathBuf>,
}

impl Service {
    #[inline]
    pub(super) fn new(dir: PathBuf, upload_dir: Option<PathBuf>) -> Self {
        Self { dir, upload_dir }
    }
}

impl HttpServerHandler for Service {
    fn handle_request<'a>(
        &'a self,
        req: Request<Incoming>,
        _client: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = Response<BoxBody<Bytes, io::Error>>> + Send + 'a>> {
        Box::pin(async move {
            tracing::info!("{} {}", req.method(), req.uri().path());

            match req.method() {
                &Method::GET => {
                    let path: &str = req.uri().path();
                    read_file(path, &self.dir).await
                }
                &Method::POST | &Method::PUT if req.uri().path() == "/upload" => {
                    let Some(upload_dir) = &self.upload_dir else {
                        return method_not_allowed();
                    };

                    let (parts, body) = req.into_parts();

                    let Some(filename) = parts.headers.get("X-Filename") else {
                        return bad_request("X-Filename header is missing");
                    };

                    let Ok(filename) = filename.to_str() else {
                        return bad_request("X-Filename header is not valid UTF-8");
                    };

                    let body: Bytes = match body.collect().await {
                        Ok(collected) => collected.to_bytes(),
                        Err(_) => return bad_request("upload body is missing"),
                    };

                    if body.is_empty() {
                        return bad_request("upload body is empty");
                    }

                    upload_file(&body, upload_dir, filename).await
                }
                _ => method_not_allowed(),
            }
        })
    }
}

async fn read_file(path: &str, dir: &PathBuf) -> Response<BoxBody<Bytes, io::Error>> {
    // Strip the first '/'
    let Some((_, path)) = path.split_once('/') else {
        return invalid_path();
    };

    if path.trim().is_empty() {
        return invalid_path();
    }

    let path: PathBuf = match resolve_path(dir, path) {
        Some(path) => path,
        None => return invalid_path(),
    };

    tracing::debug!("Reading file '{}'", path.display());

    if !path.is_file() {
        return not_found();
    }

    let Ok(file) = File::open(path.as_path()).await else {
        return not_found();
    };

    let reader_stream: ReaderStream<File> = ReaderStream::new(file);

    let stream_body = StreamBody::new(reader_stream.map_ok(Frame::data));
    let boxed_body = stream_body.boxed();

    Response::builder()
        .status(StatusCode::OK)
        .body(boxed_body)
        .expect("constant status won't error")
}

async fn upload_file(
    body: &Bytes,
    upload_dir: &PathBuf,
    filename: &str,
) -> Response<BoxBody<Bytes, io::Error>> {
    if let Err(e) = fs::create_dir_all(&upload_dir).await {
        tracing::error!("Failed to create upload directory: {}", e);
        return internal_error();
    }

    let Some(path) = resolve_path(upload_dir, filename) else {
        return invalid_path();
    };

    if path.is_file() {
        return conflict();
    }

    match fs::write(&path, body).await {
        Ok(()) => {
            tracing::info!("Uploaded file '{}'", path.display());
            created()
        }
        Err(e) => {
            tracing::error!("Failed to write file '{}': {}", path.display(), e);
            internal_error()
        }
    }
}

fn resolve_path(dir: &PathBuf, path: &str) -> Option<PathBuf> {
    let path: PathBuf = PathBuf::from(path);

    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }

    let path: PathBuf = dir.join(path);
    let path: PathBuf = path.components().collect();

    if !path.starts_with(dir) {
        return None;
    }

    Some(path)
}

fn invalid_path() -> Response<BoxBody<Bytes, io::Error>> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(
            Full::new(Bytes::from("Invalid path\n"))
                .map_err(|e| match e {})
                .boxed(),
        )
        .expect("constant status won't error")
}

fn bad_request(msg: &'static str) -> Response<BoxBody<Bytes, io::Error>> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Full::new(Bytes::from(msg)).map_err(|e| match e {}).boxed())
        .expect("constant status won't error")
}

fn created() -> Response<BoxBody<Bytes, io::Error>> {
    Response::builder()
        .status(StatusCode::CREATED)
        .body(
            Full::new(Bytes::from("Created\n"))
                .map_err(|e| match e {})
                .boxed(),
        )
        .expect("constant status won't error")
}

fn conflict() -> Response<BoxBody<Bytes, io::Error>> {
    Response::builder()
        .status(StatusCode::CONFLICT)
        .body(
            Full::new(Bytes::from("Conflict\n"))
                .map_err(|e| match e {})
                .boxed(),
        )
        .expect("constant status won't error")
}

fn not_found() -> Response<BoxBody<Bytes, io::Error>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(
            Full::new(Bytes::from("Not found\n"))
                .map_err(|e| match e {})
                .boxed(),
        )
        .expect("constant status won't error")
}

fn method_not_allowed() -> Response<BoxBody<Bytes, io::Error>> {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .body(
            Full::new(Bytes::from("Method not supported\n"))
                .map_err(|e| match e {})
                .boxed(),
        )
        .expect("constant status won't error")
}

fn internal_error() -> Response<BoxBody<Bytes, io::Error>> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(
            Full::new(Bytes::from("Something went wrong\n"))
                .map_err(|e| match e {})
                .boxed(),
        )
        .expect("constant status won't error")
}
