use std::io::ErrorKind::InvalidData;
use std::io::{Error, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, RwLock};

use bevy::log::debug;
use bevy::prelude::Component;
use crossbeam::channel::{Receiver, Sender, TryRecvError};
use packets::global::ModuleIdentification;

use crate::net::frame::SilkroadFrame;
use crate::net::security::SilkroadSecurityState;
use crate::net::{handshake, module_identification_frame};

/// Initial capacity for the inbound accumulator: a few 4 KB reads' worth, so
/// the common post-join burst is buffered without reallocating.
const READ_BUF_CAPACITY: usize = 4096 * 4;

#[derive(Component)]
#[allow(dead_code)]
pub struct SilkroadConnection {
    pub security: Arc<RwLock<SilkroadSecurityState>>,
    pub i_receiver: Receiver<SilkroadFrame>,
    pub i_sender: Sender<SilkroadFrame>,
    pub o_receiver: Receiver<SilkroadFrame>,
    pub o_sender: Sender<SilkroadFrame>,
    pub dc_receiver: Receiver<()>,
    pub dc_sender: Sender<()>,
    pub stream: Arc<Mutex<TcpStream>>,
    /// Inbound byte accumulator. Each TCP read is appended here and complete
    /// frames are drained off the front; a frame split across reads keeps its
    /// tail buffered instead of being discarded.
    pub read_buf: Vec<u8>,
}

impl SilkroadConnection {
    /// Blocking connect + handshake. Kept for callers that already run off the
    /// render loop (the headless net-check client, the agent-server handoff);
    /// the render loop uses [`Self::connect_async`] instead.
    pub fn new(addr: &str) -> Result<SilkroadConnection, Error> {
        Self::establish(Self::resolve_addr(addr)?)
    }

    /// Connects + handshakes on a dedicated worker thread so a slow or
    /// unreachable gateway never stalls a Bevy frame. Returns immediately with a
    /// [`PendingConnection`] the ECS can poll each frame; the established
    /// connection (or the error) is delivered over a channel. The blocking,
    /// `expect()`-heavy handshake runs inside `catch_unwind`, so a malformed
    /// handshake surfaces as an error instead of aborting the process.
    pub fn connect_async(addr: &str) -> PendingConnection {
        let (tx, rx) = crossbeam::channel::bounded(1);
        let addr = addr.to_string();
        // Dropping the JoinHandle detaches the worker; if the spawn itself fails
        // the closure (and `tx`) is dropped, which `PendingConnection::poll`
        // observes as a disconnect and reports as an error.
        let _ = std::thread::Builder::new()
            .name("sro-connect".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> Result<_, Error> { Self::establish(Self::resolve_addr(&addr)?) },
                ))
                .unwrap_or_else(|_| Err(Error::other("connection handshake panicked")));
                // The receiver is gone if the ECS abandoned the attempt; drop the
                // result rather than unwrapping.
                let _ = tx.send(result);
            });
        PendingConnection { rx }
    }

    /// Resolves a gateway address string to a `SocketAddr`, accepting both
    /// `ip:port` and `hostname:port` (the latter via a blocking DNS lookup).
    /// MUST only run off the render loop — both callers (`new` and the
    /// `connect_async` worker thread) satisfy that.
    fn resolve_addr(addr: &str) -> Result<SocketAddr, Error> {
        addr.to_socket_addrs()
            .map_err(|e| Error::new(InvalidData, format!("cannot resolve gateway '{addr}': {e}")))?
            .next()
            .ok_or_else(|| {
                Error::new(
                    InvalidData,
                    format!("gateway '{addr}' resolved to no addresses"),
                )
            })
    }

    /// Performs the full blocking connect: TCP connect, DH/blowfish handshake,
    /// module identification, and the switch to non-blocking mode that the
    /// receive loop relies on. On success the returned connection is ready to be
    /// polled by `receive_packets`.
    fn establish(addr: SocketAddr) -> Result<SilkroadConnection, Error> {
        let mut stream = TcpStream::connect(addr)?;
        debug!("connected to {}", addr);

        let security = Arc::new(RwLock::new(SilkroadSecurityState::new()));

        Self::do_handshake(&security, &mut stream)?;
        Self::send_module_identification(&security, &mut stream)?;

        // The receive/send loop expects WouldBlock instead of blocking reads.
        stream.set_nonblocking(true)?;

        let (i_sender, i_receiver) = crossbeam::channel::unbounded::<SilkroadFrame>();
        let (o_sender, o_receiver) = crossbeam::channel::unbounded::<SilkroadFrame>();
        let (dc_sender, dc_receiver) = crossbeam::channel::bounded(1);

        Ok(Self {
            read_buf: Vec::with_capacity(READ_BUF_CAPACITY),
            security,
            dc_sender,
            dc_receiver,
            i_sender,
            i_receiver,
            o_sender,
            o_receiver,
            stream: Arc::new(Mutex::new(stream)),
        })
    }

    fn do_handshake(
        security: &Arc<RwLock<SilkroadSecurityState>>,
        mut stream: &mut TcpStream,
    ) -> Result<(), Error> {
        match handshake::initialize(&mut stream, security.clone()) {
            Ok(new_security) => {
                let mut s = security.write().expect("security to be writable");
                s.context = new_security.context;
                s.state = new_security.state;
            }
            Err(e) => return Err(Error::new(InvalidData, e)),
        }
        debug!("finished handshake init");
        match handshake::finalize(&mut stream, security.clone()) {
            Ok(new_security) => {
                let mut s = security.write().expect("security to be writable");
                s.context = new_security.context;
                s.state = new_security.state;
            }
            Err(e) => return Err(Error::new(InvalidData, e)),
        }
        Ok(())
    }

    fn send_module_identification(
        security: &Arc<RwLock<SilkroadSecurityState>>,
        stream: &mut TcpStream,
    ) -> Result<(), Error> {
        let frame = module_identification_frame();
        let buf = frame.serialize(Arc::clone(security)).map_err(|_| {
            Error::new(
                InvalidData,
                "failed to serialize module identification frame",
            )
        })?;
        stream.write_all(&buf)?;

        let mut buf = [0; 4096];
        let read_bytes = stream.read(&mut buf)?;
        let (_, frame) = SilkroadFrame::parse(&mut buf[..read_bytes], Arc::clone(security))
            .map_err(|_| Error::new(InvalidData, "failed to parse module identification packet"))?;
        match frame {
            SilkroadFrame::Packet { opcode, data, .. } => {
                if opcode != 0x2001 {
                    return Err(Error::new(
                        InvalidData,
                        format!("invalid opcode: {}", opcode),
                    ));
                }
                // Typed parse rather than an inline read: a truncated body used
                // to index past the end of `data` and panic the connect thread.
                let ident = ModuleIdentification::try_from(data).map_err(|e| {
                    Error::new(
                        InvalidData,
                        format!("failed to parse module identification packet: {e}"),
                    )
                })?;
                debug!(
                    "module identified: {} ({:?})",
                    ident.module,
                    ident.peer_kind()
                );
                Ok(())
            }
            _ => Err(Error::new(
                InvalidData,
                "received unexpected massive packet",
            )),
        }
    }

    #[allow(dead_code)]
    pub fn get_receiver(&self) -> Receiver<SilkroadFrame> {
        self.i_receiver.clone()
    }

    pub fn get_sender(&self) -> Sender<SilkroadFrame> {
        self.o_sender.clone()
    }

    pub fn get_security(&self) -> Arc<RwLock<SilkroadSecurityState>> {
        self.security.clone()
    }

    pub fn get_dc_receiver(&self) -> Receiver<()> {
        self.dc_receiver.clone()
    }
}

/// Handle to an in-flight [`SilkroadConnection::connect_async`]. Lives as an ECS
/// component while a worker thread performs the blocking connect + handshake;
/// [`Self::poll`] is non-blocking, so the app keeps rendering until the
/// connection (or an error) is ready.
#[derive(Component)]
pub struct PendingConnection {
    rx: Receiver<Result<SilkroadConnection, Error>>,
}

impl PendingConnection {
    /// Non-blocking check for the connect result: `None` while still
    /// connecting, `Some(Ok)`/`Some(Err)` once the worker finished. A worker
    /// that vanished without delivering (e.g. a failed thread spawn) is reported
    /// as an error rather than leaving the caller polling forever.
    pub fn poll(&self) -> Option<Result<SilkroadConnection, Error>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(Error::other(
                "connect worker terminated before delivering a result",
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    /// Polls until a result arrives or the deadline passes, so the tests never
    /// hang if the worker misbehaves.
    fn poll_until_ready(
        pending: &PendingConnection,
        timeout: Duration,
    ) -> Option<Result<SilkroadConnection, Error>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(result) = pending.poll() {
                return Some(result);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn resolve_addr_accepts_literal_ip_port() {
        let addr = SilkroadConnection::resolve_addr("127.0.0.1:1").expect("ip:port resolves");
        assert_eq!(addr.port(), 1);
        assert!(addr.ip().is_loopback());
    }

    #[test]
    fn resolve_addr_rejects_malformed_without_dns() {
        // No `host:port` shape -> rejected during parsing; no DNS lookup, so this
        // stays offline and cannot block.
        assert!(SilkroadConnection::resolve_addr("not-an-addr").is_err());
    }

    #[test]
    fn connect_async_returns_immediately_and_polls_none_while_connecting() {
        // A listener that accepts but never speaks: the worker connects, then
        // blocks in the handshake read. The point is that `connect_async`
        // returned control instantly and the ECS-facing poll never blocks.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        let addr = listener.local_addr().expect("listener addr").to_string();

        let start = Instant::now();
        let pending = SilkroadConnection::connect_async(&addr);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "connect_async must not block the caller"
        );
        assert!(
            pending.poll().is_none(),
            "poll must report None while the handshake is still in flight"
        );
        // `listener` is kept alive until here so the worker stays blocked in its
        // read rather than seeing a connection reset.
        drop(listener);
    }

    #[test]
    fn connect_async_surfaces_invalid_address_as_error() {
        let pending = SilkroadConnection::connect_async("this is not a socket address");
        match poll_until_ready(&pending, Duration::from_secs(2)) {
            Some(Err(_)) => {}
            other => panic!(
                "expected an error result, got {:?}",
                other.map(|r| r.is_ok())
            ),
        }
    }
}
