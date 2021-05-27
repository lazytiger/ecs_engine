use crate::{BagInfo, DynamicManager, UserInfo};
use codegen::system;
use specs::{DispatcherBuilder, LazyUpdate, World, WorldExt};

#[system]
#[dynamic(user)]
fn user_derive(_user: &UserInfo, _bag: &BagInfo, #[state] _other: &usize) {}

#[system]
#[dynamic(lib = "guild", fn = "test")]
fn guild_derive(_user: &UserInfo, _bag: &BagInfo, #[resource] _lazy_update: &LazyUpdate) {}

pub fn run() {
    let mut world = World::new();
    let mut builder = DispatcherBuilder::new();
    let dm = DynamicManager::default();
    //UserDeriveSystem::default().setup(&mut world, &mut builder, &dm);
    //GuildDeriveSystem::default().setup(&mut world, &mut builder, &dm);
    world.insert(dm);
    let mut dispatcher = builder.build();
    dispatcher.setup(&mut world);
    dispatcher.dispatch(&world);
    world.maintain();
}
