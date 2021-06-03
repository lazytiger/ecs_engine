use codegen::{system, ChangeSet};
use ecs_engine::DynamicManager;
use specs::{world::Index, BitSet, DispatcherBuilder, Join, LazyUpdate, World, WorldExt};

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

#[system]
#[dynamic(user)]
fn user_derive(
    user: &UserInfo,
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
    let mut builder = DispatcherBuilder::new();
    let dm = DynamicManager::default();
    UserDeriveSystem::default().setup(&mut world, &mut builder, &dm);
    GuildDeriveSystem::default().setup(&mut world, &mut builder, &dm);
    world.insert(dm);
    let mut dispatcher = builder.build();
    dispatcher.setup(&mut world);
    dispatcher.dispatch(&world);
    world.maintain();
}

#[derive(ChangeSet)]
struct MyTest {
    name: String,
    age: u8,
    sex: u8,
}
