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
use network::HeaderFn;
use specs::{DispatcherBuilder, World, WorldExt};
use std::{thread::sleep, time::Duration};

pub use codegen::{changeset, export, init_log, system};
pub use component::{HashComponent, NetToken};
pub use config::Generator;
pub use dlog::{init as init_logger, LogParam};
pub use dynamic::{DynamicManager, DynamicSystem};
pub use network::{Header, RequestIdent, ResponseSender};
pub use sync::Changeset;

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
    fn decode(cmd: u32, data: &[u8]) -> Option<Self>;

    #[cfg(feature = "debug")]
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

pub struct EngineBuilder<T> {
    address: Option<SocketAddr>,
    decoder: Option<T>,
    fps: u32,
}

impl<T> EngineBuilder<T> {
    pub fn with_address(mut self, address: SocketAddr) -> Self {
        self.address.replace(address);
        self
    }

    pub fn with_decoder(mut self, decoder: T) -> Self {
        self.decoder.replace(decoder);
        self
    }

    pub fn with_fps(mut self, fps: u32) -> Self {
        self.fps = fps;
        self
    }

    pub fn build(self) -> Result<Engine<T>, BuildEngineError> {
        if self.address.is_none() {
            return Err(BuildEngineError::AddressNotSet);
        }
        if self.decoder.is_none() {
            return Err(BuildEngineError::DecoderNotSet);
        }
        let address = self.address.unwrap();
        let decoder = self.decoder.unwrap();
        let sleep = Duration::new(1, 0) / self.fps;
        Ok(Engine {
            address,
            decoder,
            sleep,
        })
    }
}

pub struct Engine<T> {
    address: SocketAddr,
    decoder: T,
    sleep: Duration,
}

impl<T> Engine<T>
where
    T: HeaderFn,
    T: Clone,
    T: Send + Sync + 'static,
{
    pub fn builder() -> EngineBuilder<T> {
        EngineBuilder {
            address: None,
            decoder: None,
            fps: 30,
        }
    }

    pub fn run<R, S, const N: usize>(self, setup: S)
    where
        R: Input + Send + Sync + 'static,
        S: Fn(&mut World, &mut DispatcherBuilder, &DynamicManager),
    {
        let (receiver, sender) = async_run::<R, _, N>(self.address, self.decoder);
        let mut world = World::new();
        world.insert(sender.clone());
        world.register::<NetToken>();

        let dm = DynamicManager::default();
        let mut builder = DispatcherBuilder::new();
        builder.add_thread_local(InputSystem::new(receiver, sender.clone()));
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
