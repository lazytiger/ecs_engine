use specs::shred::DynamicSystemData;
use specs::{DispatcherBuilder, Entities, Join, Read, ReadStorage, System, World, WorldExt};

use ecs_engine::{BagInfo, DynamicManager, DynamicSystem, GuildInfo, UserInfo};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct UserTestSystem {
    lib: DynamicSystem<fn(&UserInfo, &BagInfo)>,
}

impl UserTestSystem {
    pub fn setup(
        mut self,
        world: &mut World,
        builder: &mut DispatcherBuilder,
        dm: &DynamicManager,
    ) {
        world.register::<UserInfo>();
        world.register::<BagInfo>();
        self.lib.init("".into(), "".into(), dm);
        builder.add(self, "user_test", &[]);
    }
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

#[derive(Default)]
struct GuildTestSystem {
    lib: DynamicSystem<fn(&UserInfo, &GuildInfo)>,
}

impl GuildTestSystem {
    pub fn setup(
        mut self,
        world: &mut World,
        builder: &mut DispatcherBuilder,
        dm: &DynamicManager,
    ) {
        world.register::<UserInfo>();
        world.register::<GuildInfo>();
        self.lib.init("".into(), "".into(), dm);
        builder.add(self, "guild_test", &[]);
    }
}

impl<'a> System<'a> for GuildTestSystem {
    type SystemData = (
        Entities<'a>,
        Read<'a, DynamicManager>,
        ReadStorage<'a, UserInfo>,
        ReadStorage<'a, GuildInfo>,
    );

    fn run(&mut self, (entities, dm, user, guild): Self::SystemData) {
        if let Some(symbol) = self.lib.get_symbol(&dm) {
            for (entity, user) in (&entities, &user).join() {
                if let Some(guild) = guild.get(entity) {
                    (*symbol)(user, guild);
                } else {
                    //log
                }
            }
        } else {
            //log
        }
    }
}

fn main() {
    let mut world = World::new();
    let mut builder = DispatcherBuilder::new();
    let dm = DynamicManager::default();
    UserTestSystem::default().setup(&mut world, &mut builder, &dm);
    GuildTestSystem::default().setup(&mut world, &mut builder, &dm);
    world.insert(dm);
    let mut dispatcher = builder.build();
    dispatcher.setup(&mut world);
    dispatcher.dispatch(&world);
    world.maintain();
}
