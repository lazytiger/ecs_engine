use ecs_engine::config::Config;
use serde_derive::{Deserialize, Serialize};
use std::{fs::File, io::Read};

fn main() {
    let mut data = String::default();
    File::open("examples/example.toml")
        .unwrap()
        .read_to_string(&mut data)
        .unwrap();
    let data: Config = toml::from_str(data.as_str()).unwrap();
    println!("{:?}", data);
}
