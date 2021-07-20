use crate::{
    backend::DropEntity,
    component::{AroundFullData, Position, SceneData, SceneMember, TeamMember},
    BytesSender, NetToken, SceneSyncBackend,
};
use specs::{
    hibitset::BitSetLike, prelude::ComponentEvent, storage::GenericWriteStorage, BitSet, Component,
    Entities, Entity, Join, Read, ReadExpect, ReadStorage, ReaderId, Tracked, WriteStorage,
};
use specs_hierarchy::{Hierarchy, HierarchyEvent, Parent};
use std::{
    collections::HashMap,
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
        let times = self.times.lock().unwrap();
        for (name, (begin, end)) in times.iter() {
            log::info!(
                "frame:{}, fps:{}, system {} begin at {:?}, cost:{}",
                frame,
                fps,
                name,
                begin,
                end.as_micros() - begin.as_micros()
            );
        }
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
    hierarchy_reader: ReaderId<HierarchyEvent>,
    _phantom: PhantomData<B>,
    /// mapping from entity to grid index
    user_grids: HashMap<u32, (Entity, usize)>,
    /// mapping from scene to grids
    scene_grids: HashMap<u32, HashMap<usize, (usize, BitSet)>>,
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
            scene_mapping: Default::default(),
        }
    }

    pub(crate) fn maintain<'a>(
        &mut self,
        entities: Entities<'a>,
        positions: ReadStorage<'a, B::Position>,
        scene: ReadStorage<'a, SceneMember>,
        scene_data: ReadStorage<'a, B::SceneData>,
        scene_hierarchy: ReadExpect<'a, SceneHierarchy>,
        mut new_scene_member: WriteStorage<'a, AroundFullData>,
        tokens: ReadStorage<'a, NetToken>,
        sender: Read<'a, BytesSender>,
    ) {
        let begin = Instant::now();
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
                    self.scene_data.remove(id);
                    self.scene_mapping.remove(id);
                }
            }
        }
        for (data, id) in (&scene_data, &inserted).join() {
            self.scene_data.insert(id, data.clone());
        }
        inserted.clear();
        removed.clear();

        let events = scene_hierarchy.changed().read(&mut self.hierarchy_reader);
        for event in events {
            match event {
                HierarchyEvent::Removed(entity) => {
                    removed.add(entity.id());
                }
                _ => {}
            }
        }

        let events = positions.channel().read(&mut self.position_reader);
        for event in events {
            match event {
                ComponentEvent::Modified(id) => {
                    modified.add(*id);
                }
                ComponentEvent::Inserted(id) => {
                    inserted.add(*id);
                }
                ComponentEvent::Removed(id) => {
                    removed.add(*id);
                }
            }
        }

        for (entity, removed) in (&entities, &removed).join() {
            let around = self.get_user_around(entity);
            if !around.is_empty() {
                let tokens = NetToken::tokens(&tokens, &around);
                let mut drop_entity = B::DropEntity::default();
                drop_entity.add(removed);
                sender.broadcast_data(tokens, 0, drop_entity);
                self.remove_grid_entity(removed);
            }
        }

        for (entity, pos, scene, id) in (&entities, &positions, &scene, &inserted).join() {
            let parent = scene.parent_entity();
            if let Some(sd) = scene_data.get(parent) {
                if let Some(index) = sd.grid_index(pos.x(), pos.y()) {
                    self.insert_grid_entity(parent, entity, index);
                    log::info!(
                        "entity:{} insert into scene:{} grid {}",
                        entity.id(),
                        parent.id(),
                        index
                    );
                    let afdc = new_scene_member.get_mut_or_default(entity).unwrap();
                    let around = self.get_user_around(entity);
                    afdc.add_mask(&around);
                    for (entity, _) in (&entities, &around).join() {
                        new_scene_member.get_mut_or_default(entity).unwrap().add(id);
                    }
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
                        log::info!(
                            "entity:{} enter scene:{} grid:{}",
                            entity.id(),
                            parent.id(),
                            new_index
                        );
                        let (removed, _, inserted) = sd.diff(index, new_index);
                        let inserted = self.get_user_grids(&entity, inserted);
                        let afdc = new_scene_member.get_mut_or_default(entity).unwrap();
                        afdc.add_mask(&inserted);
                        for (entity, _) in (&entities, &inserted).join() {
                            let afdc = new_scene_member.get_mut_or_default(entity).unwrap();
                            afdc.add(id);
                        }

                        let removed = self.get_user_grids(&entity, removed);
                        if !removed.is_empty() {
                            let tokens = NetToken::tokens(&tokens, &removed);
                            let mut drop_entity = B::DropEntity::default();
                            drop_entity.add(id);
                            sender.broadcast_data(tokens, 0, drop_entity);
                        }
                        self.remove_grid_entity(id);
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
                if 0 == grids.iter().fold(0, |count, (_, (size, _))| count + size) {
                    Some(*entity)
                } else {
                    None
                }
            })
            .collect();
        empty_scene.iter().for_each(|entity| {
            self.scene_grids.remove(entity);
        });

        //log::info!("grid system cost:{}us", begin.elapsed().as_micros());
    }

    fn insert_grid_entity(&mut self, parent: Entity, entity: Entity, index: usize) {
        if !self.scene_grids.contains_key(&parent.id()) {
            self.scene_grids.insert(parent.id(), Default::default());
        }
        let grids = self.scene_grids.get_mut(&parent.id()).unwrap();
        if !grids.contains_key(&index) {
            grids.insert(index, Default::default());
        }
        let (count, grid) = grids.get_mut(&index).unwrap();
        if grid.add(entity.id()) {
            log::error!("entity:{} already in grid", entity.id());
        } else {
            *count += 1;
        }
        self.user_grids.insert(entity.id(), (parent, index));
    }

    pub fn get_scene_data(&self, entity: Entity) -> Option<&B::SceneData> {
        self.scene_data.get(&entity.id())
    }

    fn remove_grid_entity(&mut self, id: u32) {
        log::info!("entity:{} removed from manager", id);
        if let Some((parent, index)) = self.user_grids.remove(&id) {
            if let Some(scene_grid) = self.scene_grids.get_mut(&parent.id()) {
                if let Some((count, grid)) = scene_grid.get_mut(&index) {
                    if !grid.remove(id) {
                        log::warn!("entity {} not found in set", id);
                    } else {
                        *count -= 1;
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
                    if let Some((_, grid)) = grids.get(&index) {
                        set |= grid;
                    }
                }
            }
        }
        set
    }

    fn get_user_grids(&self, entity: &Entity, indexes: Vec<usize>) -> BitSet {
        let mut set = BitSet::new();
        if let Some((parent, _)) = self.user_grids.get((&entity.id())) {
            if let Some(grids) = self.scene_grids.get(&parent.id()) {
                for index in indexes {
                    if let Some((_, grid)) = grids.get(&index) {
                        set |= grid;
                    }
                }
            }
            set.remove(entity.id());
        }
        set
    }

    pub fn get_user_around(&self, entity: Entity) -> BitSet {
        if let Some((parent, index)) = self.user_grids.get(&entity.id()) {
            let mut bitset = self.get_scene_around(parent, *index);
            bitset.remove(entity.id());
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
