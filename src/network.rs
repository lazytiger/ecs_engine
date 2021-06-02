use mio::{
    event::Event,
    net::{TcpListener, TcpSocket, TcpStream},
    Events, Interest, Poll, Registry, Token,
};
use slab::Slab;
use specs::world::Index;
use std::{
    io::{ErrorKind, Result},
    net::SocketAddr,
    sync::mpsc::{channel, Receiver, Sender},
};

struct Connection {
    stream: TcpStream,
    tag: String,
    token: Token,
    interests: Interest,
    index: Index,
}

impl Connection {
    pub fn new(stream: TcpStream, addr: SocketAddr) -> Connection {
        let tag = addr.to_string();
        Self {
            stream,
            tag,
            token: Token(0),
            interests: Interest::READABLE,
            index: 0,
        }
    }

    fn setup(&mut self, registry: &Registry) {
        if let Err(err) = registry.register(&mut self.stream, self.token, self.interests) {
            log::error!("[{}]connection register failed:{}", self.tag, err);
        }
    }

    fn send(&mut self, data: Vec<u8>) {}

    fn do_event(&mut self, event: &Event, registry: &Registry) {}
}

pub type NetworkData = (usize, Vec<u8>);

struct Listener {
    listener: TcpListener,
    conns: Slab<Connection>,
    sender: Sender<NetworkData>,
    receiver: Receiver<NetworkData>,
}

impl Listener {
    pub fn new(
        listener: TcpListener,
        capacity: usize,
        sender: Sender<NetworkData>,
        receiver: Receiver<NetworkData>,
    ) -> Listener {
        Self {
            listener,
            conns: Slab::with_capacity(capacity),
            sender,
            receiver,
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
                    let conn = Connection::new(stream, addr);
                    self.insert(conn, poll);
                }
            }
        }
    }

    fn insert(&mut self, conn: Connection, poll: &Poll) {
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
        let data: Vec<NetworkData> = self.receiver.try_iter().collect();
        for (token, data) in data {
            if let Some(conn) = self.conns.get_mut(token) {
                conn.send(data);
            } else {
                log::error!("connection:{} not found", token);
            }
        }
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
    let mut listener = Listener::new(listener, 4096, sender, receiver);
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);
    loop {
        poll.poll(&mut events, None)?;
        for event in &events {
            match event.token() {
                LISTENER => listener.accept(&poll)?,
                ECS_SENDER => listener.do_send(),
                _ => listener.do_event(event, &poll),
            }
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
