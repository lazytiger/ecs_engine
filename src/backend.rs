use crate::{Position, RequestIdent, SceneData};
use byteorder::{BigEndian, ByteOrder};
use crossbeam::channel::Receiver;
use protobuf::{
    reflect::MessageDescriptor, Clear, CodedInputStream, CodedOutputStream, Message,
    ProtobufResult, UnknownFields,
};
use specs::{Component, Entity, FlaggedStorage, NullStorage, Tracked, World, WorldExt};
use std::{any::Any, ops::Deref};

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

pub trait DropEntity: Output + Default {
    fn add(&mut self, id: u32) {
        self.mut_entities().push(id);
    }
    fn add_set(&mut self, set: impl IntoIterator<Item = u32>) {
        self.mut_entities().extend(set);
    }
    fn mut_entities(&mut self) -> &mut Vec<u32>;
}

pub trait SceneSyncBackend
where
    <<Self as SceneSyncBackend>::Position as Component>::Storage: Tracked + Default,
    <<Self as SceneSyncBackend>::SceneData as Component>::Storage: Tracked + Default,
{
    type Position: Position + Component;
    type SceneData: SceneData + Component + Send + Sync;
    type DropEntity: DropEntity;
    fn setup(world: &mut World) {
        world.register::<Self::SceneData>();
        world.register::<Self::Position>();
    }
}

pub struct DummySceneSyncBackend;

impl SceneSyncBackend for DummySceneSyncBackend {
    type Position = DummyPosition;
    type SceneData = DummySceneData;
    type DropEntity = DummyDropEntity;
}

#[derive(Default)]
pub struct DummyPosition;
impl Position for DummyPosition {
    fn x(&self) -> f32 {
        todo!()
    }

    fn y(&self) -> f32 {
        todo!()
    }
}

impl Component for DummyPosition {
    type Storage = FlaggedStorage<Self, NullStorage<Self>>;
}

#[derive(Clone, Default)]
pub struct DummySceneData;

impl SceneData for DummySceneData {
    fn id(&self) -> u32 {
        todo!()
    }

    fn get_min_x(&self) -> f32 {
        todo!()
    }

    fn get_min_y(&self) -> f32 {
        todo!()
    }

    fn get_column(&self) -> i32 {
        todo!()
    }

    fn get_row(&self) -> i32 {
        todo!()
    }

    fn grid_size(&self) -> f32 {
        todo!()
    }
}

impl Component for DummySceneData {
    type Storage = FlaggedStorage<Self, NullStorage<Self>>;
}

#[derive(Default)]
pub struct DummyDropEntity;
impl DropEntity for DummyDropEntity {
    fn mut_entities(&mut self) -> &mut Vec<u32> {
        todo!()
    }
}

impl Output for DummyDropEntity {
    fn cmd() -> u32 {
        todo!()
    }
}

#[derive(Debug)]
pub struct DummyMessage;
#[allow(unused_variables)]
impl Message for DummyMessage {
    fn descriptor(&self) -> &'static MessageDescriptor {
        todo!()
    }

    fn is_initialized(&self) -> bool {
        todo!()
    }

    fn merge_from(&mut self, is: &mut CodedInputStream) -> ProtobufResult<()> {
        todo!()
    }

    fn write_to_with_cached_sizes(&self, os: &mut CodedOutputStream) -> ProtobufResult<()> {
        todo!()
    }

    fn compute_size(&self) -> u32 {
        todo!()
    }

    fn get_cached_size(&self) -> u32 {
        todo!()
    }

    fn get_unknown_fields<'s>(&'s self) -> &'s UnknownFields {
        todo!()
    }

    fn mut_unknown_fields<'s>(&'s mut self) -> &'s mut UnknownFields {
        todo!()
    }

    fn as_any(&self) -> &dyn Any {
        todo!()
    }

    fn new() -> Self
    where
        Self: Sized,
    {
        todo!()
    }

    fn default_instance() -> &'static Self
    where
        Self: Sized,
    {
        todo!()
    }
}
impl Clear for DummyMessage {
    fn clear(&mut self) {
        todo!()
    }
}

impl Deref for DummyDropEntity {
    type Target = DummyMessage;

    fn deref(&self) -> &Self::Target {
        todo!()
    }
}
