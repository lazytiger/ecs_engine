use crate::{
    component::{Closing, Member},
    dynamic::Library,
    network::{BytesSender, RequestData, ResponseSender},
    sync::ChangeSet,
    DataSet, DynamicManager, Input, NetToken, RequestIdent, SyncDirection,
};
use crossbeam::channel::Receiver;
use notify::{DebouncedEvent, RecommendedWatcher, RecursiveMode, Watcher};
use specs::{
    storage::ComponentEvent, BitSet, Component, Entities, Entity, Join, LazyUpdate, Read,
    ReadExpect, ReadStorage, ReaderId, RunNow, System, Tracked, World, WorldExt, WriteExpect,
    WriteStorage,
};
use specs_hierarchy::{Hierarchy, HierarchyEvent, HierarchySystem, Parent};
use std::{collections::HashMap, marker::PhantomData, path::PathBuf, time::Duration};

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

fn get_library_name(path: PathBuf) -> Option<String> {
    if let Some(file_name) = path.file_name() {
        if let Some(file_name) = file_name.to_str() {
            if file_name.starts_with(std::env::consts::DLL_PREFIX)
                && file_name.ends_with(std::env::consts::DLL_SUFFIX)
            {
                let begin = std::env::consts::DLL_PREFIX.len();
                let end = file_name.len() - std::env::consts::DLL_SUFFIX.len();
                return Some(file_name[begin..end].into());
            }
        }
    }
    None
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
    );

    fn run(&mut self, (mut data, token, teams, hteams, sender, entities, gm): Self::SystemData) {
        self.counter += 1;
        if self.counter != self.tick_step {
            return;
        } else {
            self.counter = 0;
        }
        if !T::is_storage_dirty() {
            return;
        }

        let mut modified = BitSet::new();
        for (data, token, entity) in (&mut data, &token, &entities).join() {
            if data.is_dirty() {
                data.commit();
                let bytes = data.encode(SyncDirection::Client);
                if let Some(bytes) = bytes {
                    sender.send_bytes(token.token(), bytes);
                }
                modified.add(entity.id());
            }
        }

        for (data, _, team) in (&mut data, &modified, &teams).join() {
            if let Some(bytes) = data.encode(SyncDirection::Team) {
                let members = hteams.all_children(team.parent_entity());
                let tokens = NetToken::tokens(&token, &members);
                sender.broadcast_bytes(tokens, bytes);
            }
        }

        for (data, _, entity) in (&mut data, &modified, &entities).join() {
            if let Some(bytes) = data.encode(SyncDirection::Around) {
                let around = gm.get_around(entity);
                let tokens = NetToken::tokens(&token, &around);
                sender.broadcast_bytes(tokens, bytes)
            }
        }

        T::clear_storage_dirty();
    }
}

pub type TeamMember = Member<0>;
pub type SceneMember = Member<1>;
pub type TeamSystem = HierarchySystem<TeamMember>;
pub type TeamHierarchy = Hierarchy<TeamMember>;
pub type SceneSystem = HierarchySystem<SceneMember>;
pub type SceneHierarchy = Hierarchy<SceneMember>;

/// 玩家的位置信息
pub trait Position {
    /// x轴坐标
    fn x(&self) -> f32;
    /// y轴坐标
    fn y(&self) -> f32;
}

/// 场景尺寸信息
pub trait SceneData: Clone {
    /// 场景坐标的最小xy值
    fn get_min_xy(&self) -> (f32, f32);
    /// 获取场景的分块尺寸，即可以分为行列数
    fn get_size(&self) -> (i32, i32);
    /// 场景分隔的正方形边长
    fn grid_size(&self) -> f32;
    /// 根据位置信息计算格子索引
    /// index = y * column + x
    fn grid_index(&self, p: &impl Position) -> usize {
        let x = p.x();
        let y = p.y();
        let (min_x, min_y) = self.get_min_xy();
        let x = ((x - min_x) * 100.0) as i32;
        let y = ((y - min_y) * 100.0) as i32;
        let grid_size = (self.grid_size() * 100.0) as i32;
        let x = x / grid_size;
        let y = y / grid_size;
        let (_, column) = self.get_size();
        (y * column + x) as usize
    }
    /// 获取周围格子的索引，包括当前格子
    fn around(&self, index: usize) -> Vec<usize> {
        let mut data = Vec::new();
        let index = index as i32;
        let (row, column) = self.get_size();
        let x = index % column;
        let y = index / column;
        for i in [-1, 0, 1] {
            let xx = x + i;
            if xx < 0 || xx >= column {
                continue;
            }
            for j in [-1, 0, 1] {
                let yy = y + j;
                if yy < 0 || yy >= row {
                    continue;
                }
                data.push((yy * column + xx) as usize)
            }
        }
        data
    }
}

pub struct GridSystem<P, S> {
    _phantom: PhantomData<(P, S)>,
}

pub struct GridManager<P, S> {
    position_reader: ReaderId<ComponentEvent>,
    scene_reader: ReaderId<ComponentEvent>,
    hierarchy_reader: ReaderId<HierarchyEvent>,
    _phantom: PhantomData<P>,
    /// mapping from entity to grid index
    user_grids: HashMap<Entity, (Entity, usize)>,
    /// mapping from scene to grids
    scene_grids: HashMap<Entity, HashMap<usize, BitSet>>,
    scene_data: HashMap<Entity, S>,
}

impl<P, S> GridManager<P, S>
where
    P: Component + Position,
    P::Storage: Tracked,
    S: Component + SceneData,
    S::Storage: Tracked,
{
    pub fn new(
        position_reader: ReaderId<ComponentEvent>,
        scene_reader: ReaderId<ComponentEvent>,
        hierarchy_reader: ReaderId<HierarchyEvent>,
    ) -> Self {
        Self {
            position_reader,
            scene_reader,
            hierarchy_reader,
            _phantom: Default::default(),
            user_grids: Default::default(),
            scene_grids: Default::default(),
            scene_data: Default::default(),
        }
    }

    fn maintain<'a>(
        &mut self,
        entities: Entities<'a>,
        positions: ReadStorage<'a, P>,
        scene: ReadStorage<'a, SceneMember>,
        scene_data: ReadStorage<'a, S>,
        scene_hierarchy: ReadExpect<'a, SceneHierarchy>,
    ) {
        let mut modified = BitSet::default();
        let mut inserted = BitSet::default();
        let mut removed = BitSet::default();
        let events = scene_data.channel().read(&mut self.scene_reader);
        for event in events {
            match event {
                ComponentEvent::Inserted(id) => {
                    inserted.add(*id);
                }
                ComponentEvent::Modified(_) => {}
                ComponentEvent::Removed(id) => {
                    removed.add(*id);
                }
            }
        }
        for (entity, _) in (&entities, &removed).join() {
            self.scene_data.remove(&entity);
        }
        for (entity, data, _) in (&entities, &scene_data, &inserted).join() {
            self.scene_data.insert(entity, data.clone());
        }
        inserted.clear();
        removed.clear();

        let events = scene_hierarchy.changed().read(&mut self.hierarchy_reader);
        for event in events {
            match event {
                HierarchyEvent::Modified(entity) | HierarchyEvent::Removed(entity) => {
                    modified.add(entity.id());
                }
            }
        }
        for (_, entity) in (&modified, &entities).join() {
            self.remove_grid_entity(entity);
        }
        modified.clear();

        let events = positions.channel().read(&mut self.position_reader);
        for event in events {
            match event {
                ComponentEvent::Modified(id) => {
                    modified.add(*id);
                }
                ComponentEvent::Inserted(id) => {
                    inserted.add(*id);
                }
                ComponentEvent::Removed(_) => {}
            }
        }

        for (entity, pos, scene, _) in (&entities, &positions, &scene, &inserted).join() {
            let parent = scene.parent_entity();
            if let Some(sd) = scene_data.get(parent) {
                let index = sd.grid_index(pos);
                self.insert_grid_entity(parent, entity, index);
            } else {
                log::error!("scene not found");
            }
        }

        for (entity, pos, _) in (&entities, &positions, &modified).join() {
            if let Some((parent, index)) = self
                .user_grids
                .get(&entity)
                .map(|(parent, index)| (*parent, *index))
            {
                if let Some(sd) = scene_data.get(parent) {
                    let new_index = sd.grid_index(pos);
                    if index == new_index {
                        continue;
                    }
                    if let Some(grids) = self.scene_grids.get_mut(&parent) {
                        if let Some(grid) = grids.get_mut(&index) {
                            grid.remove(entity.id());
                        }
                    } else {
                        log::error!("position modified, but grids not found in manager");
                    }
                    self.insert_grid_entity(parent, entity, new_index);
                } else {
                    log::error!("position modified, but scene data not found in manager");
                }
            } else {
                log::error!("position modified, but grid index not found in manager");
                continue;
            }
        }

        let empty_scene: Vec<_> = self
            .scene_grids
            .iter()
            .filter_map(|(entity, grids)| {
                if 0 == grids
                    .iter()
                    .fold(0, |count, (_, grid)| count + grid.layer0_as_slice().len())
                {
                    Some(*entity)
                } else {
                    None
                }
            })
            .collect();
        empty_scene.iter().for_each(|entity| {
            self.scene_grids.remove(entity);
        });
    }

    fn insert_grid_entity(&mut self, parent: Entity, entity: Entity, index: usize) {
        if !self.scene_grids.contains_key(&parent) {
            self.scene_grids.insert(parent, Default::default());
        }
        let grids = self.scene_grids.get_mut(&parent).unwrap();
        if !grids.contains_key(&index) {
            grids.insert(index, Default::default());
        }
        let grid = grids.get_mut(&index).unwrap();
        if grid.add(entity.id()) {
            log::error!("entity:{} already in grid", entity.id());
        }
        self.user_grids.insert(entity, (parent, index));
    }

    fn remove_grid_entity(&mut self, entity: Entity) {
        if let Some((parent, index)) = self.user_grids.remove(&entity) {
            if let Some(scene_grid) = self.scene_grids.get_mut(&parent) {
                if let Some(grid) = scene_grid.get_mut(&index) {
                    grid.remove(entity.id());
                }
            }
        }
    }

    pub fn get_around(&self, entity: Entity) -> BitSet {
        let mut set = BitSet::new();
        if let Some((parent, index)) = self.user_grids.get(&entity) {
            if let Some(sd) = self.scene_data.get(parent) {
                if let Some(grids) = self.scene_grids.get(parent) {
                    for index in sd.around(*index) {
                        if let Some(grid) = grids.get(&index) {
                            set |= grid;
                        }
                    }
                }
            }
        }
        set
    }
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
    );

    fn run(
        &mut self,
        (entities, positions, scene, scene_data, mut gm, hierarchy): Self::SystemData,
    ) {
        gm.maintain(entities, positions, scene, scene_data, hierarchy);
    }
}
