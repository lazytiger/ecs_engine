use std::{
    io::{ErrorKind, Read, Result, Write},
    marker::PhantomData,
    net::{Shutdown, SocketAddr},
    sync::mpsc::{channel, Receiver, Sender},
    time::{Duration, Instant},
};

use crate::config::ConfigType::Request;
use mio::{
    event::Event,
    net::{TcpListener, TcpStream},
    Events, Interest, Poll, Registry, Token,
};
use slab::Slab;
use specs::{world::Index, Entity, LazyUpdate, RunNow, World, WorldExt};

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

#[derive(Default, Clone)]
pub struct Header {
    pub length: usize,
    pub cmd: u32,
    //TODO more flags for expand Header
}

struct Connection<T, const N: usize>
where
    T: Fn([u8; N]) -> Header,
{
    stream: TcpStream,
    tag: String,
    token: Token,
    interests: Interest,
    read_bytes: Vec<u8>,
    write_bytes: Vec<u8>,
    last_time: Instant,
    header_raw: [u8; N],
    decoder: T,
    header: Header,
    sender: Sender<NetworkInputData>,
    ident: RequestIdent,
}

impl<T, const N: usize> Connection<T, N>
where
    T: Fn([u8; N]) -> Header,
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
            header_raw: [0; N],
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
        if let Err(err) = registry.reregister(&mut self.stream, self.token, self.interests) {
            log::error!("[{}]reregister failed {}", self.tag, err);
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
                    log::error!("[{}]read zero byte, connecton closed", self.tag);
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
        if self.header.length == 0 && read_bytes.len() >= N {
            self.header_raw.copy_from_slice(read_bytes);
            self.header = (self.decoder)(self.header_raw);
            read_bytes = &read_bytes[N..];
        }

        while self.header.length > 0 && read_bytes.len() >= self.header.length {
            let body: Vec<_> = read_bytes[..self.header.length].into();
            if let Err(err) = self
                .sender
                .send((self.ident.clone(), self.header.clone(), body))
            {
                log::error!("send data to ecs failed:{}", err);
                //todo close
            }
            read_bytes = &read_bytes[self.header.length..];
            if read_bytes.len() >= N {
                self.header_raw.copy_from_slice(read_bytes);
                self.header = (self.decoder)(self.header_raw);
                read_bytes = &read_bytes[N..];
            } else {
                self.header.length = 0;
            }
        }

        self.read_bytes = read_bytes.into();
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
        todo!()
    }

    fn set_token(&mut self, token: Token) {
        self.token = token;
        self.ident.replace_token(token);
    }
}

pub type NetworkInputData = (RequestIdent, Header, Vec<u8>);
pub type RequestData<T> = (RequestIdent, T);
pub type NetworkOutputData = (Token, Vec<u8>);

struct Listener<T, const N: usize>
where
    T: Clone,
    T: Fn([u8; N]) -> Header,
{
    listener: TcpListener,
    conns: Slab<Connection<T, N>>,
    sender: Sender<NetworkInputData>,
    receiver: Option<Receiver<NetworkOutputData>>,
    decoder: T,
}

impl<T, const N: usize> Listener<T, N>
where
    T: Clone,
    T: Fn([u8; N]) -> Header,
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
                    return Ok(());
                }
                Err(err) => return Err(err),
                Ok((stream, addr)) => {
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
        conn.token = Token(index);
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
        receiver.try_iter().for_each(|(token, data)| {
            if let Some(conn) = self.conns.get_mut(token.0) {
                conn.send(data.as_slice());
            } else {
                log::error!("connection:{} not found", token.0);
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
    address: SocketAddr,
    sender: Sender<NetworkInputData>,
    receiver: Receiver<NetworkOutputData>,
    decoder: D,
) -> Result<()>
where
    D: Fn([u8; N]) -> Header,
    D: Clone,
{
    let listener = TcpListener::bind(address)?;
    let mut listener = Listener::new(listener, 4096, sender, receiver, decoder);
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);
    let mut poll_timeout = Duration::new(1, 0);
    let mut read_write_timeout = Duration::new(30, 0);
    let mut begin = Instant::now();
    loop {
        poll.poll(&mut events, Some(poll_timeout))?;
        for event in &events {
            match event.token() {
                LISTENER => listener.accept(&poll)?,
                ECS_SENDER => listener.do_send(),
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
) -> (Receiver<RequestData<T>>, Sender<NetworkOutputData>)
where
    T: Send + Input + 'static,
    D: Fn([u8; N]) -> Header,
    D: Clone + Sync + Send + 'static,
{
    // network send data to decode, one-to-one
    let (network_sender, network_receiver) = channel::<NetworkInputData>();
    // decode send data to ecs, one-to-one
    let (request_sender, request_receiver) = channel::<RequestData<T>>();
    // ecs send data to network many-to-one
    let (response_sender, response_receiver) = channel::<NetworkOutputData>();
    rayon::spawn(move || {
        if let Err(err) = run_network(addr, network_sender, response_receiver, decoder) {
            log::error!("network thread quit with error:{}", err);
        }
    });
    rayon::spawn(move || {
        run_decode(request_sender, network_receiver);
    });
    (request_receiver, response_sender)
}

fn run_decode<T>(sender: Sender<RequestData<T>>, receiver: Receiver<NetworkInputData>)
where
    T: Input,
{
    receiver.iter().for_each(|(ident, header, data)| {
        let data = T::decode(header.cmd, data.as_slice());
        if let Err(err) = sender.send((ident, data)) {
            log::error!("send data to ecs failed {}", err);
        }
    })
}

/// Trait for requests enum type, it's an aggregation of all requests
pub trait Input {
    /// Match the actual type contains in enum, and add it to world.
    /// If entity is none and current type is Login, a new entity will be created.
    fn add_component(
        self,
        ident: RequestIdent,
        world: &World,
    ) -> std::result::Result<(), specs::error::Error>;

    /// Register all the actual types as components
    fn setup(world: &mut World);

    /// Decode actual type as header specified.
    fn decode(cmd: u32, data: &[u8]) -> Self;
}

pub struct InputSystem<T> {
    receiver: Receiver<RequestData<T>>,
}

impl<T> InputSystem<T> {
    pub fn new(receiver: Receiver<RequestData<T>>) -> InputSystem<T> {
        Self { receiver }
    }
}

impl<'a, T> RunNow<'a> for InputSystem<T>
where
    T: Input + Send + Sync + 'static,
{
    fn run_now(&mut self, world: &'a World) {
        self.receiver.try_iter().for_each(|(ident, data)| {
            data.add_component(ident, world);
        })
    }

    fn setup(&mut self, world: &mut World) {
        T::setup(world);
    }
}
