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
use byteorder::{BigEndian, ByteOrder};

/// 请求标识
#[derive(Clone)]
pub enum RequestIdent {
    /// Entity已经建立，正常工作中
    Entity(Entity),
    /// 网络端连接已经关闭
    Close(Entity),
    /// 握手包，通知当前的网络Token
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

struct Connection {
    stream: TcpStream,
    tag: String,
    token: Token,
    read_bytes: Vec<u8>,
    write_bytes: Vec<u8>,
    last_time: Instant,
    last_read_time: Instant,
    last_write_time: Instant,
    sender: Sender<NetworkInputData>,
    ident: RequestIdent,
    conn_status: ConnStatus,
    ecs_status: EcsStatus,
    length: usize,
    max_request_size: usize,
}

impl Connection {
    pub fn new(
        stream: TcpStream,
        address: SocketAddr,
        sender: Sender<NetworkInputData>,
        max_request_size: usize,
    ) -> Self {
        let tag = address.to_string();
        Self {
            stream,
            tag,
            token: Token(0),
            read_bytes: Vec::with_capacity(1024),
            write_bytes: Vec::with_capacity(1024),
            last_time: Instant::now(),
            last_read_time: Instant::now(),
            last_write_time: Instant::now(),
            sender,
            ident: RequestIdent::Token(Token(0)),
            conn_status: ConnStatus::Established,
            ecs_status: EcsStatus::Initializing,
            length: 0,
            max_request_size,
        }
    }

    fn setup(&mut self, token: Token, registry: &Registry) {
        self.token = token;
        self.ident.replace_token(token);
        if let Err(err) = registry.register(
            &mut self.stream,
            self.token,
            Interest::WRITABLE | Interest::READABLE,
        ) {
            log::error!("[{}]connection register failed:{}", self.tag, err);
        }
    }

    fn reregister(&mut self, registry: &Registry) {
        match self.conn_status {
            ConnStatus::Established => {}
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

    fn write(&mut self, data: &[u8]) {
        let write_bytes = if self.write_bytes.is_empty() {
            Vec::new()
        } else {
            self.write_bytes.extend_from_slice(data);
            let mut write_bytes = Vec::new();
            std::mem::swap(&mut write_bytes, &mut self.write_bytes);
            write_bytes
        };

        let mut data = if write_bytes.is_empty() {
            data
        } else {
            write_bytes.as_slice()
        };

        self.last_write_time = Instant::now();
        while !data.is_empty() {
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
            self.length = 0;
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
            if self.length > 0 && read_bytes.len() >= self.length {
                let body: Vec<_> = read_bytes[..self.length].into();
                read_bytes = &read_bytes[self.length..];
                self.send_ecs(body);
                self.length = 0;
            } else if self.length == 0 && read_bytes.len() >= 4 {
                self.length = BigEndian::read_u32(read_bytes) as usize;
                if self.length > self.max_request_size {
                    log::error!("[{}]got invalid request size:{}", self.tag, self.length);
                    self.shutdown();
                    return;
                }
                read_bytes = &read_bytes[4..];
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
        if let Err(err) = self.sender.send((self.ident.clone(), data)) {
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
        if self.write_bytes.is_empty() {
            return;
        }
        self.write(&[]);
    }

    fn do_send(&mut self, registry: &Registry, data: &[u8]) {
        log::debug!("[{}]got {} bytes data", self.tag, data.len());
        self.write(data);
        self.reregister(registry);
    }

    fn do_close(&mut self, confirm: bool) {
        log::debug!("[{}]got close {}", self.tag, confirm);
        if confirm {
            self.close();
        } else {
            self.shutdown();
        }
    }

    fn is_timeout(
        &self,
        idle_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> bool {
        if let ConnStatus::Established = self.conn_status {
            if self.length != 0 && self.last_read_time.elapsed() > read_timeout {
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
    /// 握手完成，返回对应的Entity
    Entity(Entity),
    /// 需要发送给用户的数据
    Data(Vec<u8>),
    /// 逻辑端需要关闭网络连接
    /// true表示Ecs已经确认清理完成，网络端可以释放资源了
    /// false表示Ecs发现问题，需要网络端关闭连接
    Close(bool),
}

pub type NetworkInputData = (RequestIdent, Vec<u8>);
pub type RequestData<T> = (RequestIdent, Option<T>);
pub type NetworkOutputData = (Vec<Token>, Response);

struct Listener {
    listener: TcpListener,
    conns: Slab<Connection>,
    sender: Sender<NetworkInputData>,
    receiver: Option<Receiver<NetworkOutputData>>,
    idle_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
}

impl Listener {
    pub fn new(
        listener: TcpListener,
        capacity: usize,
        sender: Sender<NetworkInputData>,
        receiver: Receiver<NetworkOutputData>,
        idle_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> Self {
        Self {
            listener,
            conns: Slab::with_capacity(capacity),
            sender,
            receiver: Some(receiver),
            idle_timeout,
            read_timeout,
            write_timeout,
        }
    }

    pub fn accept(&mut self, registry: &Registry, max_request_size: usize) -> Result<()> {
        loop {
            match self.listener.accept() {
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    log::debug!("no more connection, stop now");
                    return Ok(());
                }
                Err(err) => return Err(err),
                Ok((stream, addr)) => {
                    log::info!("accept connection:{}", addr);
                    let conn = Connection::new(stream, addr, self.sender.clone(), max_request_size);
                    self.insert(registry, conn);
                }
            }
        }
    }

    fn insert(&mut self, registry: &Registry, conn: Connection) {
        let index = self.conns.insert(conn);
        let conn = self.conns.get_mut(index).unwrap();
        conn.setup(Self::index2token(index), registry);
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

    pub fn do_send(&mut self, registry: &Registry) {
        let receiver = self.receiver.take().unwrap();
        receiver.try_iter().for_each(|(tokens, data)| {
            for token in tokens {
                if let Some(conn) = self.conns.get_mut(Self::token2index(token)) {
                    match &data {
                        Response::Data(data) => conn.do_send(registry, data.as_slice()),
                        Response::Entity(entity) => conn.set_entity(*entity),
                        Response::Close(confirm) => conn.do_close(*confirm),
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

pub fn run_network(
    mut poll: Poll,
    address: SocketAddr,
    sender: Sender<NetworkInputData>,
    receiver: Receiver<NetworkOutputData>,
    idle_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
    poll_timeout: Option<Duration>,
    max_request_size: usize,
) -> Result<()> {
    let mut listener = TcpListener::bind(address)?;
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;
    let mut listener = Listener::new(
        listener,
        4096,
        sender,
        receiver,
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
        listener.do_send(registry);
        for event in &events {
            match event.token() {
                LISTENER => listener.accept(registry, max_request_size)?,
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

pub fn async_run<T>(
    address: SocketAddr,
    idle_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
    poll_timeout: Option<Duration>,
    max_request_size: usize,
    max_response_size: usize,
) -> (Receiver<RequestData<T>>, ResponseSender)
where
    T: Send + Input + 'static,
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
        if let Err(err) = run_network(
            poll,
            address,
            network_sender,
            response_receiver,
            idle_timeout,
            read_timeout,
            write_timeout,
            poll_timeout,
            max_request_size,
        ) {
            log::error!("network thread quit with error:{}", err);
        }
    });
    rayon::spawn(move || {
        run_decode(request_sender, network_receiver);
    });
    (
        request_receiver,
        ResponseSender::new(response_sender, waker, max_response_size),
    )
}

fn run_decode<T>(sender: Sender<RequestData<T>>, receiver: Receiver<NetworkInputData>)
where
    T: Input,
{
    receiver.iter().for_each(|(ident, data)| {
        let data = T::decode(data.as_slice());
        if let Err(err) = sender.send((ident, data)) {
            log::error!("send data to ecs failed {}", err);
        }
    })
}

#[derive(Default, Clone)]
pub struct ResponseSender {
    sender: Option<Sender<NetworkOutputData>>,
    waker: Option<Arc<Waker>>,
    max_response_size: usize,
}

impl ResponseSender {
    pub fn new(
        sender: Sender<NetworkOutputData>,
        waker: Arc<Waker>,
        max_response_size: usize,
    ) -> Self {
        Self {
            sender: Some(sender),
            waker: Some(waker),
            max_response_size,
        }
    }

    fn broadcast(&self, tokens: Vec<Token>, response: Response) {
        if let Err(err) = self.sender.as_ref().unwrap().send((tokens, response)) {
            log::error!("send response to network failed {}", err);
        }
    }

    pub fn broadcast_data(&self, tokens: Vec<Token>, data: Vec<u8>) {
        if data.len() > self.max_response_size {
            log::error!(
                "response size:{} is greater than {}",
                data.len(),
                self.max_response_size
            );
        }
        self.broadcast(tokens, Response::Data(data));
    }

    pub fn broadcast_close(&self, tokens: Vec<Token>) {
        self.broadcast(tokens, Response::Close(true));
    }

    pub fn send_data(&self, token: Token, data: Vec<u8>) {
        self.broadcast_data(vec![token], data);
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
