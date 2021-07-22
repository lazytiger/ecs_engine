use crate::{
    backend::{DropEntity, DummySceneSyncBackend},
    component::{AroundFullData, Closing, SceneMember, TeamFullData, TeamMember},
    dynamic::{get_library_name, Library},
    events_to_bitsets,
    network::BytesSender,
    resource::{FrameCounter, SceneManager, TeamHierarchy, TimeStatistic},
    DataSet, DynamicManager, NetToken, SceneSyncBackend, SelfSender, SyncDirection,
};
use crossbeam::channel::{Receiver, Sender};
use mio::Token;
use notify::{DebouncedEvent, RecommendedWatcher, RecursiveMode, Watcher};
use protobuf::Mask;
use specs::{
    hibitset::BitSetLike, prelude::ComponentEvent, shred::SystemData, storage::GenericWriteStorage,
    BitSet, Component, Entities, Entity, Join, LazyUpdate, Read, ReadExpect, ReadStorage, ReaderId,
    RunNow, System, Tracked, World, WorldExt, WriteExpect, WriteStorage,
};
use specs_hierarchy::{HierarchySystem, Parent};
use std::{
    collections::HashMap,
    fmt::Debug,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    time::{Duration, UNIX_EPOCH},
};

pub struct HandshakeSystem {
    receiver: Receiver<Token>,
}

impl HandshakeSystem {
    pub fn new(receiver: Receiver<Token>) -> Self {
        Self { receiver }
    }
}

impl<'a> System<'a> for HandshakeSystem {
    type SystemData = (
        WriteStorage<'a, NetToken>,
        Entities<'a>,
        ReadExpect<'a, BytesSender>,
        WriteStorage<'a, SelfSender>,
    );

    fn run(&mut self, (mut net_token, entities, sender, mut ss): Self::SystemData) {
        self.receiver.try_iter().for_each(|token| {
            let entity = entities
                .build_entity()
                .with(NetToken::new(token.0), &mut net_token)
                .build();
            sender.send_entity(token, entity);
            if let Err(err) = ss.insert(entity, SelfSender::new(entity.id(), token, sender.clone()))
            {
                log::error!("insert SelfSender failed:{}", err);
            }
        })
    }
}

pub struct InputSystem<T> {
    receiver: Receiver<(Entity, T)>,
}

impl<T> InputSystem<T> {
    pub fn new(receiver: Receiver<(Entity, T)>) -> Self {
        Self { receiver }
    }
}

impl<'a, T> System<'a> for InputSystem<T>
where
    T: Component + Debug,
{
    type SystemData = WriteStorage<'a, T>;

    fn run(&mut self, mut data: Self::SystemData) {
        self.receiver
            .try_iter()
            .for_each(|(entity, t)| match data.insert(entity, t) {
                Ok(t) => {
                    if let Some(t) = t {
                        log::warn!("request:{:?} already exists", t);
                    }
                }
                Err(err) => {
                    log::error!("insert input failed:{}", err);
                }
            });
    }
}

pub struct CloseSystem;

impl<'a> System<'a> for CloseSystem {
    type SystemData = (
        Entities<'a>,
        WriteStorage<'a, Closing>,
        ReadStorage<'a, NetToken>,
        Read<'a, LazyUpdate>,
        Read<'a, BytesSender>,
    );

    fn run(&mut self, (entities, mut closing, tokens, lazy_update, sender): Self::SystemData) {
        let (entities, tokens): (Vec<_>, Vec<_>) = (&entities, &tokens, closing.drain())
            .join()
            .filter_map(|(entity, token, closing)| {
                if closing.0 {
                    log::debug!("entity:{} has shutdown network", entity.id());
                    Some((entity, token.token()))
                } else {
                    log::debug!("entity:{} has invalid data", entity.id());
                    sender.send_close(token.token(), false);
                    None
                }
            })
            .unzip();
        if entities.is_empty() {
            return;
        }

        lazy_update.exec_mut(move |world| {
            if let Err(err) = world.delete_entities(entities.as_slice()) {
                log::error!("delete entities failed:{}", err);
            }
            log::debug!("{} entities deleted", entities.len());
            world.read_resource::<BytesSender>().broadcast_close(tokens);
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
    #[allow(dead_code)]
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
                    let lib = unsafe { &mut *(lib.as_ref() as *const Library as *mut Library) };
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

pub struct CommitChangeSystem<T, B = DummySceneSyncBackend> {
    reader: ReaderId<ComponentEvent>,
    _phantom: PhantomData<(T, B)>,
}

impl<T, B> CommitChangeSystem<T, B>
where
    T: Component + Send + Sync + 'static,
    <T as Component>::Storage: Tracked + Default,
{
    pub fn new(world: &mut World) -> Self {
        let reader = world.write_storage::<T>().register_reader();
        Self {
            reader,
            _phantom: Default::default(),
        }
    }
}

impl<'a, T, B> System<'a> for CommitChangeSystem<T, B>
where
    T: Component + DataSet,
    <T as Component>::Storage: Tracked,
    <T as Deref>::Target: Mask,
    T: DerefMut,
    B: SceneSyncBackend + Send + Sync + 'static,
    <<B as SceneSyncBackend>::Position as Component>::Storage: Tracked + Default,
    <<B as SceneSyncBackend>::SceneData as Component>::Storage: Tracked + Default,
{
    type SystemData = (
        WriteStorage<'a, T>,
        ReadStorage<'a, NetToken>,
        ReadStorage<'a, TeamMember>,
        ReadExpect<'a, TeamHierarchy>,
        Read<'a, BytesSender>,
        Entities<'a>,
        ReadExpect<'a, SceneManager<B>>,
        ReadStorage<'a, AroundFullData>,
        ReadStorage<'a, TeamFullData>,
    );

    fn run(
        &mut self,
        (data, token, teams, hteams, sender, entities, gm, new_scene_member, new_team_member): Self::SystemData,
    ) {
        //log::info!("CommitChangeSystem:{}", std::any::type_name::<T>());
        // 处理有新玩家进入时需要完整数据集的情况
        if T::is_direction_enabled(SyncDirection::Around) {
            for (data, member, entity) in (&data, &new_scene_member, &entities).join() {
                if member.mask().is_empty() {
                    continue;
                }
                log::info!(
                    "entity:{} {} should send full data",
                    entity.id(),
                    std::any::type_name::<T>()
                );
                let mut data = data.clone();
                data.mask_all();
                data.commit();
                if let Some(bytes) = data.encode(entity.id(), SyncDirection::Around) {
                    let tokens = NetToken::tokens(&token, member.mask());
                    sender.broadcast_bytes(tokens, bytes)
                } else {
                    log::warn!("full data synchronization required, but nothing to send");
                }
            }
        }

        if T::is_direction_enabled(SyncDirection::Team) {
            for (data, member, entity) in (&data, &new_team_member, &entities).join() {
                if member.mask().is_empty() {
                    continue;
                }
                log::info!(
                    "entity:{} {} should send full data",
                    entity.id(),
                    std::any::type_name::<T>()
                );
                let mut data = data.clone();
                data.mask_all();
                data.commit();
                if let Some(bytes) = data.encode(entity.id(), SyncDirection::Team) {
                    let tokens = NetToken::tokens(&token, member.mask());
                    sender.broadcast_bytes(tokens, bytes)
                } else {
                    log::warn!("full data synchronization required, but nothing to send");
                }
            }
        }

        let mut inserted = BitSet::new();
        let mut modified = BitSet::new();
        let mut removed = BitSet::new();
        let events = data.channel().read(&mut self.reader);
        events_to_bitsets(events, &mut inserted, &mut modified, &mut removed);

        // 处理针对玩家的数据集
        let mut not_modified = BitSet::new();
        for (data, id) in (&data, &modified).join() {
            if data.is_data_dirty() {
                let data = unsafe { &mut *(data as *const T as *mut T) };
                data.commit();
            } else {
                log::info!("entity:{} {} not changed", id, std::any::type_name::<T>());
                not_modified.add(id);
            }
        }
        modified &= &!&not_modified;

        if T::is_direction_enabled(SyncDirection::Client) {
            for (data, id, token) in (&data, &(&modified | &inserted), &token).join() {
                let data = unsafe { &mut *(data as *const T as *mut T) };
                let bytes = data.encode(id, SyncDirection::Client);
                if let Some(bytes) = bytes {
                    sender.send_bytes(token.token(), bytes);
                }
            }
        }

        // 处理针对组队的数据集
        if T::is_direction_enabled(SyncDirection::Team) {
            for (data, id, team) in (&data, &modified, &teams).join() {
                let data = unsafe { &mut *(data as *const T as *mut T) };
                if let Some(bytes) = data.encode(id, SyncDirection::Team) {
                    let members = hteams.all_children(team.parent_entity());
                    let tokens = NetToken::tokens(&token, &members);
                    sender.broadcast_bytes(tokens, bytes);
                }
            }
        }

        // 处理针对场景的数据集
        if T::is_direction_enabled(SyncDirection::Around) {
            for (data, id, entity, _) in (&data, &modified, &entities, !&new_scene_member).join() {
                let data = unsafe { &mut *(data as *const T as *mut T) };
                if let Some(bytes) = data.encode(id, SyncDirection::Around) {
                    let around = gm.get_user_around(entity.id());
                    let tokens = NetToken::tokens(&token, &around);
                    sender.broadcast_bytes(tokens, bytes)
                }
            }
        }

        if T::is_direction_enabled(SyncDirection::Database) {
            //TODO
        }
    }
}

pub type TeamSystem = HierarchySystem<TeamMember>;
pub type SceneSystem = HierarchySystem<SceneMember>;

pub struct TeamManagerSystem<B> {
    reader: ReaderId<ComponentEvent>,
    mapping: HashMap<u32, Entity>,
    _phantom: PhantomData<B>,
}

impl<B> TeamManagerSystem<B> {
    pub fn new(world: &mut World) -> Self {
        let mut storage = world.write_storage::<TeamMember>();
        let reader = storage.register_reader();
        Self {
            reader,
            mapping: Default::default(),
            _phantom: Default::default(),
        }
    }
}

impl<'a, B> System<'a> for TeamManagerSystem<B>
where
    B: SceneSyncBackend + Send + Sync + 'static,
    <<B as SceneSyncBackend>::Position as Component>::Storage: Tracked + Default,
    <<B as SceneSyncBackend>::SceneData as Component>::Storage: Tracked + Default,
{
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, TeamMember>,
        ReadExpect<'a, TeamHierarchy>,
        WriteStorage<'a, TeamFullData>,
        ReadStorage<'a, NetToken>,
        ReadExpect<'a, BytesSender>,
    );

    fn run(&mut self, (entities, tm, th, mut tfd, tokens, sender): Self::SystemData) {
        let events = tm.channel().read(&mut self.reader);
        let mut inserted = BitSet::new();
        let mut modified = BitSet::new();
        let mut removed = BitSet::new();
        events_to_bitsets(events, &mut inserted, &mut modified, &mut removed);
        for (entity, tm, _) in (&entities, &tm, &inserted).join() {
            self.mapping.insert(entity.id(), tm.parent_entity());
            let mut members = th.all_children(tm.parent_entity());
            members.remove(entity.id());
            tfd.get_mut_or_default(entity).unwrap().add_mask(&members);
            let id = entity.id();
            for (entity, _) in (&entities, &members).join() {
                tfd.get_mut_or_default(entity).unwrap().add(id);
            }
        }
        for id in removed {
            if let Some(parent) = self.mapping.remove(&id) {
                let members = th.all_children(parent);
                let mut drop_entity = B::DropEntity::default();
                drop_entity.add_set(&members);
                let tokens = NetToken::tokens(&tokens, &members);
                sender.broadcast_data(tokens, 0, drop_entity);
            }
        }
    }
}

pub struct GridSystem<B> {
    _phantom: PhantomData<B>,
}

impl<B> GridSystem<B>
where
    B: SceneSyncBackend + Send + Sync + 'static,
    <<B as SceneSyncBackend>::Position as Component>::Storage: Tracked + Default,
    <<B as SceneSyncBackend>::SceneData as Component>::Storage: Tracked + Default,
{
    pub fn new(world: &mut World) -> Self {
        if !world.has_value::<SceneManager<B>>() {
            let gm = {
                let mut p_storage = world.write_storage::<B::Position>();
                let mut s_storage = world.write_storage::<B::SceneData>();
                SceneManager::<B>::new(p_storage.register_reader(), s_storage.register_reader())
            };
            world.insert(gm);
        }
        Self {
            _phantom: Default::default(),
        }
    }
}

impl<'a, B> System<'a> for GridSystem<B>
where
    B: SceneSyncBackend + Send + Sync + 'static,
    <<B as SceneSyncBackend>::Position as Component>::Storage: Tracked + Default,
    <<B as SceneSyncBackend>::SceneData as Component>::Storage: Tracked + Default,
{
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, B::Position>,
        ReadStorage<'a, SceneMember>,
        ReadStorage<'a, B::SceneData>,
        WriteExpect<'a, SceneManager<B>>,
        WriteStorage<'a, AroundFullData>,
        ReadStorage<'a, NetToken>,
        Read<'a, BytesSender>,
    );

    fn run(
        &mut self,
        (
            entities,
            positions,
            scene,
            scene_data,
            mut sm,
            new_scene_member,
            tokens,
            sender,
        ): Self::SystemData,
    ) {
        //log::info!("GridSystem");
        sm.maintain(
            entities,
            positions,
            scene,
            scene_data,
            new_scene_member,
            tokens,
            sender,
        );
    }
}

pub trait GameSystem<'a> {
    type SystemData: SystemData<'a>;

    fn run(&mut self, data: Self::SystemData);
}

impl<'a, T: ?Sized> GameSystem<'a> for T
where
    T: System<'a>,
    <T as System<'a>>::SystemData: SystemData<'a>,
{
    type SystemData = <T as System<'a>>::SystemData;

    fn run(&mut self, data: Self::SystemData) {
        System::run(self, data);
    }
}

pub struct StatisticSystem<T>(pub String, pub T);

impl<'a, T> System<'a> for StatisticSystem<T>
where
    T: GameSystem<'a> + System<'a>,
{
    type SystemData = (
        ReadExpect<'a, TimeStatistic>,
        <T as GameSystem<'a>>::SystemData,
    );

    fn run(&mut self, (ts, data): Self::SystemData) {
        let begin = UNIX_EPOCH.elapsed().unwrap();
        GameSystem::run(&mut self.1, data);
        let end = UNIX_EPOCH.elapsed().unwrap();
        ts.add_time(self.0.clone(), begin, end);
    }
}

pub struct StatisticRunNow<T>(pub String, pub T);

impl<'a, T> RunNow<'a> for StatisticRunNow<T>
where
    T: RunNow<'a>,
{
    fn run_now(&mut self, world: &'a World) {
        let ts = world.read_resource::<TimeStatistic>();
        let begin = UNIX_EPOCH.elapsed().unwrap();
        self.1.run_now(world);
        let end = UNIX_EPOCH.elapsed().unwrap();
        ts.add_time(self.0.clone(), begin, end);
    }

    fn setup(&mut self, world: &mut World) {
        self.1.setup(world);
    }
}

pub struct PrintStatisticSystem;

impl<'a> System<'a> for PrintStatisticSystem {
    type SystemData = (Read<'a, FrameCounter>, ReadExpect<'a, TimeStatistic>);

    fn run(&mut self, (frame, data): Self::SystemData) {
        data.print(frame.frame(), frame.fps());
        data.clear();
    }
}

#[derive(Default)]
pub struct CleanStorageSystem<T> {
    sender: Option<Sender<Vec<Entity>>>,
    _phantom: PhantomData<T>,
}

impl<T> CleanStorageSystem<T> {
    pub fn new(sender: Sender<Vec<Entity>>) -> Self {
        Self {
            sender: Some(sender),
            _phantom: Default::default(),
        }
    }
}

impl<'a, T> System<'a> for CleanStorageSystem<T>
where
    T: Component,
{
    type SystemData = (Entities<'a>, WriteStorage<'a, T>);

    fn run(&mut self, (entities, mut data): Self::SystemData) {
        //log::info!("CleanStorageSystem:{}", std::any::type_name::<T>());
        let entities = (&entities, data.drain())
            .join()
            .map(|(entity, _)| entity)
            .collect();
        if let Some(sender) = &self.sender {
            if let Err(err) = sender.send(entities) {
                log::error!("send next ticket to decode failed:{}", err);
            }
        }
    }
}
