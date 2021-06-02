use codegen::system;
use ecs_engine::{BagInfo, DynamicManager, UserInfo};
use specs::{DispatcherBuilder, Join, LazyUpdate, World, WorldExt};

#[system]
#[dynamic(user)]
fn user_derive(
    user: &UserInfo,
    bag: &mut BagInfo,
    #[state] other: &mut usize,
    #[resource] re: &mut String,
) {
}

#[system]
#[dynamic(lib = "guild", func = "test")]
fn guild_derive(_user: &UserInfo, _bag: &BagInfo, #[resource] _lazy_update: &LazyUpdate) {}

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
