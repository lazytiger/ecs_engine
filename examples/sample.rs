use specs::{
    BitSet, DispatcherBuilder, Entities, Join, Read, ReadStorage, System, World, WorldExt, Write,
    WriteStorage,
};

use ecs_engine::{ChangeSet, DynamicManager, DynamicSystem, Mutable, SerDe};
use specs::world::Index;

#[derive(Clone, Default)]
pub struct UserInfo {
    pub name: String,
    pub guild_id: Index,
}

#[derive(Clone, Default)]
pub struct GuildInfo {
    users: BitSet,
    pub name: String,
}

#[derive(Clone, Default)]
pub struct BagInfo {
    pub items: Vec<String>,
}

#[derive(Clone, Default)]
pub struct GuildMember {
    pub role: u8,
}

pub type UserTestFn = fn(&UserInfo, &BagInfo, &usize);

fn test(_a: &UserInfo, _b: &BagInfo, _c: &usize) {}

static _T: UserTestFn = test;

#[derive(Default)]
struct UserTestSystem {
    lib: DynamicSystem<fn(&UserInfo, &BagInfo, &usize)>,
}

pub type UserInfoMut = Mutable<UserInfo, 1>;
pub type BagInfoMut = Mutable<BagInfo, 2>;

impl UserTestSystem {
    pub fn setup(
        mut self,
        world: &mut World,
        builder: &mut DispatcherBuilder,
        dm: &DynamicManager,
    ) {
        world.register::<UserInfoMut>();
        world.register::<BagInfoMut>();
        self.lib.init("".into(), "".into(), dm);
        builder.add(self, "user_test", &[]);
    }
}

impl<'a> System<'a> for UserTestSystem {
    type SystemData = (
        ReadStorage<'a, UserInfoMut>,
        ReadStorage<'a, BagInfoMut>,
        Read<'a, DynamicManager>,
        Write<'a, usize>,
    );

    fn run(&mut self, (user, bag, dm, mut size): Self::SystemData) {
        if let Some(symbol) = self.lib.get_symbol(&dm) {
            for (user, bag) in (&user, &bag).join() {
                (*symbol)(user, bag, &mut size);
            }
        } else {
            log::error!("symbol not found");
        }
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
        world.register::<UserInfoMut>();
        world.register::<GuildInfoMut>();
        world.register::<GuildMemberMut>();
        self.lib.init("".into(), "".into(), dm);
        builder.add(self, "guild_test", &[]);
    }
}

pub type GuildInfoMut = Mutable<GuildInfo, 3>;
pub type GuildMemberMut = Mutable<GuildMember, 4>;

impl<'a> System<'a> for GuildTestSystem {
    type SystemData = (
        Entities<'a>,
        Read<'a, DynamicManager>,
        ReadStorage<'a, UserInfoMut>,
        ReadStorage<'a, GuildInfoMut>,
        WriteStorage<'a, GuildMemberMut>,
    );

    fn run(&mut self, (entities, dm, user, guild, mut member_storage): Self::SystemData) {
        if let Some(symbol) = self.lib.get_symbol(&dm) {
            for (entity, user) in (&entities, &user).join() {
                if let Some(guild) = guild.get(entity) {
                    if let Some(member) = (*symbol)(user, guild) {
                        if let Err(err) = member_storage.insert(entity, Mutable::new(member)) {
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

impl ChangeSet for MyTest {
    fn index() -> usize {
        0
    }

    fn mask(&self) -> u128 {
        self._mask
    }

    fn reset(&mut self) {
        self._mask = 0;
    }
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

impl MyTest {
    pub fn foreach_change<T, S>(&self, callback: T)
    where
        T: Fn(u128, &dyn SerDe),
    {
        let mut mask = self._mask;
        for i in 0..10 {
            if mask & 0x1 == 0 {
                continue;
            }
            mask >>= 1;
            match i {
                0 => callback(i, &self.age),
                1 => callback(i, &self.gender),
                _ => unreachable!(),
            }
        }
    }
}
