use crate::{
    dlog::{log_param, LogParam},
    Symbol,
};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

pub struct Library {
    name: String,
    lib: Option<libloading::Library>,
    generation: usize,
}

impl Library {
    pub fn new(name: String) -> Library {
        let mut lib = Library {
            name,
            lib: None,
            generation: 0,
        };
        lib.reload();
        lib
    }

    pub fn get<T>(&self, name: &String) -> Option<Symbol<T>> {
        if self.lib.is_none() {
            log::debug!("library is not set");
            return None;
        }

        let mut bname = name.as_bytes().to_owned();
        bname.push(0);
        unsafe {
            match self.lib.as_ref().unwrap().get::<fn()>(bname.as_slice()) {
                Ok(f) => Some(std::mem::transmute(f.into_raw())),
                Err(err) => {
                    log::error!(
                        "get function {} from library {} failed {}",
                        name,
                        self.name,
                        err
                    );
                    None
                }
            }
        }
    }

    pub fn reload(&mut self) {
        let name = libloading::library_filename(self.name.as_str());
        log::debug!("loading library {:?}", name);
        match unsafe { libloading::Library::new(name) } {
            Ok(lib) => {
                if let Some(olib) = self.lib.take() {
                    if let Err(err) = olib.close() {
                        log::error!("close library `{}` failed with `{:?}`", self.name, err);
                    }
                }
                self.lib.replace(lib);
                self.generation += 1;
                let fname = "init_logger".into();
                if let Some(f) = self.get::<fn(LogParam)>(&fname) {
                    f(log_param());
                }
            }
            Err(err) => log::error!("open library `{}` failed with `{:?}`", self.name, err),
        }
    }

    pub fn generation(&self) -> usize {
        self.generation
    }
}

#[derive(Default)]
pub struct DynamicManager {
    libraries: RwLock<HashMap<String, Arc<Library>>>,
}

impl DynamicManager {
    pub fn get(&self, lib: &String) -> Arc<Library> {
        {
            if let Some(lib) = self.libraries.read().unwrap().get(lib) {
                return lib.clone();
            }
        }

        {
            let nlib = Arc::new(Library::new(lib.clone()));
            self.libraries
                .write()
                .unwrap()
                .insert(lib.clone(), nlib.clone());
            nlib
        }
    }
}

pub struct DynamicSystem<T> {
    lname: String,
    fname: String,
    generation: usize,
    lib: Option<Arc<Library>>,
    func: Option<Arc<Symbol<T>>>,
}

impl<T> Default for DynamicSystem<T> {
    fn default() -> Self {
        Self {
            lname: "".into(),
            fname: "".into(),
            generation: 0,
            lib: None,
            func: None,
        }
    }
}

impl<T> DynamicSystem<T> {
    pub fn get_symbol(&mut self, dm: &DynamicManager) -> Option<Arc<Symbol<T>>> {
        if let Some(lib) = &self.lib {
            if lib.generation() == self.generation {
                return self.func.clone();
            } else {
                self.lib.take();
                self.func.take();
            }
        }

        if let None = self.lib {
            self.lib.replace(dm.get(&self.lname));
            self.generation = self.lib.as_ref().unwrap().generation;
        }

        if let Some(func) = self.lib.as_ref().unwrap().get(&self.fname) {
            self.func.replace(Arc::new(func));
        }
        self.func.clone()
    }

    pub fn init(&mut self, lname: String, fname: String, dm: &DynamicManager) {
        if self.generation != 0 {
            panic!(
                "DynamicSystem({}, {}) already initialized",
                self.lname, self.fname
            )
        }
        log::info!("init dynamic library {}, function:{}", lname, fname);
        self.lname = lname;
        self.fname = fname;
        self.get_symbol(dm);
    }
}
