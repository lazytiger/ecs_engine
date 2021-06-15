use ecs_engine::config::{Config, ConfigFile};
use serde_derive::{Deserialize, Serialize};
use std::{fs::File, io::Read};

fn main() {
    let mut data = String::default();
    File::open("examples/example.toml")
        .unwrap()
        .read_to_string(&mut data)
        .unwrap();
    let data: ConfigFile = toml::from_str(data.as_str()).unwrap();
    println!("{:?}", data);
}
