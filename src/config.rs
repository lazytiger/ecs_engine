use serde_derive::Deserialize;
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};

#[derive(Deserialize)]
pub struct Column {
    pub name: String,
    pub r#type: String,
    pub storage: String,
    pub flag: Option<String>,
    pub field: u32,
}

#[derive(Deserialize)]
pub struct Table {
    pub name: String,
    pub mask: bool,
    pub columns: Vec<Column>,
}

impl Table {
    fn max_number(&self) -> u32 {
        if let Some(c) = self.columns.iter().max_by(|a, b| a.field.cmp(&b.field)) {
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
            .map(|f| -> Result<(String, Table), Error> {
                let mut file = File::open(f).map_err(|err| Error::Io(err))?;
                let mut data = String::new();
                file.read_to_string(&mut data);
                let table: Table = toml::from_str(data.as_str()).map_err(|err| Error::De(err))?;
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
            self.gen_proto(&mut file, &v);
        }

        Ok(())
    }

    fn gen_proto(&self, file: &mut File, v: &Table) {
        writeln!(file, r#"syntax = "proto3";"#);
        writeln!(file, "message {} {{", v.name);
        v.columns.iter().for_each(|c| {
            writeln!(
                file,
                "\t{} {} {} = {};",
                c.flag.clone().unwrap_or_default(),
                c.r#type,
                c.name,
                c.field
            );
        });
        if v.mask {
            writeln!(file, "u64 mask = {};", v.max_number() + 1);
        }
        writeln!(file, "}}");
    }
}
