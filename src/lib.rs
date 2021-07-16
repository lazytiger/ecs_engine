#![feature(trait_alias)]
#![feature(associated_type_bounds)]

use std::{net::SocketAddr, ops::Deref};

pub(crate) mod component;
pub(crate) mod dlog;
pub(crate) mod dynamic;
pub(crate) mod network;
pub(crate) mod resource;
pub(crate) mod sync;
pub(crate) mod system;

use crate::{
    component::FullDataCommit,
    network::async_run,
    resource::TimeStatistic,
    system::{GameSystem, PrintStatisticSystem, StatisticRunNow, StatisticSystem},
};
use byteorder::{BigEndian, ByteOrder};
use crossbeam::channel::Receiver;
use protobuf::Message;
use specs::{Dispatcher, DispatcherBuilder, Entity, RunNow, System, SystemData, World, WorldExt};
use std::{
    thread::sleep,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub use codegen::{export, init_log, request, setup, system};
pub use component::{
    Closing, HashComponent, NetToken, Position, SceneData, SceneMember, SelfSender, TeamMember,
};
pub use dlog::{init as init_logger, LogParam};
pub use dynamic::{DynamicManager, DynamicSystem};
pub use generator::{Generator, SyncDirection};
pub use network::{channel, BytesSender, RequestIdent};
pub use resource::SceneManager;
pub use sync::{ChangeSet, DataSet};
pub use system::{
    CleanStorageSystem, CloseSystem, CommitChangeSystem, GridSystem, HandshakeSystem, InputSystem,
    SceneSystem, TeamSystem,
};

use crate::{component::AroundFullData, resource::FrameCounter};
#[cfg(target_os = "windows")]
pub use libloading::os::windows::Symbol;
#[cfg(not(target_os = "windows"))]
pub use libloading::os::windows::Symbol;
use specs::shred::DynamicSystemData;
use std::marker::PhantomData;

/// Trait for requests enum type, it's an aggregation of all requests
pub trait Input {
    /// decode data and send by channels
    fn dispatch(&mut self, ident: RequestIdent, data: Vec<u8>);

    fn next_receiver(&self) -> Receiver<Vec<Entity>>;

    fn do_next(&mut self, entity: Entity);
}

pub trait CommandId<T> {
    fn cmd(_t: &T) -> u32;
}

pub trait Output: Deref<Target: Message> {
    fn encode(&self, id: u32) -> Vec<u8> {
        let mut data = vec![0u8; 12];
        self.write_to_vec(&mut data).unwrap();
        let length = (data.len() - 4) as u32;
        let cmd = Self::cmd();
        let header = data.as_mut_slice();
        BigEndian::write_u32(header, length);
        BigEndian::write_u32(&mut header[4..], id);
        BigEndian::write_u32(&mut header[8..], cmd);
        data
    }
    fn cmd() -> u32;
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
    bounded_size: usize,
    library_path: String,
    profile: bool,
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

    pub fn with_bounded_size(mut self, bounded_size: usize) -> Self {
        self.bounded_size = bounded_size;
        self
    }

    pub fn with_library_path(mut self, library_path: &str) -> Self {
        self.library_path = library_path.into();
        self
    }

    pub fn with_profile(mut self) -> Self {
        self.profile = true;
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
            bounded_size: 0,
            library_path: Default::default(),
            profile: false,
        }
    }

    pub fn run<I, S>(mut self, setup: S)
    where
        I: Input + Send + Sync + 'static,
        S: Fn(&mut World, &mut GameDispatcherBuilder, &DynamicManager) -> I,
    {
        let mut builder =
            GameDispatcherBuilder::new(DispatcherBuilder::new(), self.builder.profile);
        let mut world = World::new();
        let dm = DynamicManager::new(self.builder.library_path.clone());
        let request = setup(&mut world, &mut builder, &dm);
        let sender = async_run(
            self.address,
            self.builder.idle_timeout,
            self.builder.read_timeout,
            self.builder.write_timeout,
            self.builder.poll_timeout,
            self.builder.max_request_size,
            self.builder.max_response_size,
            self.builder.bounded_size,
            request,
        );
        world.insert(sender.clone());
        world.insert(FrameCounter::default());
        world.register::<NetToken>();

        if self.builder.profile {
            world.insert(TimeStatistic::new());
            //builder.add_thread_local("print_statistic", PrintStatisticSystem);
        }
        cfg_if::cfg_if! {
            if #[cfg(feature="debug")] {
                builder.add_thread_local("reload", crate::system::FsNotifySystem::new(self.builder.library_path.clone(), false));
            }
        }
        builder.add(CloseSystem, "close", &[]);

        builder.add(
            CleanStorageSystem::<AroundFullData>::default(),
            "around_full_data_clean",
            &[],
        );

        world.insert(dm);

        // setup dispatcher
        let mut dispatcher = builder.build();
        dispatcher.setup(&mut world);

        loop {
            // input
            world.write_resource::<FrameCounter>().next_frame();
            let start_time = Instant::now();
            dispatcher.dispatch(&world);
            world.maintain();
            // notify network
            sender.flush();
            let elapsed = start_time.elapsed();
            if elapsed < self.sleep {
                sleep(self.sleep - elapsed);
            }
        }
    }
}

pub fn unix_timestamp() -> Duration {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Err(err) => {
            log::error!("get unix timestamp failed:{}", err);
            Duration::from_secs(0)
        }
        Ok(d) => d,
    }
}

pub struct GameDispatcherBuilder<'a, 'b> {
    builder: DispatcherBuilder<'a, 'b>,
    profile: bool,
}

impl<'a, 'b> GameDispatcherBuilder<'a, 'b> {
    pub fn new(builder: DispatcherBuilder<'a, 'b>, statistic: bool) -> Self {
        Self {
            builder,
            profile: statistic,
        }
    }

    pub fn with<T>(mut self, system: T, name: &str, dep: &[&str]) -> Self
    where
        for<'c> T: GameSystem<'c> + System<'c> + Send + 'a,
    {
        let GameDispatcherBuilder {
            profile: statistic,
            builder,
        } = self;
        let builder = if statistic {
            builder.with(StatisticSystem(name.into(), system), name, dep)
        } else {
            builder.with(system, name, dep)
        };
        Self {
            builder,
            profile: statistic,
        }
    }

    pub fn add<T>(&mut self, system: T, name: &str, dep: &[&str])
    where
        for<'c> T: System<'c> + GameSystem<'c> + Send + 'a,
    {
        if self.profile {
            self.builder
                .add(StatisticSystem(name.into(), system), name, dep);
        } else {
            self.builder.add(system, name, dep);
        }
    }

    pub fn with_thread_local<T>(mut self, name: &str, system: T) -> Self
    where
        T: for<'c> RunNow<'c> + 'b,
    {
        let GameDispatcherBuilder {
            profile: statistic,
            builder,
        } = self;
        let builder = if statistic {
            builder.with_thread_local(StatisticRunNow(name.into(), system))
        } else {
            builder.with_thread_local(system)
        };
        Self {
            builder,
            profile: statistic,
        }
    }

    pub fn add_thread_local<T>(&mut self, name: &str, system: T)
    where
        T: for<'c> RunNow<'c> + 'b,
    {
        if self.profile {
            self.builder
                .add_thread_local(StatisticRunNow(name.into(), system));
        } else {
            self.builder.add_thread_local(system);
        }
    }

    pub fn build(self) -> Dispatcher<'a, 'b> {
        self.builder.build()
    }

    pub fn into(self) -> DispatcherBuilder<'a, 'b> {
        self.builder
    }
}
