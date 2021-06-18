use std::{
    io::{ErrorKind, Read, Result, Write},
    net::{Shutdown, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(feature = "bounded")]
use crossbeam::channel::bounded as channel;
#[cfg(not(feature = "bounded"))]
use crossbeam::channel::unbounded as channel;
use crossbeam::channel::{Receiver, Sender};
use mio::{
    event::Event,
    net::{TcpListener, TcpStream},
    Events, Interest, Poll, Registry, Token, Waker,
};
use slab::Slab;
use specs::Entity;

use crate::Input;

pub trait HeaderFn = Fn(&[u8]) -> Header;

#[derive(Clone)]
pub enum RequestIdent {
    Entity(Entity),
    Close(Entity),
    Token(Token),
}

impl RequestIdent {
    pub fn token(self) -> Token {
        match self {
            RequestIdent::Token(token) => token,
            _ => panic!("not a token RequestIdent"),
        }
    }

    pub fn entity(self) -> Entity {
        match self {
            RequestIdent::Entity(entity) => entity,
            _ => panic!("not a entity RequestIdent"),
        }
    }

    pub fn replace_entity(&mut self, entity: Entity) {
        *self = RequestIdent::Entity(entity);
    }

    pub fn replace_close(&mut self) {
        if let RequestIdent::Entity(entity) = self {
            *self = RequestIdent::Close(*entity);
        } else {
            panic!("not a entity RequestIdent");
        }
    }

    pub fn replace_token(&mut self, token: Token) {
        *self = RequestIdent::Token(token);
    }

    pub fn is_entity(&self) -> bool {
        matches!(self, RequestIdent::Entity(_))
    }

    pub fn is_token(&self) -> bool {
        matches!(self, RequestIdent::Token(_))
    }
}

#[derive(Default, Clone, Debug)]
pub struct Header {
    pub length: usize,
    pub cmd: u32,
}

impl Header {
    pub fn empty(&self) -> bool {
        self.cmd == 0
    }

    pub fn clear(&mut self) {
        self.cmd = 0;
        self.length = 0;
    }

    pub fn new(cmd: u32, length: usize) -> Self {
        Self { cmd, length }
    }
}

#[derive(Debug)]
enum ConnStatus {
    /// 连接建立，可以正常进行读写，此时如果断开连接，则直接到Closed
    Established,
    /// 网络连接已经关闭
    Closed,
    /// 注册已经清除
    Deregistered,
}

#[derive(Debug)]
enum EcsStatus {
    /// 网络连接建立，正在初始化中，还未收到初始请求
    Initializing,
    /// 收到初始请求，并将Token发送到ECS
    TokenSent,
    /// 收到ECS响应Token，可以正常工作了
    EntityReceived,
    /// 网络出现问题，已经发送Close请求到ecs，等待确认
    CloseSent,
    /// Ecs确认清理已经完成，可以清理资源
    CloseConfirmed,
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
    last_read_time: Instant,
    last_write_time: Instant,
    decoder: T,
    header: Header,
    sender: Sender<NetworkInputData>,
    ident: RequestIdent,
    conn_status: ConnStatus,
    ecs_status: EcsStatus,
}

impl<T, const N: usize> Connection<T, N>
where
    T: HeaderFn,
{
    pub fn new(
        stream: TcpStream,
        address: SocketAddr,
        decoder: T,
        sender: Sender<NetworkInputData>,
    ) -> Self {
        let tag = address.to_string();
        Self {
            stream,
            tag,
            token: Token(0),
            interests: Interest::READABLE,
            read_bytes: Vec::with_capacity(1024),
            write_bytes: Vec::with_capacity(1024),
            last_time: Instant::now(),
            last_read_time: Instant::now(),
            last_write_time: Instant::now(),
            decoder,
            header: Header::default(),
            sender,
            ident: RequestIdent::Token(Token(0)),
            conn_status: ConnStatus::Established,
            ecs_status: EcsStatus::Initializing,
        }
    }

    fn setup(&mut self, registry: &Registry) {
        if let Err(err) = registry.register(&mut self.stream, self.token, self.interests) {
            log::error!("[{}]connection register failed:{}", self.tag, err);
        }
    }

    fn reregister(&mut self, registry: &Registry) {
        match self.conn_status {
            ConnStatus::Established => {
                let mut modified = false;
                if !self.write_bytes.is_empty() && !self.interests.is_writable() {
                    modified = true;
                    self.interests |= Interest::WRITABLE;
                } else if self.write_bytes.is_empty() && self.interests.is_writable() {
                    modified = true;
                    self.interests = Interest::READABLE;
                }
                if modified {
                    if let Err(err) =
                        registry.reregister(&mut self.stream, self.token, self.interests)
                    {
                        log::error!("[{}]reregister failed {}", self.tag, err);
                    }
                }
            }
            ConnStatus::Closed => {
                if let Err(err) = registry.deregister(&mut self.stream) {
                    log::error!("[{}]connection deregister failed{}", self.tag, err);
                }
                self.conn_status = ConnStatus::Deregistered;
            }
            ConnStatus::Deregistered => {
                log::warn!("[{}]connection got event after deregister", self.tag)
            }
        }
    }

    fn write(&mut self, mut data: &[u8]) {
        if !self.write_bytes.is_empty() {
            self.write_bytes.extend_from_slice(data);
            return;
        }

        self.last_write_time = Instant::now();
        loop {
            match self.stream.write(data) {
                Ok(size) => data = &data[size..],
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    self.write_bytes.extend_from_slice(data);
                    break;
                }
                Err(err) => {
                    log::error!("[{}]write failed {}", self.tag, err);
                    self.shutdown();
                    return;
                }
            }
        }
    }

    fn shutdown(&mut self) {
        if let ConnStatus::Established = self.conn_status {
            if let Err(err) = self.stream.shutdown(Shutdown::Both) {
                log::error!("[{}]close failed {}", self.tag, err);
            }
            self.conn_status = ConnStatus::Closed;
            self.read_bytes.clear();
            self.write_bytes.clear();
            self.header.clear();
            self.send_close();
            log::info!("[{}]connection closed now", self.tag);
        } else {
            log::debug!("[{}]connection already closed", self.tag);
        }
    }

    fn do_event(&mut self, event: &Event, registry: &Registry) {
        log::debug!("[{}]connection has event:{:?}", self.tag, event);
        self.last_time = Instant::now();
        if event.is_read_closed() {
            self.shutdown();
        } else if event.is_readable() {
            self.do_read();
        }

        if event.is_write_closed() {
            self.shutdown();
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
                    self.shutdown();
                    return;
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(err) => {
                    log::error!("[{}]read failed {}", self.tag, err);
                    self.shutdown();
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

        let mut read_bytes_vec = Vec::new();
        std::mem::swap(&mut read_bytes_vec, &mut self.read_bytes);
        let mut read_bytes = read_bytes_vec.as_slice();
        let mut new_header = false;
        loop {
            if !self.header.empty() && read_bytes.len() >= self.header.length {
                let body: Vec<_> = read_bytes[..self.header.length].into();
                read_bytes = &read_bytes[self.header.length..];
                self.send_ecs(body);
                self.header.clear();
            } else if self.header.empty() && read_bytes.len() >= N {
                self.header = (self.decoder)(&read_bytes[..N]);
                read_bytes = &read_bytes[N..];
                new_header = true;
            } else {
                break;
            }
        }

        if new_header {
            self.last_read_time = Instant::now();
        }

        if read_bytes.len() == read_bytes_vec.len() {
            std::mem::swap(&mut read_bytes_vec, &mut self.read_bytes);
        } else {
            self.read_bytes.extend_from_slice(read_bytes);
        }
    }

    fn send_ecs(&mut self, data: Vec<u8>) {
        match self.ecs_status {
            EcsStatus::Initializing => self.ecs_status = EcsStatus::TokenSent,
            EcsStatus::TokenSent => {
                log::error!(
                    "[{}]another request found while entity not received, dropped",
                    self.tag
                );
                return;
            }
            EcsStatus::EntityReceived => {}
            _ => {
                log::error!("[{}]close sent to ecs, should not send more data", self.tag);
                return;
            }
        }
        if let Err(err) = self
            .sender
            .send((self.ident.clone(), self.header.clone(), data))
        {
            log::error!("[{}]send data to ecs failed:{}", self.tag, err);
        }
    }

    fn send_close(&mut self) {
        match self.ecs_status {
            EcsStatus::EntityReceived => {
                self.ident.replace_close();
                self.send_ecs(Vec::new());
                self.ecs_status = EcsStatus::CloseSent;
                log::info!("[{}]connection send close to ecs", self.tag);
            }
            EcsStatus::Initializing => {
                self.ecs_status = EcsStatus::CloseConfirmed;
                log::info!(
                    "[{}]connection is initializing, close confirm now",
                    self.tag
                );
            }
            _ => log::info!(
                "[{}]connection has not received entity, close later",
                self.tag
            ),
        };
    }

    fn do_write(&mut self) {
        let mut write_bytes = Vec::new();
        std::mem::swap(&mut self.write_bytes, &mut write_bytes);
        self.write(write_bytes.as_slice());
    }

    fn is_timeout(
        &self,
        idle_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> bool {
        if let ConnStatus::Established = self.conn_status {
            if self.header.length != 0 && self.last_read_time.elapsed() > read_timeout {
                log::warn!("[{}]read timeout", self.tag);
                return true;
            }
            if !self.write_bytes.is_empty() && self.last_write_time.elapsed() > write_timeout {
                log::warn!("[{}]write timeout", self.tag);
                return true;
            }
            if self.last_time.elapsed() > idle_timeout {
                log::warn!("[{}]idle timeout", self.tag);
                return true;
            }
        }
        false
    }

    fn close(&mut self) {
        match self.ecs_status {
            EcsStatus::CloseSent => {
                log::info!("[{}]ecs confirm closed, it's ok to release now", self.tag);
                self.ecs_status = EcsStatus::CloseConfirmed;
            }
            _ => log::error!(
                "[{}]connection received CloseConfirmed while in status:{:?}",
                self.tag,
                self.ecs_status
            ),
        }
    }

    fn releasable(&self) -> bool {
        matches!(self.ecs_status, EcsStatus::CloseConfirmed)
    }

    fn set_token(&mut self, token: Token) {
        self.token = token;
        self.ident.replace_token(token);
    }

    fn set_entity(&mut self, entity: Entity) {
        log::debug!("[{}]got entity:{:?}", self.tag, entity);
        if let EcsStatus::TokenSent = self.ecs_status {
            self.ident.replace_entity(entity);
            self.ecs_status = EcsStatus::EntityReceived;
            if !matches!(self.conn_status, ConnStatus::Established) {
                self.send_close();
            }
        } else {
            log::error!(
                "[{}]connection got entity while in status:{:?}",
                self.tag,
                self.ecs_status
            );
        }
    }
}

pub enum Response {
    Entity(Entity),
    Data(Vec<u8>),
    Close(bool),
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
    idle_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
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
        idle_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> Self {
        Self {
            listener,
            conns: Slab::with_capacity(capacity),
            sender,
            receiver: Some(receiver),
            decoder,
            idle_timeout,
            read_timeout,
            write_timeout,
        }
    }

    pub fn accept(&mut self, registry: &Registry) -> Result<()> {
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
                    self.insert(registry, conn);
                }
            }
        }
    }

    fn insert(&mut self, registry: &Registry, conn: Connection<T, N>) {
        let index = self.conns.insert(conn);
        let conn = self.conns.get_mut(index).unwrap();
        conn.set_token(Self::index2token(index));
        conn.setup(registry);
        log::info!("connection:{} installed", index);
    }

    fn token2index(token: Token) -> usize {
        token.0 - MIN_CLIENT
    }

    fn index2token(index: usize) -> Token {
        Token(index + MIN_CLIENT)
    }

    pub fn do_event(&mut self, event: &Event, poll: &Poll) {
        if let Some(conn) = self.conns.get_mut(Self::token2index(event.token())) {
            conn.do_event(event, poll.registry());
        } else {
            log::error!("connection:{} not found", Self::token2index(event.token()));
        }
    }

    pub fn do_send(&mut self) {
        let receiver = self.receiver.take().unwrap();
        receiver.try_iter().for_each(|(tokens, data)| {
            for token in tokens {
                if let Some(conn) = self.conns.get_mut(Self::token2index(token)) {
                    match &data {
                        Response::Data(data) => conn.write(data.as_slice()),
                        Response::Entity(entity) => conn.set_entity(*entity),
                        Response::Close(done) => {
                            if *done {
                                conn.close()
                            } else {
                                conn.shutdown();
                            }
                        }
                    }
                } else {
                    log::error!("connection:{} not found", Self::token2index(token));
                }
            }
        });
        self.receiver.replace(receiver);
    }

    pub fn check_timeout(&mut self) {
        let idle_timeout = self.idle_timeout;
        let read_timeout = self.read_timeout;
        let write_timeout = self.write_timeout;
        self.conns
            .iter_mut()
            .filter(|(_, conn)| conn.is_timeout(idle_timeout, read_timeout, write_timeout))
            .for_each(|(_, conn)| conn.shutdown());
    }

    pub fn check_release(&mut self) {
        let indexes: Vec<_> = self
            .conns
            .iter()
            .filter(|(_, conn)| (*conn).releasable())
            .map(|(index, _)| index)
            .collect();
        indexes.iter().for_each(|index| {
            self.conns.remove(*index);
            log::info!("connection:{} released now", index);
        });
    }
}

const LISTENER: Token = Token(1);
const ECS_SENDER: Token = Token(2);
const MIN_CLIENT: usize = 3;

pub fn run_network<D, const N: usize>(
    mut poll: Poll,
    address: SocketAddr,
    sender: Sender<NetworkInputData>,
    receiver: Receiver<NetworkOutputData>,
    decoder: D,
    idle_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
    poll_timeout: Option<Duration>,
) -> Result<()>
where
    D: HeaderFn,
    D: Clone,
{
    let mut listener = TcpListener::bind(address)?;
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;
    let mut listener: Listener<_, N> = Listener::new(
        listener,
        4096,
        sender,
        receiver,
        decoder,
        idle_timeout,
        read_timeout,
        write_timeout,
    );
    let mut events = Events::with_capacity(1024);
    let mut last_check_time = Instant::now();
    let check_timeout = Duration::new(1, 0);
    loop {
        poll.poll(&mut events, poll_timeout)?;
        let registry = poll.registry();
        listener.do_send();
        for event in &events {
            match event.token() {
                LISTENER => listener.accept(registry)?,
                ECS_SENDER => {}
                _ => listener.do_event(event, &poll),
            }
        }
        if last_check_time.elapsed() >= check_timeout {
            last_check_time = Instant::now();
            listener.check_release();
            listener.check_timeout();
        }
    }
}

pub fn async_run<T, D, const N: usize>(
    address: SocketAddr,
    decoder: D,
    idle_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
    poll_timeout: Option<Duration>,
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
        if let Err(err) = run_network::<_, N>(
            poll,
            address,
            network_sender,
            response_receiver,
            decoder,
            idle_timeout,
            read_timeout,
            write_timeout,
            poll_timeout,
        ) {
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

    pub fn broadcast_close(&self, tokens: Vec<Token>) {
        self.broadcast(tokens, Response::Close(true));
    }

    pub fn send_data(&self, token: Token, data: Vec<u8>) {
        self.broadcast(vec![token], Response::Data(data));
    }

    pub fn send_entity(&self, token: Token, entity: Entity) {
        self.broadcast(vec![token], Response::Entity(entity));
    }

    pub fn send_close(&self, token: Token, done: bool) {
        self.broadcast(vec![token], Response::Close(done));
    }

    pub fn flush(&self) {
        if let Err(err) = self.waker.as_ref().unwrap().wake() {
            log::error!("wake poll failed:{}", err);
        }
    }
}
