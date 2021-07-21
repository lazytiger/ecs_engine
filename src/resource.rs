use crate::{
    backend::DropEntity,
    component::{AroundFullData, Position, SceneData, SceneMember, TeamMember},
    events_to_bitsets, BytesSender, NetToken, SceneSyncBackend,
};
use specs::{
    hibitset::BitSetLike, prelude::ComponentEvent, storage::GenericWriteStorage, BitSet, Component,
    Entities, Entity, Join, Read, ReadExpect, ReadStorage, ReaderId, Tracked, WriteStorage,
};
use specs_hierarchy::{Hierarchy, HierarchyEvent, Parent};
use std::{
    collections::HashMap,
    fmt::Write,
    marker::PhantomData,
    sync::Mutex,
    time::{Duration, Instant},
};

pub struct TimeStatistic {
    times: Mutex<HashMap<String, (Duration, Duration)>>,
}

impl TimeStatistic {
    pub fn new() -> Self {
        Self {
            times: Default::default(),
        }
    }

    pub fn add_time(&self, name: String, begin: Duration, end: Duration) {
        self.times.lock().unwrap().insert(name, (begin, end));
    }

    pub fn print(&self, frame: usize, fps: usize) {
        let mut buffer = bytes::BytesMut::new();
        write!(buffer, "frame:{}, fps:{},", frame, fps).unwrap();
        let times = self.times.lock().unwrap();
        for (name, (begin, end)) in times.iter() {
            write!(
                buffer,
                " system {} begin at {:?}, cost:{},",
                name,
                begin,
                end.as_micros() - begin.as_micros()
            )
            .unwrap();
        }
        log::info!("{}", String::from_utf8(buffer.to_vec()).unwrap());
    }

    pub fn clear(&self) {
        self.times.lock().unwrap().clear();
    }
}

pub struct FrameCounter {
    time: Instant,
    delta: Duration,
    frame: usize,
}

impl Default for FrameCounter {
    fn default() -> Self {
        Self {
            time: Instant::now(),
            delta: Duration::from_millis(1),
            frame: 0,
        }
    }
}

impl FrameCounter {
    pub fn next_frame(&mut self) {
        self.delta = self.time.elapsed();
        self.time = Instant::now();
        self.frame += 1;
    }

    pub fn frame(&self) -> usize {
        self.frame
    }

    pub fn fps(&self) -> usize {
        let delta = self.delta.as_millis() as usize;
        if delta == 0 {
            1000
        } else {
            1000 / delta
        }
    }
}

pub struct SceneManager<B>
where
    B: SceneSyncBackend,
    <<B as SceneSyncBackend>::Position as Component>::Storage: Tracked + Default,
    <<B as SceneSyncBackend>::SceneData as Component>::Storage: Tracked + Default,
{
    position_reader: ReaderId<ComponentEvent>,
    scene_reader: ReaderId<ComponentEvent>,
    _phantom: PhantomData<B>,
    /// mapping from entity to grid index
    user_grids: HashMap<u32, (Entity, usize)>,
    /// mapping from scene to grids
    scene_grids: HashMap<u32, HashMap<usize, BitSet>>,
    scene_data: HashMap<u32, B::SceneData>,
    scene_mapping: HashMap<u32, Entity>,
}

impl<B> SceneManager<B>
where
    B: SceneSyncBackend,
    <<B as SceneSyncBackend>::Position as Component>::Storage: Tracked + Default,
    <<B as SceneSyncBackend>::SceneData as Component>::Storage: Tracked + Default,
{
    pub fn new(
        position_reader: ReaderId<ComponentEvent>,
        scene_reader: ReaderId<ComponentEvent>,
    ) -> Self {
        Self {
            position_reader,
            scene_reader,
            _phantom: Default::default(),
            user_grids: Default::default(),
            scene_grids: Default::default(),
            scene_data: Default::default(),
            scene_mapping: Default::default(),
        }
    }

    fn drop_entities<'a>(
        entity: u32,
        set: BitSet,
        storage: &ReadStorage<'a, NetToken>,
        sender: &BytesSender,
    ) {
        if set.is_empty() {
            return;
        }

        let tokens = NetToken::tokens(storage, &set);
        let mut drop_entity = B::DropEntity::default();
        drop_entity.add(entity);
        sender.broadcast_data(tokens, entity, drop_entity);
    }

    fn add_full_data_commit<'a>(
        entity: Entity,
        set: BitSet,
        storage: &mut WriteStorage<'a, AroundFullData>,
        entities: &Entities<'a>,
    ) {
        let afdc = storage.get_mut_or_default(entity).unwrap();
        afdc.add_mask(&set);
        let id = entity.id();
        for (entity, _) in (entities, &set).join() {
            storage.get_mut_or_default(entity).unwrap().add(id);
        }
    }

    pub(crate) fn maintain<'a>(
        &mut self,
        entities: Entities<'a>,
        positions: ReadStorage<'a, B::Position>,
        scene: ReadStorage<'a, SceneMember>,
        scene_data: ReadStorage<'a, B::SceneData>,
        mut new_scene_member: WriteStorage<'a, AroundFullData>,
        tokens: ReadStorage<'a, NetToken>,
        sender: Read<'a, BytesSender>,
    ) {
        let mut modified = BitSet::default();
        let mut inserted = BitSet::default();
        let mut removed = BitSet::default();
        let events = scene_data.channel().read(&mut self.scene_reader);
        events_to_bitsets(events, &mut inserted, &mut modified, &mut removed);
        for id in &removed {
            self.scene_data.remove(&id);
            self.scene_mapping.remove(&id);
        }
        for (data, id) in (&scene_data, &inserted).join() {
            self.scene_data.insert(id, data.clone());
        }
        inserted.clear();
        removed.clear();
        modified.clear();

        let events = positions.channel().read(&mut self.position_reader);
        events_to_bitsets(events, &mut inserted, &mut modified, &mut removed);

        for id in &removed {
            let around = self.get_user_around(id);
            Self::drop_entities(id, around, &tokens, &sender);
            self.remove_grid_entity(id);
            log::info!("entity:{} removed from scene", id);
        }

        for (entity, pos, scene, _id) in (&entities, &positions, &scene, &inserted).join() {
            let parent = scene.parent_entity();
            if let Some(sd) = scene_data.get(parent) {
                if let Some(index) = sd.grid_index(pos.x(), pos.y()) {
                    self.insert_grid_entity(parent, entity, index);
                    let around = self.get_user_around(entity.id());
                    Self::add_full_data_commit(entity, around, &mut new_scene_member, &entities);
                } else {
                    log::error!(
                        "invalid position:[{},{}] for scene:{}",
                        pos.x(),
                        pos.y(),
                        parent.id()
                    );
                }
            } else {
                log::error!("scene:{} not found", parent.id());
            }
        }

        for (entity, pos, id) in (&entities, &positions, &modified).join() {
            if let Some((parent, index)) = self
                .user_grids
                .get(&id)
                .map(|(parent, index)| (*parent, *index))
            {
                if let Some(sd) = scene_data.get(parent) {
                    if let Some(new_index) = sd.grid_index(pos.x(), pos.y()) {
                        if index == new_index {
                            continue;
                        }
                        let (removed, _, inserted) = sd.diff(index, new_index);
                        let inserted = self.get_user_grids(&entity, inserted);
                        Self::add_full_data_commit(
                            entity,
                            inserted,
                            &mut new_scene_member,
                            &entities,
                        );

                        let removed = self.get_user_grids(&entity, removed);
                        Self::drop_entities(entity.id(), removed, &tokens, &sender);
                        self.insert_grid_entity(parent, entity, new_index);
                    } else {
                        log::error!(
                            "invalid position:[{}, {}] for scene:{}",
                            pos.x(),
                            pos.y(),
                            parent.id()
                        );
                    }
                } else {
                    log::error!("scene data {} not found in manager", parent.id());
                }
            } else {
                log::error!("entity:{} not found in user grid", entity.id());
            }
        }

        let empty_scene: Vec<_> = self
            .scene_grids
            .iter()
            .filter_map(|(entity, grids)| {
                if grids.iter().any(|(_, grid)| !grid.is_empty()) {
                    None
                } else {
                    Some(*entity)
                }
            })
            .collect();
        empty_scene.iter().for_each(|id| {
            log::info!("scene:{} deleted", id);
            let entity = entities.entity(*id);
            if entities.is_alive(entity) {
                if let Err(err) = entities.delete(entity) {
                    log::error!("delete entity:{} failed:{}", entity.id(), err);
                }
                self.scene_grids.remove(id);
            }
        });

        //log::info!("grid system cost:{}us", begin.elapsed().as_micros());
    }

    fn insert_grid_entity(&mut self, parent: Entity, entity: Entity, index: usize) {
        self.remove_grid_entity(entity.id());
        if !self.scene_grids.contains_key(&parent.id()) {
            self.scene_grids.insert(parent.id(), Default::default());
        }
        let grids = self.scene_grids.get_mut(&parent.id()).unwrap();
        if !grids.contains_key(&index) {
            grids.insert(index, Default::default());
        }
        let grid = grids.get_mut(&index).unwrap();
        if grid.add(entity.id()) {
            log::error!("entity:{} already in grid", entity.id());
        }
        self.user_grids.insert(entity.id(), (parent, index));
        log::info!(
            "entity:{} insert into scene:{} grid:{}",
            entity.id(),
            parent.id(),
            index
        );
    }

    pub fn get_scene_data(&self, entity: Entity) -> Option<&B::SceneData> {
        self.scene_data.get(&entity.id())
    }

    fn remove_grid_entity(&mut self, id: u32) {
        if let Some((parent, index)) = self.user_grids.remove(&id) {
            if let Some(scene_grid) = self.scene_grids.get_mut(&parent.id()) {
                if let Some(grid) = scene_grid.get_mut(&index) {
                    if !grid.remove(id) {
                        log::warn!("entity {} not found in set", id);
                    }
                }
            }
        }
    }

    fn get_scene_around(&self, parent: &Entity, index: usize) -> BitSet {
        let mut set = BitSet::new();
        if let Some(sd) = self.scene_data.get(&parent.id()) {
            if let Some(grids) = self.scene_grids.get(&parent.id()) {
                for index in sd.around(index) {
                    if let Some(grid) = grids.get(&index) {
                        set |= grid;
                    }
                }
            }
        }
        set
    }

    fn get_user_grids(&self, entity: &Entity, indexes: Vec<usize>) -> BitSet {
        let mut set = BitSet::new();
        if let Some((parent, _)) = self.user_grids.get(&entity.id()) {
            if let Some(grids) = self.scene_grids.get(&parent.id()) {
                for index in indexes {
                    if let Some(grid) = grids.get(&index) {
                        set |= grid;
                    }
                }
            }
            set.remove(entity.id());
        }
        set
    }

    pub fn get_user_around(&self, entity: u32) -> BitSet {
        if let Some((parent, index)) = self.user_grids.get(&entity) {
            let mut bitset = self.get_scene_around(parent, *index);
            bitset.remove(entity);
            bitset
        } else {
            BitSet::new()
        }
    }

    pub fn insert_scene(&mut self, id: u32, entity: Entity) {
        if self.scene_mapping.insert(id, entity).is_some() {
            log::error!("scene:{} already inserted", id);
        }
    }

    pub fn get_scene_entity(&self, id: u32) -> Option<Entity> {
        self.scene_mapping.get(&id).map(|entity| *entity)
    }
}
pub type TeamHierarchy = Hierarchy<TeamMember>;
pub type SceneHierarchy = Hierarchy<SceneMember>;
