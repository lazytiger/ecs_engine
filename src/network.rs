use mio::{
    event::Event,
    net::{TcpListener, TcpStream},
    Events, Interest, Poll, Registry, Token,
};
use slab::Slab;
use specs::{world::Index, RunNow, World};
use std::{
    io::{ErrorKind, Read, Result, Write},
    marker::PhantomData,
    net::{Shutdown, SocketAddr},
    sync::mpsc::{channel, Receiver, Sender},
    time::{Duration, Instant},
};

struct Connection<T: Fn(&[u8]) -> usize, const N: usize> {
    stream: TcpStream,
    tag: String,
    token: Token,
    interests: Interest,
    index: Index,
    read_bytes: Vec<u8>,
    write_bytes: Vec<u8>,
    last_time: Instant,
    header: [u8; N],
    decoder: T,
    length: usize,
}

impl<T: Fn(&[u8]) -> usize, const N: usize> Connection<T, N> {
    pub fn new(stream: TcpStream, addr: SocketAddr, decoder: T) -> Self {
        let tag = addr.to_string();
        Self {
            stream,
            tag,
            token: Token(0),
            interests: Interest::READABLE,
            index: 0,
            read_bytes: Vec::with_capacity(1024),
            write_bytes: Vec::with_capacity(1024),
            last_time: Instant::now(),
            header: [0; N],
            decoder,
            length: 0,
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
        if self.length == 0 && read_bytes.len() >= N {
            self.header.copy_from_slice(read_bytes);
            self.length = (self.decoder)(&self.header);
            read_bytes = &read_bytes[N..];
        }

        while self.length > 0 && read_bytes.len() >= self.length {
            let body: Vec<_> = read_bytes[..self.length].into();
            //self.sender.send((self.index, self.header, body));
            read_bytes = &read_bytes[self.length..];
            self.length = 0;
            if read_bytes.len() >= N {
                self.header.copy_from_slice(read_bytes);
                self.length = (self.decoder)(&self.header);
                read_bytes = &read_bytes[N..];
            } else {
                self.length = 0;
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
}

pub type NetworkData = (usize, Vec<u8>);

struct Listener<T: Fn(&[u8]) -> usize + Clone, const N: usize> {
    listener: TcpListener,
    conns: Slab<Connection<T, N>>,
    sender: Sender<NetworkData>,
    receiver: Option<Receiver<NetworkData>>,
    decoder: T,
}

impl<T: Fn(&[u8]) -> usize + Clone, const N: usize> Listener<T, N> {
    pub fn new(
        listener: TcpListener,
        capacity: usize,
        sender: Sender<NetworkData>,
        receiver: Receiver<NetworkData>,
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
                    let conn = Connection::new(stream, addr, self.decoder.clone());
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
            if let Some(conn) = self.conns.get_mut(token) {
                conn.send(data.as_slice());
            } else {
                log::error!("connection:{} not found", token);
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

pub fn run(
    addr: SocketAddr,
    sender: Sender<NetworkData>,
    receiver: Receiver<NetworkData>,
) -> Result<()> {
    let listener = TcpListener::bind(addr)?;
    let mut listener = Listener::<_, 8>::new(listener, 4096, sender, receiver, |data| 0);
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

pub fn async_run(addr: SocketAddr) -> Sender<NetworkData> {
    let (sender, receiver) = channel();
    let r_sender = sender.clone();
    rayon::spawn(move || {
        if let Err(err) = run(addr, sender, receiver) {
            log::error!("network thread quit with error:{}", err);
        }
    });
    r_sender
}

pub trait Input {
    fn add_component(self, world: &World);
    fn setup(world: &World);
}

pub struct InputSystem<T> {
    receiver: Receiver<T>,
}

impl<T> InputSystem<T> {
    pub fn new(receiver: Receiver<T>) -> InputSystem<T> {
        Self { receiver }
    }
}

impl<'a, T> RunNow<'a> for InputSystem<T>
where
    T: Input,
{
    fn run_now(&mut self, world: &'a World) {
        self.receiver.try_iter().for_each(|t: T| {
            t.add_component(world);
        })
    }

    fn setup(&mut self, world: &mut World) {
        T::setup(world);
    }
}
