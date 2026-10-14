use std::net::{IpAddr, SocketAddr};

use tokio::io::{self, AsyncWriteExt, Stdout};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::mpsc;

mod output;

use self::output::EventOutput;
use crate::ldap::{LdapEvent, LdapServer};

pub(crate) async fn run(ip: IpAddr, port: u16, raw: bool) -> io::Result<()> {
    let bind: SocketAddr = SocketAddr::new(ip, port);

    let listener: TcpListener = TcpListener::bind(bind).await?;
    let local_addr: SocketAddr = listener.local_addr()?;

    let (sender, receiver) = mpsc::channel(256);

    let server: LdapServer = LdapServer::new(listener, sender, raw);

    let mut stdout: Stdout = io::stdout();

    stdout
        .write_all(format!("Listening at ldap://{local_addr}/\nCtrl+C to stop.\n\n").as_bytes())
        .await?;
    stdout.flush().await?;

    let printer = print_events(receiver);

    tokio::pin!(printer);

    tokio::select! {
        result = server.run(signal::ctrl_c()) => {
            // The server has dropped its senders. Drain queued LDAP events
            // before returning, including when the listener failed.
            printer.await?;
            result?;
        }
        result = &mut printer => result?,
    }

    stdout.write_all(b"\nListener stopped.\n").await?;
    stdout.flush().await?;

    Ok(())
}

async fn print_events(mut receiver: mpsc::Receiver<LdapEvent>) -> io::Result<()> {
    let mut stdout: Stdout = io::stdout();

    while let Some(event) = receiver.recv().await {
        let text: String = EventOutput::new(&event).to_string();
        stdout.write_all(text.as_bytes()).await?;
        stdout.flush().await?;
    }

    Ok(())
}
