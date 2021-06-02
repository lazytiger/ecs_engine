use codegen::system;
use ecs_engine::{BagInfo, DynamicManager, UserInfo};
use specs::{DispatcherBuilder, Entity, Join, LazyUpdate, World, WorldExt};

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
fn static_test(user: &UserInfo, #[resource] index: &mut usize) {
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
