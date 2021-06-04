use std::{
    collections::HashMap,
    ops::Deref,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
};

#[cfg(target_os = "windows")]
pub use libloading::os::windows::Symbol;
#[cfg(not(target_os = "windows"))]
pub use libloading::os::windows::Symbol;
use specs::{world::Index, BitSet, Component, Join, System, VecStorage, WriteStorage};
use std::{
    io::{Read, Write},
    marker::PhantomData,
};

pub mod network;

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
        match unsafe { libloading::Library::new(name) } {
            Ok(lib) => {
                if let Some(olib) = self.lib.take() {
                    if let Err(err) = olib.close() {
                        log::error!("close library `{}` failed with `{:?}`", self.name, err);
                    }
                    self.lib.replace(lib);
                    self.generation += 1;
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
        self.lname = lname;
        self.fname = fname;
        self.get_symbol(dm);
    }
}

const MAX_COMPONENTS: usize = 1024;

lazy_static::lazy_static! {
    pub static ref MODS:Vec<AtomicBool> = {
        let mut mods = Vec::with_capacity(MAX_COMPONENTS);
        for _i in 0..MAX_COMPONENTS {
           mods.push(AtomicBool::new(false));
        }
        mods
    };
}

pub struct CommitChangeSystem<T> {
    tick_step: usize,
    counter: usize,
    _phantom: PhantomData<T>,
}

impl<T> CommitChangeSystem<T> {
    fn new(tick_step: usize) -> Self {
        Self {
            tick_step,
            counter: 0,
            _phantom: Default::default(),
        }
    }
}

impl<T> Default for CommitChangeSystem<T> {
    fn default() -> Self {
        Self::new(1)
    }
}

impl<'a, T> System<'a> for CommitChangeSystem<T>
where
    T: Component,
    T: Changeset,
{
    type SystemData = (WriteStorage<'a, T>,);

    fn run(&mut self, (data,): Self::SystemData) {
        self.counter += 1;
        if self.counter != self.tick_step {
            return;
        } else {
            self.counter = 0;
        }
        if !T::is_storage_dirty() {
            return;
        }

        for (data,) in (&data,).join() {
            if !data.is_dirty() {
                continue;
            }
        }
        T::clear_storage_dirty();
    }
}

// mask的最高两位表示修改状态
// 00 表示修改
// 10 表示新建
// 11 表示删除
pub trait Changeset {
    fn mask(&self) -> u128;
    fn mask_mut(&mut self) -> &mut u128;
    fn index() -> usize;

    #[inline]
    fn is_dirty(&self) -> bool {
        self.mask() != 0
    }

    #[inline]
    fn mask_new(&mut self) {
        *self.mask_mut() |= 0x80000000;
    }

    #[inline]
    fn mask_del(&mut self) {
        *self.mask_mut() |= 0xc0000000;
    }

    #[inline]
    fn is_new(&self) -> bool {
        self.mask() & 0x80000000 != 0
    }

    #[inline]
    fn is_del(&self) -> bool {
        self.mask() & 0xc0000000 != 0
    }

    #[inline]
    fn set_storage_dirty() {
        MODS[Self::index()].store(true, Ordering::Relaxed);
    }

    #[inline]
    fn clear_storage_dirty() {
        MODS[Self::index()].store(false, Ordering::Relaxed);
    }

    #[inline]
    fn is_storage_dirty() -> bool {
        MODS[Self::index()].load(Ordering::Relaxed)
    }
}

pub trait SerDe {
    fn ser<W: Write>(&self, w: &mut Write);
    fn de<R: Read>(&mut self, r: &R);
}

impl SerDe for &u8 {
    fn ser<W: Write>(&self, w: &mut dyn Write) {
        todo!()
    }

    fn de<R: Read>(&mut self, r: &R) {
        todo!()
    }
}
