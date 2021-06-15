use specs::{
    BitSet, Component, DenseVecStorage, DispatcherBuilder, Entities, HashMapStorage, Join, Read,
    ReadStorage, System, World, WorldExt, Write, WriteStorage,
};

use ecs_engine::{Changeset, DynamicManager, DynamicSystem, HashComponent};
use specs::world::Index;

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
    pub name: String,
}

pub type UserTestFn = fn(&UserInfo, &BagInfo, &usize);

fn test(_a: &UserInfo, _b: &BagInfo, _c: &usize) {}

static _T: UserTestFn = test;

#[derive(Default)]
struct UserTestSystem {
    lib: DynamicSystem<fn(&UserInput, &BagInfo, &usize) -> Option<UserInfo>>,
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
        world.register::<UserInput>();
        self.lib.init("".into(), "".into(), dm);
        builder.add(self, "user_test", &[]);
    }
}

impl<'a> System<'a> for UserTestSystem {
    type SystemData = (
        WriteStorage<'a, UserInput>,
        WriteStorage<'a, UserInfo>,
        ReadStorage<'a, BagInfo>,
        Read<'a, DynamicManager>,
        Write<'a, usize>,
        Entities<'a>,
    );

    fn run(&mut self, (mut input, mut user, bag, dm, mut size, entities): Self::SystemData) {
        if let Some(symbol) = self.lib.get_symbol(&dm) {
            (&input, &bag, &entities)
                .join()
                .for_each(|(input, bag, entity)| {
                    if let Some(u) = symbol(&input, bag, &mut size) {
                        user.insert(entity, u);
                    }
                });
        } else {
            log::error!("symbol not found");
        }
        let es: Vec<_> = (&entities, &input).join().map(|(e, _)| e).collect();
        es.iter().for_each(|e| {
            input.remove(*e);
        });
    }
}

#[derive(Default)]
struct GuildTestSystem {
    lib: DynamicSystem<fn(&UserInfo, &GuildInfo) -> Option<GuildMember>>,
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
        world.register::<GuildMember>();
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
        WriteStorage<'a, GuildMember>,
    );

    fn run(&mut self, (entities, dm, user, guild, mut member_storage): Self::SystemData) {
        if let Some(symbol) = self.lib.get_symbol(&dm) {
            for (entity, user) in (&entities, &user).join() {
                if let Some(guild) = guild.get(entity) {
                    if let Some(member) = (*symbol)(user, guild) {
                        if let Err(err) = member_storage.insert(entity, member) {
                            log::error!("insert component failed:{}", err);
                        }
                    }
                } else {
                    log::error!("guild:{:?} not found", entity);
                }
            }
        } else {
            log::error!("symbol not found");
        }
    }
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

struct MyTest {
    name: String,
    gender: u8,
    age: u8,
    profession: u8,
    _mask: u128,
}

impl MyTest {
    #[inline]
    pub fn name(&self) -> &String {
        &self.name
    }

    #[inline]
    pub fn name_mut(&mut self) -> &mut String {
        self._mask |= 1 << 0;
        &mut self.name
    }

    pub fn gender(&self) -> u8 {
        self.gender
    }

    pub fn age(&self) -> u8 {
        self.age
    }
}
