#[cfg(target_os = "windows")]
pub use libloading::os::windows::Symbol;
#[cfg(not(target_os = "windows"))]
pub use libloading::os::windows::Symbol;
use specs::world::Index;
use specs::BitSet;
use specs::{Component, VecStorage};

mod derive;
mod sample;
pub use derive::run as derive_run;
pub use sample::run as sample_run;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Component)]
#[storage(VecStorage)]
pub struct UserInfo {
    pub name: String,
    pub guild_id: Index,
}

#[derive(Component)]
#[storage(VecStorage)]
pub struct GuildInfo {
    users: BitSet,
    pub name: String,
}

#[derive(Component)]
#[storage(VecStorage)]
pub struct BagInfo {
    pub items: Vec<String>,
}

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
            return None;
        }

        let mut bname = name.as_bytes().to_owned();
        bname.push(0);
        unsafe {
            if let Ok(f) = self.lib.as_ref().unwrap().get::<fn()>(bname.as_slice()) {
                Some(std::mem::transmute(f.into_raw()))
            } else {
                None
            }
        }
    }

    pub fn reload(&mut self) {
        let name = libloading::library_filename(self.name.as_str());
        if let Ok(lib) = unsafe { libloading::Library::new(name) } {
            if let Some(olib) = self.lib.take() {
                if let Err(err) = olib.close() {
                    todo!()
                }
                self.lib.replace(lib);
                self.generation += 1;
            }
        } else {
            todo!()
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
        if let Some(lib) = self.libraries.read().unwrap().get(lib) {
            lib.clone()
        } else {
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
        self.lname = lname;
        self.fname = fname;
        self.get_symbol(dm).unwrap();
    }
}
