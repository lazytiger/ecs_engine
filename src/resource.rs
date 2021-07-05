use crate::component::{Position, SceneData, SceneMember, TeamMember};
use specs::{
    prelude::ComponentEvent, BitSet, Component, Entities, Entity, Join, ReadExpect, ReadStorage,
    ReaderId, Tracked,
};
use specs_hierarchy::{Hierarchy, HierarchyEvent, Parent};
use std::{collections::HashMap, marker::PhantomData, sync::Mutex, time::Duration};

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

    pub fn print(&self) {
        let times = self.times.lock().unwrap();
        for (name, (begin, end)) in times.iter() {
            println!(
                "system {} begin at {:?}, cost:{}",
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

    pub(crate) fn maintain<'a>(
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
pub type TeamHierarchy = Hierarchy<TeamMember>;
pub type SceneHierarchy = Hierarchy<SceneMember>;
