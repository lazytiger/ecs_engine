use serde_derive::Deserialize;
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};

#[derive(Deserialize, Debug)]
pub enum StorageType {
    Vec,
    DefaultVec,
    DenseVec,
    HashMap,
}

#[derive(Deserialize, Debug)]
pub enum ConfigType {
    Request,
    Response,
    Component,
}

#[derive(Deserialize, Debug)]
pub struct Component {
    pub flagged: bool,
    pub mask: bool,
    pub r#type: StorageType,
}

#[derive(Deserialize, Debug)]
pub struct Field {
    pub name: String,
    pub r#type: String,
    pub field: u32,
    pub repeated: Option<bool>,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub name: String,
    pub r#type: ConfigType,
    pub component: Option<Component>,
    pub fields: Vec<Field>,
}

impl Config {
    fn max_number(&self) -> u32 {
        if let Some(c) = self.fields.iter().max_by(|a, b| a.field.cmp(&b.field)) {
            c.field
        } else {
            0
        }
    }
}

pub struct Generator {
    inputs: Vec<String>,
    output: String,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    De(toml::de::Error),
}

impl Generator {
    pub fn gen(&self) -> Result<(), Error> {
        let results: Vec<_> = self
            .inputs
            .iter()
            .map(|f| -> Result<(String, Config), Error> {
                let mut file = File::open(f).map_err(|err| Error::Io(err))?;
                let mut data = String::new();
                file.read_to_string(&mut data)
                    .map_err(|err| Error::Io(err))?;
                let table: Config = toml::from_str(data.as_str()).map_err(|err| Error::De(err))?;
                Ok((f.clone(), table))
            })
            .filter(|ret| {
                if let Err(err) = ret {
                    log::error!("decode failed:{:?}", err);
                }
                ret.is_ok()
            })
            .map(|ret| ret.unwrap())
            .collect();
        for (k, v) in results {
            let mut name = k.split('.').next().unwrap().to_owned();
            name.push_str(".proto");
            let mut path = PathBuf::new();
            path.push(&self.output);
            path.push(name);
            let mut file = File::create(path).map_err(|err| Error::Io(err))?;
            self.gen_proto(&mut file, &v)
                .map_err(|err| Error::Io(err))?;
        }

        Ok(())
    }

    fn gen_proto(&self, file: &mut File, v: &Config) -> std::io::Result<()> {
        writeln!(file, r#"syntax = "proto3";"#)?;
        writeln!(file, "message {} {{", v.name)?;
        for c in &v.fields {
            writeln!(
                file,
                "\t{}{} {} = {};",
                if c.repeated.is_some() && c.repeated.unwrap() {
                    "repeated "
                } else {
                    ""
                },
                c.r#type,
                c.name,
                c.field
            )?;
        }
        if let ConfigType::Component = v.r#type {
            let mask = if let Some(component) = &v.component {
                component.mask
            } else {
                false
            };
            if mask {
                writeln!(file, "u64 mask = {};", v.max_number() + 1)?;
            }
        }
        writeln!(file, "}}")?;
        Ok(())
    }
}
