use std::io;
use std::path::{Component, PathBuf};

use bytes::Bytes;
use futures_util::TryStreamExt;
use http::{Method, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::fs;
use tokio::fs::File;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio_util::io::ReaderStream;

type ServerBuilder = hyper::server::conn::http1::Builder;

pub struct HttpServer {
    listener: TcpListener,
    dir: PathBuf,
    upload_dir: Option<PathBuf>,
}

impl HttpServer {
    #[inline]
    pub fn new(listener: TcpListener, dir: PathBuf, upload_dir: Option<PathBuf>) -> Self {
        Self {
            listener,
            dir,
            upload_dir,
        }
    }

    pub async fn run<F>(self, shutdown: F) -> io::Result<()>
    where
        F: Future<Output = io::Result<()>>,
    {
        let mut connections: JoinSet<()> = JoinSet::new();

        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                biased;
                result = &mut shutdown => {
                    result?;
                    break;
                }
                result = self.listener.accept() => {
                    let (stream, _) = result?;
                    let io: TokioIo<TcpStream> = TokioIo::new(stream);
                    let dir: PathBuf = self.dir.clone();
                    let upload_dir: Option<PathBuf> = self.upload_dir.clone();

                    connections.spawn(async move {
                        let service = service_fn(move |req| {
                            let dir: PathBuf = dir.clone();
                            let upload_dir: Option<PathBuf> = upload_dir.clone();
                            async move { response(req, dir, upload_dir).await }
                        });

                        let _ = ServerBuilder::new()
                            .preserve_header_case(true)
                            .title_case_headers(true)
                            .serve_connection(io, service)
                            .with_upgrades()
                            .await;
                    });
                }

                Some(result) = connections.join_next(), if !connections.is_empty() => {
                    if let Err(error) = result {
                        tracing::error!("Connection task error: {error}");
                    }
                }
            }
        }

        connections.shutdown().await;

        Ok(())
    }
}

async fn response(
    req: Request<Incoming>,
    dir: PathBuf,
    upload_dir: Option<PathBuf>,
) -> Result<Response<BoxBody<Bytes, io::Error>>, hyper::Error> {
    tracing::info!("{} {}", req.method(), req.uri().path());

    match req.method() {
        &Method::GET => {
            let path: &str = req.uri().path();
            read_file(path, dir).await
        }
        &Method::POST | &Method::PUT if req.uri().path() == "/upload" => {
            let Some(upload_dir) = upload_dir else {
                return Ok(method_not_allowed());
            };

            let (parts, body) = req.into_parts();

            let Some(filename) = parts.headers.get("X-Filename") else {
                return Ok(bad_request("X-Filename header is missing"));
            };

            let Ok(filename) = filename.to_str() else {
                return Ok(bad_request("X-Filename header is not valid UTF-8"));
            };

            let body: Bytes = match body.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(_) => return Ok(bad_request("upload body is missing")),
            };

            if body.is_empty() {
                return Ok(bad_request("upload body is empty"));
            }

            upload_file(&body, upload_dir, filename).await
        }
        _ => Ok(method_not_allowed()),
    }
}

async fn read_file(
    path: &str,
    dir: PathBuf,
) -> Result<Response<BoxBody<Bytes, io::Error>>, hyper::Error> {
    // Strip the first '/'
    let Some((_, path)) = path.split_once('/') else {
        return Ok(invalid_path());
    };

    if path.trim().is_empty() {
        return Ok(invalid_path());
    }

    let path: PathBuf = match resolve_path(dir, path) {
        Some(path) => path,
        None => return Ok(invalid_path()),
    };

    tracing::debug!("Reading file '{}'", path.display());

    if !path.is_file() {
        return Ok(not_found());
    }

    let Ok(file) = File::open(path.as_path()).await else {
        return Ok(not_found());
    };

    let reader_stream: ReaderStream<File> = ReaderStream::new(file);

    let stream_body = StreamBody::new(reader_stream.map_ok(Frame::data));
    let boxed_body = stream_body.boxed();

    let response = Response::builder()
        .status(StatusCode::OK)
        .body(boxed_body)
        .expect("constant status won't error");

    Ok(response)
}

async fn upload_file(
    body: &Bytes,
    upload_dir: PathBuf,
    filename: &str,
) -> Result<Response<BoxBody<Bytes, io::Error>>, hyper::Error> {
    if let Err(e) = fs::create_dir_all(&upload_dir).await {
        tracing::error!("Failed to create upload directory: {}", e);
        return Ok(internal_error());
    }

    let Some(path) = resolve_path(upload_dir, filename) else {
        return Ok(invalid_path());
    };

    if path.is_file() {
        return Ok(conflict());
    }

    match fs::write(&path, body).await {
        Ok(()) => {
            tracing::info!("Uploaded file '{}'", path.display());
            Ok(created())
        }
        Err(e) => {
            tracing::error!("Failed to write file '{}': {}", path.display(), e);
            Ok(internal_error())
        }
    }
}

fn resolve_path(dir: PathBuf, path: &str) -> Option<PathBuf> {
    let path: PathBuf = PathBuf::from(path);

    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }

    let path: PathBuf = dir.join(path);
    let path: PathBuf = path.components().collect();

    if !path.starts_with(&dir) {
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
