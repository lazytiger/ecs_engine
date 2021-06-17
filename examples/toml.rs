use ecs_engine::config::{ConfigFile};

use std::{convert::TryInto, fs::File, io::Read};

fn main() {
    transmute();
}

fn toml() {
    let mut data = String::default();
    File::open("examples/example.toml")
        .unwrap()
        .read_to_string(&mut data)
        .unwrap();
    let data: ConfigFile = toml::from_str(data.as_str()).unwrap();
    println!("{:?}", data);
}

fn transmute() {
    let mut data = [0u8; 8];
    let length = 0x0010u32;
    let cmd = 0x1234u32;
    unsafe {
        data = std::mem::transmute((length, cmd));
    }
    println!("{:x?}", data);

    let slice = &data[..];
    unsafe {
        let data: [u8; 8] = slice.try_into().expect("try into failed");
        let (length, cmd): (u32, u32) = std::mem::transmute(data);
        println!("{:x?} {:x?}", length, cmd);
    }
}
