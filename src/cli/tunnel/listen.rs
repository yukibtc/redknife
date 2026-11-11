use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use bytes::Bytes;
use rand::Rng;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::{fs, signal};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};

use super::Forward;
use crate::cli::redknife_home_dir;
use crate::dir::{create_private_dir, write_secret};
use crate::logger;
use crate::socks::SocksListener;
use crate::tunnel::{TunnelListener, TunnelTlsCerts, Waiting};

pub(super) async fn run(
    addr: SocketAddr,
    auth: Option<String>,
    socks: SocketAddr,
    no_socks: bool,
    forwards: Vec<Forward>,
    reverse_ports_whitelist: HashSet<u16>,
    data_dir: Option<PathBuf>,
) -> io::Result<()> {
    logger::init()?;

    let state_dir: PathBuf = match data_dir {
        Some(data_dir) => data_dir,
        None => redknife_home_dir()?.join("tunnel"),
    };

    let certs: TunnelTlsCerts = load_or_generate_certs(&state_dir).await?;

    let mut tasks: JoinSet<()> = JoinSet::new();

    // Local clients waiting to reach a target through the tunnel.
    let (pending_tx, pending_rx) = mpsc::channel(128);

    // SOCKS5 proxy, the destination is chosen by the connecting client.
    if !no_socks {
        let listener: SocksListener = SocksListener::bind(socks).await?;
        let socks_addr: SocketAddr = listener.local_addr()?;

        tracing::info!("SOCKS5 proxy on {socks_addr}");

        let pending_tx: mpsc::Sender<Waiting> = pending_tx.clone();

        tasks.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, target)) => {
                        let waiting: Waiting = Waiting { stream, target };
                        if pending_tx.send(waiting).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => tracing::debug!("SOCKS client rejected: {error}"),
                }
            }
        });
    }

    // Static forwards, the destination is fixed at startup, for tools that cannot speak SOCKS.
    for forward in forwards {
        let listener: TcpListener = TcpListener::bind(forward.bind).await?;

        tracing::info!("Forwarding {} -> {}", forward.bind, forward.target);

        let pending_tx: mpsc::Sender<Waiting> = pending_tx.clone();

        tasks.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let waiting: Waiting = Waiting {
                            stream,
                            target: forward.target.clone(),
                        };
                        if pending_tx.send(waiting).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => tracing::debug!("Forward connection failed: {error}"),
                }
            }
        });
    }

    let listener: TcpListener = TcpListener::bind(addr).await?;

    tracing::info!("Tunnel listening on {addr}");
    tracing::info!("Certificate fingerprint: {}", certs.fingerprint());

    let auth: String = auth.unwrap_or_else(|| {
        let mut rng = UnwrapErr(SysRng);
        let mut bytes: [u8; 32] = [0; 32];
        rng.fill_bytes(&mut bytes);
        let encoded: String = STANDARD_NO_PAD.encode(bytes);
        tracing::info!("Auth secret: {encoded}");
        encoded
    });
    let token: Bytes = Bytes::from(auth);

    let tunnel: TunnelListener =
        TunnelListener::new(token, &certs, reverse_ports_whitelist, pending_rx)?;

    tokio::select! {
        result = tunnel.run(listener) => result,
        _ = signal::ctrl_c() => Ok(()),
    }
}

async fn load_or_generate_certs(dir: &Path) -> io::Result<TunnelTlsCerts> {
    create_private_dir(dir).await?;

    let cert_path: PathBuf = dir.join("cert.der");
    let key_path: PathBuf = dir.join("key.der");

    let output = tokio::try_join!(fs::read(&cert_path), fs::read(&key_path));

    if let Ok((cert_bytes, key_bytes)) = output {
        tracing::info!("Loaded tunnel certs from {}", dir.display());

        let cert: CertificateDer<'static> = CertificateDer::from(cert_bytes);
        let key: PrivatePkcs8KeyDer<'static> = PrivatePkcs8KeyDer::from(key_bytes);

        return Ok(TunnelTlsCerts::new(cert, key));
    }

    let certs: TunnelTlsCerts = TunnelTlsCerts::generate();

    let cert_bytes: Vec<u8> = certs.cert().to_vec();
    let key_bytes: &[u8] = certs.key().secret_pkcs8_der();

    write_secret(&cert_path, &cert_bytes).await?;
    write_secret(&key_path, key_bytes).await?;

    tracing::info!("Generated new tunnel certs in {}", dir.display());

    Ok(certs)
}
