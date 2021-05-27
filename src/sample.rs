use specs::shred::DynamicSystemData;
use specs::{DispatcherBuilder, Entities, Join, Read, ReadStorage, System, World, WorldExt};

use crate::{BagInfo, DynamicManager, DynamicSystem, GuildInfo, Library, Symbol, UserInfo};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct UserTestSystem {
    lib: DynamicSystem<fn(&UserInfo, &BagInfo)>,
}

impl<'a> System<'a> for UserTestSystem {
    type SystemData = (
        ReadStorage<'a, UserInfo>,
        ReadStorage<'a, BagInfo>,
        Read<'a, DynamicManager>,
    );

    fn run(&mut self, (user, bag, dm): Self::SystemData) {
        if let Some(symbol) = self.lib.get_symbol(&dm) {
            for (user, bag) in (&user, &bag).join() {
                (*symbol)(user, bag);
            }
        } else {
            todo!()
        }
    }
}

pub fn user_test_setup(world: &mut World, builder: &mut DispatcherBuilder, dm: &DynamicManager) {
    world.register::<UserInfo>();
    world.register::<BagInfo>();
    let mut system = UserTestSystem::default();
    system.lib.init("".into(), "".into(), dm);
    builder.add(system, "user_test", &[]);
}

#[derive(Default)]
struct GuildTestSystem {
    lib: DynamicSystem<fn(&UserInfo, &GuildInfo)>,
}

impl<'a> System<'a> for GuildTestSystem {
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, UserInfo>,
        ReadStorage<'a, GuildInfo>,
    );

    fn run(&mut self, (entities, user, guild): Self::SystemData) {
        for (entity, _user) in (&entities, &user).join() {
            if let Some(_guild) = guild.get(entity) {
                //guild_test(user, guild);
            } else {
                //log
            }
        }
    }
}

pub fn guild_test_setup(world: &mut World, builder: &mut DispatcherBuilder, dm: &DynamicManager) {
    world.register::<UserInfo>();
    world.register::<GuildInfo>();
    let mut system = GuildTestSystem::default();
    system.lib.init("".into(), "".into(), dm);
    builder.add(system, "guild_test", &[]);
}

pub fn run() {
    let mut world = World::new();
    let mut builder = DispatcherBuilder::new();
    let dm = DynamicManager::default();
    user_test_setup(&mut world, &mut builder, &dm);
    guild_test_setup(&mut world, &mut builder, &dm);
    world.insert(dm);
    let mut dispatcher = builder.build();
    dispatcher.setup(&mut world);
    dispatcher.dispatch(&world);
    world.maintain();
}
