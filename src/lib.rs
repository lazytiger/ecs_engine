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

pub enum MutableStatus {
    Loaded,
    Modified,
    Created,
    Deleted,
}

pub struct Mutable<T, const N: usize> {
    status: MutableStatus,
    old: Option<T>,
    curr: T,
}

impl<T, const N: usize> Deref for Mutable<T, N> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.curr
    }
}

impl<T, const N: usize> Mutable<T, N>
where
    T: Clone,
    T: Default,
{
    pub fn new(t: T) -> Self {
        Self {
            old: None,
            curr: t,
            status: MutableStatus::Loaded,
        }
    }

    pub fn create(mut self) -> Self {
        self.status = MutableStatus::Created;
        self
    }

    pub fn delete(mut self) -> Self {
        self.status = MutableStatus::Deleted;
        self
    }

    pub fn get_mut(&mut self) -> &mut T {
        if let None = self.old {
            self.old.replace(self.curr.clone());
        }
        MODS[N].store(true, Ordering::Relaxed);
        &mut self.curr
    }

    pub fn diff(&self) -> Vec<u8> {
        todo!()
    }
}

impl<T, const N: usize> Mutable<T, N> {
    #[inline]
    pub fn modified() -> bool {
        MODS[N].load(Ordering::Relaxed)
    }

    #[inline]
    pub fn reset() {
        MODS[N].store(false, Ordering::Relaxed);
    }
}

impl<T, const N: usize> Component for Mutable<T, N>
where
    T: 'static + Send + Sync,
{
    type Storage = VecStorage<Mutable<T, N>>;
}

pub const MAX_COMPONENTS: usize = 1024;

lazy_static::lazy_static! {
    pub static ref MODS:Vec<AtomicBool> = {
        let mut mods = Vec::with_capacity(MAX_COMPONENTS);
        for _i in 0..MAX_COMPONENTS {
           mods.push(AtomicBool::new(false));
        }
        mods
    };
}

pub struct CommitChangeSystem<T, const N: usize, const M: usize> {
    counter: usize,
    _phantom: PhantomData<T>,
}

impl<'a, T, const N: usize, const M: usize> System<'a> for CommitChangeSystem<T, N, M>
where
    Mutable<T, N>: Component,
{
    type SystemData = (WriteStorage<'a, Mutable<T, N>>,);

    fn run(&mut self, (data,): Self::SystemData) {
        self.counter += 1;
        if self.counter != M {
            return;
        } else {
            self.counter = 0;
        }
        if !Mutable::<T, N>::modified() {
            return;
        }
        for (_data,) in (&data,).join() {
            todo!()
        }
        Mutable::<T, N>::reset();
    }
}

pub trait ChangeSet {
    fn mask(&self) -> u128;
    fn mask_mut(&mut self) -> &mut u128;
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
