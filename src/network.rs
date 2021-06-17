use std::{
    io::{ErrorKind, Read, Result, Write},
    net::{Shutdown, SocketAddr},
    time::{Duration, Instant},
};

use crossbeam::channel::{Receiver, Sender};

#[cfg(feature = "bounded")]
use crossbeam::channel::bounded as channel;
#[cfg(not(feature = "bounded"))]
use crossbeam::channel::unbounded as channel;

pub trait HeaderFn = Fn(&[u8]) -> Header;

use mio::{
    event::Event,
    net::{TcpListener, TcpStream},
    Events, Interest, Poll, Registry, Token, Waker,
};
use slab::Slab;
use specs::{Entity, RunNow, World};
use std::sync::Arc;

#[derive(Clone)]
pub enum RequestIdent {
    Entity(Entity),
    Token(Token),
}

impl RequestIdent {
    pub fn token(self) -> Token {
        match self {
            RequestIdent::Entity(_) => panic!("entity stored instead of token"),
            RequestIdent::Token(token) => token,
        }
    }

    pub fn entity(self) -> Entity {
        match self {
            RequestIdent::Entity(entity) => entity,
            RequestIdent::Token(_) => panic!("token stored instead of entity"),
        }
    }

    pub fn replace_entity(&mut self, entity: Entity) {
        *self = RequestIdent::Entity(entity);
    }

    pub fn replace_token(&mut self, token: Token) {
        *self = RequestIdent::Token(token);
    }

    pub fn is_entity(&self) -> bool {
        if let RequestIdent::Entity(_) = self {
            true
        } else {
            false
        }
    }

    pub fn is_token(&self) -> bool {
        !self.is_entity()
    }
}

#[derive(Default, Clone, Debug)]
pub struct Header {
    pub length: usize,
    pub cmd: u32,
    //TODO more flags for expand Header
}

impl Header {
    pub fn empty(&self) -> bool {
        self.cmd == 0
    }

    pub fn clear(&mut self) {
        self.cmd = 0;
        self.length = 0;
    }
}

struct Connection<T, const N: usize>
where
    T: HeaderFn,
{
    stream: TcpStream,
    tag: String,
    token: Token,
    interests: Interest,
    read_bytes: Vec<u8>,
    write_bytes: Vec<u8>,
    last_time: Instant,
    decoder: T,
    header: Header,
    sender: Sender<NetworkInputData>,
    ident: RequestIdent,
}

impl<T, const N: usize> Connection<T, N>
where
    T: HeaderFn,
{
    pub fn new(
        stream: TcpStream,
        addr: SocketAddr,
        decoder: T,
        sender: Sender<NetworkInputData>,
    ) -> Self {
        let tag = addr.to_string();
        Self {
            stream,
            tag,
            token: Token(0),
            interests: Interest::READABLE,
            read_bytes: Vec::with_capacity(1024),
            write_bytes: Vec::with_capacity(1024),
            last_time: Instant::now(),
            decoder,
            header: Header::default(),
            sender,
            ident: RequestIdent::Token(Token(0)),
        }
    }

    fn setup(&mut self, registry: &Registry) {
        if let Err(err) = registry.register(&mut self.stream, self.token, self.interests) {
            log::error!("[{}]connection register failed:{}", self.tag, err);
        }
    }

    fn reregister(&mut self, registry: &Registry) {
        let mut modified = false;
        if !self.write_bytes.is_empty() && !self.interests.is_writable() {
            modified = true;
            self.interests |= Interest::WRITABLE;
        } else if self.write_bytes.is_empty() && self.interests.is_writable() {
            modified = true;
            self.interests = Interest::READABLE;
        }
        if modified {
            if let Err(err) = registry.reregister(&mut self.stream, self.token, self.interests) {
                log::error!("[{}]reregister failed {}", self.tag, err);
            }
        }
    }

    fn send(&mut self, mut data: &[u8]) {
        if !self.write_bytes.is_empty() {
            self.write_bytes.extend_from_slice(data);
            return;
        }

        self.last_time = Instant::now();
        loop {
            match self.stream.write(data) {
                Ok(size) => data = &data[size..],
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    self.write_bytes.extend_from_slice(data);
                    break;
                }
                Err(err) => {
                    log::error!("[{}]write failed {}", self.tag, err);
                    //TODO status
                    return;
                }
            }
        }
    }

    fn do_event(&mut self, event: &Event, registry: &Registry) {
        log::debug!("[{}]connection has event:{:?}", self.tag, event);
        if event.is_read_closed() {
        } else if event.is_readable() {
            self.do_read();
        }

        if event.is_write_closed() {
        } else if event.is_writable() {
            self.do_write();
        }

        self.reregister(registry);
    }

    fn do_read(&mut self) {
        let mut bytes = [0u8; 1024];
        loop {
            match self.stream.read(&mut bytes) {
                Ok(size) if size > 0 => self.read_bytes.extend_from_slice(&bytes[..size]),
                Ok(_) => {
                    log::error!("[{}]read zero byte, connection closed", self.tag);
                    //TODO status
                    return;
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(err) => {
                    log::error!("[{}]read failed {}", self.tag, err);
                    //TODO status
                    return;
                }
            }
        }
        self.parse();
    }

    fn parse(&mut self) {
        if self.read_bytes.is_empty() {
            return;
        }

        let mut read_bytes = self.read_bytes.as_slice();
        loop {
            if !self.header.empty() && read_bytes.len() >= self.header.length {
                let body: Vec<_> = read_bytes[..self.header.length].into();
                read_bytes = &read_bytes[self.header.length..];
                if let Err(err) = self
                    .sender
                    .send((self.ident.clone(), self.header.clone(), body))
                {
                    log::error!("send data to ecs failed:{}", err);
                    //todo close
                }
                self.header.clear();
            } else if self.header.empty() && read_bytes.len() >= N {
                self.header = (self.decoder)(&read_bytes[..N]);
                read_bytes = &read_bytes[N..];
            } else {
                break;
            }
        }

        if read_bytes.len() != self.read_bytes.len() {
            self.read_bytes = read_bytes.into();
        }
    }

    fn do_write(&mut self) {
        let mut write_bytes = Vec::new();
        std::mem::swap(&mut self.write_bytes, &mut write_bytes);
        self.send(write_bytes.as_slice());
    }

    fn is_timeout(&self, timeout: Duration) -> bool {
        self.last_time.elapsed() > timeout
    }

    fn close(&mut self) {
        if let Err(err) = self.stream.shutdown(Shutdown::Both) {
            log::error!("[{}]close failed {}", self.tag, err);
        }
    }

    fn closed(&self) -> bool {
        //TODO
        false
    }

    fn set_token(&mut self, token: Token) {
        self.token = token;
        self.ident.replace_token(token);
    }

    fn set_entity(&mut self, entity: Entity) {
        log::debug!("[{}]got entity:{:?}", self.tag, entity);
        if self.ident.is_entity() {
            log::error!("entity already set for connection:{}", self.tag);
            return;
        }
        self.ident.replace_entity(entity);
    }
}

pub enum Response {
    Entity(Entity),
    Data(Vec<u8>),
    Close,
}

pub type NetworkInputData = (RequestIdent, Header, Vec<u8>);
pub type RequestData<T> = (RequestIdent, T);
pub type NetworkOutputData = (Vec<Token>, Response);

struct Listener<T, const N: usize>
where
    T: HeaderFn,
{
    listener: TcpListener,
    conns: Slab<Connection<T, N>>,
    sender: Sender<NetworkInputData>,
    receiver: Option<Receiver<NetworkOutputData>>,
    decoder: T,
}

impl<T, const N: usize> Listener<T, N>
where
    T: HeaderFn,
    T: Clone,
{
    pub fn new(
        listener: TcpListener,
        capacity: usize,
        sender: Sender<NetworkInputData>,
        receiver: Receiver<NetworkOutputData>,
        decoder: T,
    ) -> Self {
        Self {
            listener,
            conns: Slab::with_capacity(capacity),
            sender,
            receiver: Some(receiver),
            decoder,
        }
    }

    pub fn accept(&mut self, poll: &Poll) -> Result<()> {
        loop {
            match self.listener.accept() {
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    log::debug!("no more connection, stop now");
                    return Ok(());
                }
                Err(err) => return Err(err),
                Ok((stream, addr)) => {
                    log::info!("accept connection:{}", addr);
                    let conn =
                        Connection::new(stream, addr, self.decoder.clone(), self.sender.clone());
                    self.insert(conn, poll);
                }
            }
        }
    }

    fn insert(&mut self, conn: Connection<T, N>, poll: &Poll) {
        let index = self.conns.insert(conn);
        let conn = self.conns.get_mut(index).unwrap();
        conn.set_token(Token(index));
        conn.setup(poll.registry());
    }

    pub fn do_event(&mut self, event: &Event, poll: &Poll) {
        if let Some(conn) = self.conns.get_mut(event.token().0) {
            conn.do_event(event, poll.registry());
        } else {
            log::error!("connection:{} not found", event.token().0);
        }
    }

    pub fn do_send(&mut self) {
        let receiver = self.receiver.take().unwrap();
        receiver.try_iter().for_each(|(tokens, data)| {
            for token in tokens {
                if let Some(conn) = self.conns.get_mut(token.0) {
                    match &data {
                        Response::Data(data) => conn.send(data.as_slice()),
                        Response::Entity(entity) => conn.set_entity(*entity),
                        Response::Close => conn.close(),
                    }
                } else {
                    log::error!("connection:{} not found", token.0);
                }
            }
        });
        self.receiver.replace(receiver);
    }

    pub fn check_timeout(&mut self, timeout: Duration) {
        self.conns
            .iter_mut()
            .filter(|(_, conn)| conn.is_timeout(timeout))
            .for_each(|(_, conn)| conn.close());
    }

    pub fn check_close(&mut self) {
        let indexes: Vec<_> = self
            .conns
            .iter()
            .filter(|(_, conn)| (*conn).closed())
            .map(|(index, _)| index)
            .collect();
        indexes.iter().for_each(|index| {
            self.conns.remove(*index);
        });
    }
}

const LISTENER: Token = Token(1);
const ECS_SENDER: Token = Token(2);

pub fn run_network<D, const N: usize>(
    mut poll: Poll,
    address: SocketAddr,
    sender: Sender<NetworkInputData>,
    receiver: Receiver<NetworkOutputData>,
    decoder: D,
) -> Result<()>
where
    D: HeaderFn,
    D: Clone,
{
    let mut listener = TcpListener::bind(address)?;
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;
    let mut listener: Listener<_, N> = Listener::new(listener, 4096, sender, receiver, decoder);
    let mut events = Events::with_capacity(1024);
    let poll_timeout = Duration::new(1, 0);
    let read_write_timeout = Duration::new(30, 0);
    let mut begin = Instant::now();
    loop {
        poll.poll(&mut events, Some(poll_timeout))?;
        listener.do_send();
        for event in &events {
            match event.token() {
                LISTENER => listener.accept(&poll)?,
                ECS_SENDER => {}
                _ => listener.do_event(event, &poll),
            }
        }
        if begin.elapsed() >= poll_timeout {
            begin = Instant::now();
            listener.check_close();
            listener.check_timeout(read_write_timeout);
        }
    }
}

pub fn async_run<T, D, const N: usize>(
    addr: SocketAddr,
    decoder: D,
) -> (Receiver<RequestData<T>>, ResponseSender)
where
    T: Send + Input + 'static,
    D: HeaderFn,
    D: Clone,
    D: Sync + Send + 'static,
{
    // network send data to decode, one-to-one
    let (network_sender, network_receiver) = channel::<NetworkInputData>();
    // decode send data to ecs, one-to-one
    let (request_sender, request_receiver) = channel::<RequestData<T>>();
    // ecs send data to network many-to-one
    let (response_sender, response_receiver) = channel::<NetworkOutputData>();
    let poll = Poll::new().unwrap();
    let waker = Arc::new(Waker::new(poll.registry(), ECS_SENDER).unwrap());
    rayon::spawn(move || {
        if let Err(err) =
            run_network::<_, N>(poll, addr, network_sender, response_receiver, decoder)
        {
            log::error!("network thread quit with error:{}", err);
        }
    });
    rayon::spawn(move || {
        run_decode(request_sender, network_receiver);
    });
    (
        request_receiver,
        ResponseSender::new(response_sender, waker),
    )
}

fn run_decode<T>(sender: Sender<RequestData<T>>, receiver: Receiver<NetworkInputData>)
where
    T: Input,
{
    receiver.iter().for_each(|(ident, header, data)| {
        if let Some(data) = T::decode(header.cmd, data.as_slice()) {
            if let Err(err) = sender.send((ident, data)) {
                log::error!("send data to ecs failed {}", err);
            }
        }
    })
}

/// Trait for requests enum type, it's an aggregation of all requests
pub trait Input: Sized {
    /// Match the actual type contains in enum, and add it to world.
    /// If entity is none and current type is Login, a new entity will be created.
    fn add_component(
        self,
        ident: RequestIdent,
        world: &World,
        sender: &ResponseSender,
    ) -> std::result::Result<(), specs::error::Error>;

    /// Register all the actual types as components
    fn setup(world: &mut World);

    /// Decode actual type as header specified.
    fn decode(cmd: u32, data: &[u8]) -> Option<Self>;

    #[cfg(feature = "debug")]
    fn encode(&self) -> Vec<u8>;
}

pub struct InputSystem<T> {
    receiver: Receiver<RequestData<T>>,
    sender: ResponseSender,
}

impl<T> InputSystem<T> {
    pub fn new(receiver: Receiver<RequestData<T>>, sender: ResponseSender) -> InputSystem<T> {
        Self { receiver, sender }
    }
}

impl<'a, T> RunNow<'a> for InputSystem<T>
where
    T: Input + Send + Sync + 'static,
{
    fn run_now(&mut self, world: &'a World) {
        self.receiver.try_iter().for_each(|(ident, data)| {
            log::debug!("new request found");
            if let Err(err) = data.add_component(ident, world, &self.sender) {
                log::error!("add component failed:{}", err);
            }
        })
    }

    fn setup(&mut self, world: &mut World) {
        T::setup(world);
    }
}

#[derive(Default, Clone)]
pub struct ResponseSender {
    sender: Option<Sender<NetworkOutputData>>,
    waker: Option<Arc<Waker>>,
}

impl ResponseSender {
    pub fn new(sender: Sender<NetworkOutputData>, waker: Arc<Waker>) -> Self {
        Self {
            sender: Some(sender),
            waker: Some(waker),
        }
    }

    fn broadcast(&self, tokens: Vec<Token>, response: Response) {
        if let Err(err) = self.sender.as_ref().unwrap().send((tokens, response)) {
            log::error!("send response to network failed {}", err);
        }
    }

    pub fn broadcast_data(&self, tokens: Vec<Token>, data: Vec<u8>) {
        self.broadcast(tokens, Response::Data(data));
    }

    pub fn send_data(&self, token: Token, data: Vec<u8>) {
        self.broadcast(vec![token], Response::Data(data));
    }

    pub fn send_entity(&self, token: Token, entity: Entity) {
        self.broadcast(vec![token], Response::Entity(entity));
    }

    pub fn send_close(&self, token: Token) {
        self.broadcast(vec![token], Response::Close);
    }

    pub fn flush(&self) {
        if let Err(err) = self.waker.as_ref().unwrap().wake() {
            log::error!("wake poll failed:{}", err);
        }
    }
}
