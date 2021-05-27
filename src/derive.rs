use crate::{UserInfo, BagInfo};
use specs::{World, DispatcherBuilder, WorldExt, LazyUpdate};
use codegen::system;

#[system]
#[dynamic(user)]
fn user_derive(_user:&UserInfo, _bag:&BagInfo, #[state] _other:&usize) {

}

#[system]
#[dynamic(lib = "guild", fn = "test")]
fn guild_derive(_user:&UserInfo, _bag:&BagInfo, #[resource] _lazy_update:&LazyUpdate) {

}

pub fn run() {
    let mut world = World::new();
    let builder = DispatcherBuilder::new();
    //user_derive_setup(&mut world, &mut builder);
    //guild_derive_setup(&mut world, &mut builder);
    let mut dispatcher = builder.build();
    dispatcher.setup(&mut world);
    dispatcher.dispatch(&world);
    world.maintain();
}