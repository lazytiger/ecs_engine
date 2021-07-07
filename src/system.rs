use crate::{
    component::{Closing, NewSceneMember, Position, SceneData, SceneMember, TeamMember},
    dynamic::{get_library_name, Library},
    network::{BytesSender, RequestData, ResponseSender},
    resource::{GridManager, SceneHierarchy, TeamHierarchy, TimeStatistic},
    sync::ChangeSet,
    DataSet, DynamicManager, Input, NetToken, RequestIdent, SyncDirection,
};
use crossbeam::channel::Receiver;
use notify::{DebouncedEvent, RecommendedWatcher, RecursiveMode, Watcher};
use protobuf::Mask;
use specs::{
    shred::SystemData, BitSet, Component, Entities, Join, LazyUpdate, Read, ReadExpect,
    ReadStorage, RunNow, System, Tracked, World, WorldExt, WriteExpect, WriteStorage,
};
use specs_hierarchy::{HierarchySystem, Parent};
use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
    time::{Duration, UNIX_EPOCH},
};

pub struct InputSystem<I, O> {
    receiver: Receiver<RequestData<I>>,
    sender: ResponseSender<O>,
}

impl<I, O> InputSystem<I, O> {
    pub fn new(receiver: Receiver<RequestData<I>>, sender: ResponseSender<O>) -> Self {
        Self { receiver, sender }
    }
}

impl<'a, I, O> RunNow<'a> for InputSystem<I, O>
where
    I: Input<Output = O> + Send + Sync + 'static,
    O: Clone,
{
    fn run_now(&mut self, world: &'a World) {
        //TODO how to control input frequency.
        self.receiver.try_iter().for_each(|(ident, data)| {
            log::debug!("new request found");
            if let Some(data) = data {
                if let Err(err) = data.add_component(ident, world, self.sender.clone()) {
                    log::error!("add component failed:{}", err);
                }
            } else {
                match ident {
                    RequestIdent::Entity(entity) => {
                        if let Some(token) = world.read_component::<NetToken>().get(entity) {
                            self.sender.send_close(token.token(), false);
                        } else {
                            log::error!("entity:{:?} has no NetToken component", entity);
                        }
                    }
                    RequestIdent::Token(token) => {
                        self.sender.send_close(token, false);
                    }
                    _ => unreachable!("close shouldn't decode failed"),
                }
            }
        })
    }

    fn setup(&mut self, world: &mut World) {
        I::setup(world);
    }
}

pub struct CloseSystem<T> {
    _phantom: PhantomData<T>,
}

impl<T> CloseSystem<T> {
    pub fn new() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }
}

impl<'a, T> System<'a> for CloseSystem<T>
where
    T: Send + Sync + 'static,
{
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, Closing>,
        ReadStorage<'a, NetToken>,
        Read<'a, LazyUpdate>,
    );

    fn run(&mut self, (entities, closing, tokens, lazy_update): Self::SystemData) {
        let (entities, tokens): (Vec<_>, Vec<_>) = (&entities, &tokens, &closing)
            .join()
            .map(|(entity, token, _)| (entity, token.token()))
            .unzip();
        if entities.is_empty() {
            return;
        }

        lazy_update.exec_mut(move |world| {
            if let Err(err) = world.delete_entities(entities.as_slice()) {
                log::error!("delete entities failed:{}", err);
            }
            log::debug!("{} entities deleted", entities.len());
            world
                .read_resource::<ResponseSender<T>>()
                .broadcast_close(tokens);
        });
    }

    fn setup(&mut self, world: &mut World) {
        world.register::<Closing>();
    }
}

pub struct FsNotifySystem {
    _watcher: RecommendedWatcher,
    receiver: std::sync::mpsc::Receiver<DebouncedEvent>,
}

impl FsNotifySystem {
    pub fn new(path: String, recursive: bool) -> FsNotifySystem {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut watcher =
            notify::watcher(sender, Duration::from_secs(2)).expect("create FsNotify failed");
        watcher
            .watch(
                path,
                if recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                },
            )
            .expect("watch FsNotify failed");
        Self {
            _watcher: watcher,
            receiver,
        }
    }
}

impl<'a> RunNow<'a> for FsNotifySystem {
    fn run_now(&mut self, world: &'a World) {
        let dm = world.write_resource::<DynamicManager>();
        self.receiver.try_iter().for_each(|event| match event {
            DebouncedEvent::Create(path) | DebouncedEvent::Write(path) => {
                log::debug!("path:{:?} changed", path);
                if let Some(lname) = get_library_name(path) {
                    log::warn!("library {} updated", lname);
                    let lib = dm.get(&lname);
                    let lib: &mut Library = unsafe {
                        #[allow(mutable_transmutes)]
                        std::mem::transmute(lib.as_ref())
                    };
                    lib.reload();
                }
            }
            DebouncedEvent::Error(err, path) => {
                log::error!("Found error:{} in path {:?}", err, path)
            }
            _ => {}
        })
    }

    fn setup(&mut self, _world: &mut World) {}
}

pub struct CommitChangeSystem<T, P, S> {
    tick_step: usize,
    counter: usize,
    _phantom: PhantomData<(T, P, S)>,
}

impl<T, P, S> CommitChangeSystem<T, P, S> {
    fn new(tick_step: usize) -> Self {
        Self {
            tick_step,
            counter: 0,
            _phantom: Default::default(),
        }
    }
}

impl<T, P, S> Default for CommitChangeSystem<T, P, S> {
    fn default() -> Self {
        Self::new(1)
    }
}

impl<'a, T, P, S> System<'a> for CommitChangeSystem<T, P, S>
where
    T: Component + ChangeSet + DataSet,
    <T as Deref>::Target: Mask,
    T: DerefMut,
    P: Component + Position + Send + Sync + 'static,
    P::Storage: Tracked,
    S: Component + SceneData + Send + Sync + 'static,
    S::Storage: Tracked,
{
    type SystemData = (
        WriteStorage<'a, T>,
        ReadStorage<'a, NetToken>,
        ReadStorage<'a, TeamMember>,
        ReadExpect<'a, TeamHierarchy>,
        Read<'a, BytesSender>,
        Entities<'a>,
        ReadExpect<'a, GridManager<P, S>>,
        WriteStorage<'a, NewSceneMember>,
    );

    fn run(
        &mut self,
        (mut data, token, teams, hteams, sender, entities, gm, mut new_scene_member): Self::SystemData,
    ) {
        self.counter += 1;
        if self.counter != self.tick_step {
            return;
        } else {
            self.counter = 0;
        }

        // 处理有新玩家进入时需要完整数据集的情况
        for (data, member, entity) in (&data, new_scene_member.drain(), &entities).join() {
            if !data.is_direction_enabled(SyncDirection::Around) {
                continue;
            }
            let mut data = data.clone();
            data.mask_all();
            if let Some(bytes) = data.encode(SyncDirection::Around) {
                let around = if let Some(around) = member.0 {
                    around
                } else {
                    gm.get_user_around(entity)
                };
                let tokens = NetToken::tokens(&token, &around);
                sender.broadcast_bytes(tokens, bytes)
            }
        }

        if !T::is_storage_dirty() {
            return;
        }

        // 处理针对玩家的数据集
        let mut modified = BitSet::new();
        for (data, token, entity) in (&mut data, &token, &entities).join() {
            if data.is_data_dirty() {
                data.commit();
                let bytes = data.encode(SyncDirection::Client);
                if let Some(bytes) = bytes {
                    sender.send_bytes(token.token(), bytes);
                }
                modified.add(entity.id());
            }
        }

        // 处理针对组队的数据集
        for (data, _, team) in (&mut data, &modified, &teams).join() {
            if let Some(bytes) = data.encode(SyncDirection::Team) {
                let members = hteams.all_children(team.parent_entity());
                let tokens = NetToken::tokens(&token, &members);
                sender.broadcast_bytes(tokens, bytes);
            }
        }

        // 处理针对场景的数据集
        for (data, _, entity) in (&mut data, &modified, &entities).join() {
            if let Some(bytes) = data.encode(SyncDirection::Around) {
                let around = gm.get_user_around(entity);
                let tokens = NetToken::tokens(&token, &around);
                sender.broadcast_bytes(tokens, bytes)
            }
        }

        T::clear_storage_dirty();
    }
}

pub type TeamSystem = HierarchySystem<TeamMember>;
pub type SceneSystem = HierarchySystem<SceneMember>;

pub struct GridSystem<P, S> {
    _phantom: PhantomData<(P, S)>,
}

impl<'a, P, S> GridSystem<P, S>
where
    P: Component + Position + Send + Sync + 'static,
    P::Storage: Tracked,
    S: Component + SceneData + Send + Sync + 'static,
    S::Storage: Tracked,
{
    pub fn new(world: &mut World) -> Self {
        if !world.has_value::<GridManager<P, S>>() {
            let gm = {
                let mut p_storage = world.write_storage::<P>();
                let mut s_storage = world.write_storage::<S>();
                let mut hierarchy = world.write_resource::<SceneHierarchy>();
                GridManager::<P, S>::new(
                    p_storage.register_reader(),
                    s_storage.register_reader(),
                    hierarchy.track(),
                )
            };
            world.insert(gm);
        }
        Self {
            _phantom: Default::default(),
        }
    }
}

impl<'a, P, S> System<'a> for GridSystem<P, S>
where
    P: Component + Position + Send + Sync + 'static,
    P::Storage: Tracked,
    S: Component + SceneData + Send + Sync + 'static,
    S::Storage: Tracked,
{
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, P>,
        ReadStorage<'a, SceneMember>,
        ReadStorage<'a, S>,
        WriteExpect<'a, GridManager<P, S>>,
        ReadExpect<'a, SceneHierarchy>,
        WriteStorage<'a, NewSceneMember>,
    );

    fn run(
        &mut self,
        (entities, positions, scene, scene_data, mut gm, hierarchy, new_scene_member): Self::SystemData,
    ) {
        gm.maintain(
            entities,
            positions,
            scene,
            scene_data,
            hierarchy,
            new_scene_member,
        );
    }
}

pub struct StatisticSystem<T>(pub String, pub T);

impl<'a, T> System<'a> for StatisticSystem<T>
where
    T: System<'a>,
    T::SystemData: SystemData<'a>,
{
    type SystemData = (ReadExpect<'a, TimeStatistic>, T::SystemData);

    fn run(&mut self, (ts, data): Self::SystemData) {
        let begin = UNIX_EPOCH.elapsed().unwrap();
        self.1.run(data);
        let end = UNIX_EPOCH.elapsed().unwrap();
        ts.add_time(self.0.clone(), begin, end);
    }
}

pub struct PrintStatisticSystem;

impl<'a> System<'a> for PrintStatisticSystem {
    type SystemData = ReadExpect<'a, TimeStatistic>;

    fn run(&mut self, data: Self::SystemData) {
        data.print();
        data.clear();
    }
}
