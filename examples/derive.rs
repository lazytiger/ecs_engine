use codegen::system;
use ecs_engine::{BagInfo, DynamicManager, UserInfo};
use specs::{DispatcherBuilder, LazyUpdate, World, WorldExt};

#[system]
#[dynamic(user)]
fn user_derive(_user: &UserInfo, _bag: &BagInfo, #[state] _other: &usize) {}

#[system]
#[dynamic(lib = "guild", fn = "test")]
fn guild_derive(_user: &UserInfo, _bag: &BagInfo, #[resource] _lazy_update: &LazyUpdate) {}

fn main() {
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
