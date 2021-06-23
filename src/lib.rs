#![feature(trait_alias)]

use std::{net::SocketAddr, ops::Deref};

mod component;
mod config;
mod dlog;
mod dynamic;
mod network;
mod sync;
mod system;

use crate::{network::async_run, system::InputSystem};
use specs::{DispatcherBuilder, World, WorldExt};
use std::{thread::sleep, time::Duration};

pub use codegen::{changeset, export, init_log, system};
pub use component::{Closing, HashComponent, NetToken, SelfSender};
pub use config::Generator;
pub use dlog::{init as init_logger, LogParam};
pub use dynamic::{DynamicManager, DynamicSystem};
pub use network::{RequestIdent, ResponseSender};
pub use sync::Changeset;

use crate::system::CloseSystem;
#[cfg(target_os = "windows")]
pub use libloading::os::windows::Symbol;
#[cfg(not(target_os = "windows"))]
pub use libloading::os::windows::Symbol;

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
    fn decode(data: &[u8]) -> Option<Self>;

    #[cfg(feature = "debug")]
    fn encode(&self) -> Vec<u8>;
}

pub trait Output: Sized {
    #[cfg(feature = "debug")]
    fn decode(data: &[u8]) -> Option<Self>;

    fn encode(&self) -> Vec<u8>;
}

/// 只读封装，如果某个变量从根本上不希望进行修改，则可以使用此模板类型
pub struct ReadOnly<T> {
    data: T,
}

impl<T> Deref for ReadOnly<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

#[derive(Debug)]
pub enum BuildEngineError {
    AddressNotSet,
    DecoderNotSet,
}

pub struct EngineBuilder {
    address: Option<SocketAddr>,
    fps: u32,
    idle_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
    poll_timeout: Option<Duration>,
    max_request_size: usize,
    max_response_size: usize,
}

impl EngineBuilder {
    pub fn with_address(mut self, address: SocketAddr) -> Self {
        self.address.replace(address);
        self
    }

    pub fn with_fps(mut self, fps: u32) -> Self {
        self.fps = fps;
        self
    }

    pub fn with_idle_timeout(mut self, idle_timeout: Duration) -> Self {
        self.idle_timeout = idle_timeout;
        self
    }

    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = read_timeout;
        self
    }

    pub fn with_write_timeout(mut self, write_timeout: Duration) -> Self {
        self.write_timeout = write_timeout;
        self
    }

    pub fn with_poll_timeout(mut self, poll_timeout: Option<Duration>) -> Self {
        self.poll_timeout = poll_timeout;
        self
    }

    pub fn with_max_request_size(mut self, max_request_size: usize) -> Self {
        self.max_request_size = max_request_size;
        self
    }

    pub fn with_max_response_size(mut self, max_response_size: usize) -> Self {
        self.max_response_size = max_response_size;
        self
    }

    pub fn build(self) -> Result<Engine, BuildEngineError> {
        if self.address.is_none() {
            return Err(BuildEngineError::AddressNotSet);
        }
        let address = self.address.clone().unwrap();
        let sleep = Duration::new(1, 0) / self.fps;
        Ok(Engine {
            address,
            sleep,
            builder: self,
        })
    }
}

pub struct Engine {
    address: SocketAddr,
    sleep: Duration,
    builder: EngineBuilder,
}

impl Engine {
    pub fn builder() -> EngineBuilder {
        EngineBuilder {
            address: None,
            fps: 30,
            idle_timeout: Duration::new(30 * 60, 0),
            read_timeout: Duration::new(30, 0),
            write_timeout: Duration::new(30, 0),
            max_request_size: 1024 * 16,
            max_response_size: 1024 * 16,
            poll_timeout: None,
        }
    }

    pub fn run<R, S>(self, setup: S)
    where
        R: Input + Send + Sync + 'static,
        S: Fn(&mut World, &mut DispatcherBuilder, &DynamicManager),
    {
        let (receiver, sender) = async_run::<R>(
            self.address,
            self.builder.idle_timeout,
            self.builder.read_timeout,
            self.builder.write_timeout,
            self.builder.poll_timeout,
            self.builder.max_request_size,
        );
        let mut world = World::new();
        world.insert(sender.clone());
        world.register::<NetToken>();

        let dm = DynamicManager::default();
        let mut builder = DispatcherBuilder::new();
        builder.add_thread_local(InputSystem::new(receiver, sender.clone()));
        builder.add(CloseSystem, "close", &[]);
        setup(&mut world, &mut builder, &dm);

        world.insert(dm);

        // setup dispatcher
        let mut dispatcher = builder.build();
        dispatcher.setup(&mut world);

        loop {
            // input
            dispatcher.dispatch_thread_local(&world);
            world.maintain();
            // systems
            dispatcher.dispatch_par(&world);
            world.maintain();
            // notify network
            sender.flush();
            sleep(self.sleep);
        }
    }
}
