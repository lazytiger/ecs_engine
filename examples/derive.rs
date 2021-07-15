#![feature(macro_attributes_in_derive_output)]
#![deny(unsafe_code)]
#![allow(dead_code)]
use ecs_engine::{export, system, ChangeSet, DynamicManager, GameDispatcherBuilder};
use specs::{
    world::Index, BitSet, Component, DenseVecStorage, DispatcherBuilder, HashMapStorage, Join,
    LazyUpdate, VecStorage, World, WorldExt,
};

#[derive(Clone, Default, Component)]
pub struct UserInfo {
    pub name: String,
    pub guild_id: Index,
}

#[derive(Clone, Default, Component)]
pub struct GuildInfo {
    users: BitSet,
    pub name: String,
}

#[derive(Clone, Default, Component)]
pub struct BagInfo {
    pub items: Vec<String>,
}

#[derive(Clone, Default, Component)]
pub struct GuildMember {
    pub role: u8,
}

#[derive(Component)]
#[storage(HashMapStorage)]
pub struct UserInput {
    name: String,
}

impl ChangeSet for BagInfo {
    fn index() -> usize {
        todo!()
    }
}

impl ChangeSet for UserInfo {
    fn index() -> usize {
        todo!()
    }
}

#[system]
#[dynamic(user)]
fn user_derive(
    input: &UserInput,
    bag: &mut BagInfo,
    #[state] other: &mut usize,
    #[resource] re: &mut String,
) -> Option<UserInfo> {
    None
}

#[system]
#[dynamic(lib = "guild", func = "test")]
fn guild_derive(
    entity: &Entity,
    user: &UserInfo,
    bag: &BagInfo,
    #[resource] lazy_update: &LazyUpdate,
) {
}

#[system]
#[statics]
fn static_test(_user: &UserInfo, #[resource] index: &mut usize) {
    *index += 1;
}

fn setup_logger() -> Result<(), fern::InitError> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}:{}][{}]{}",
                chrono::Local::now().format("[%Y-%m-%d %H:%M:%S%.6f]"),
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                record.level(),
                message
            ))
        })
        .level(log::LevelFilter::Trace)
        .chain(std::io::stdout())
        .apply()?;
    Ok(())
}

fn main() {
    setup_logger().unwrap();
    let mut world = World::new();
    let mut builder = GameDispatcherBuilder::new(DispatcherBuilder::new(), true);
    let dm = DynamicManager::default();
    UserDeriveSystem::default().setup(&mut world, &mut builder, &dm);
    GuildDeriveSystem::default().setup(&mut world, &mut builder, &dm);
    world.insert(dm);
    let mut dispatcher = builder.build();
    dispatcher.setup(&mut world);
    dispatcher.dispatch(&world);
    world.maintain();
}

#[export(UserDeriveSystemFn)]
fn user_derive_test(
    _user: &UserInput,
    _bag: &mut BagInfo,
    _other: &mut usize,
    _re: &mut String,
) -> Option<UserInfo> {
    None
}

#[derive(Component)]
#[storage(VecStorage)]
pub struct MyTest {
    pub name: String,
    age: u8,
    sex: u8,
}

#[derive(Component)]
#[storage(VecStorage)]
pub struct MyTest1 {
    pub name: String,
    age: u8,
    sex: u8,
}
