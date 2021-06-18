use crate::{
    network::{RequestData, ResponseSender},
    sync::Changeset,
    Input,
};
use crossbeam::channel::Receiver;
use specs::{Component, Join, RunNow, System, World, WriteStorage};
use std::marker::PhantomData;

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

pub struct InputSystem<T> {
    receiver: Receiver<RequestData<T>>,
    sender: ResponseSender,
}

impl<T> InputSystem<T> {
    pub fn new(receiver: Receiver<RequestData<T>>, sender: ResponseSender) -> InputSystem<T> {
        Self { receiver, sender }
    }
}

impl<'a, T> RunNow<'a> for InputSystem<T>
where
    T: Input + Send + Sync + 'static,
{
    fn run_now(&mut self, world: &'a World) {
        self.receiver.try_iter().for_each(|(ident, data)| {
            log::debug!("new request found");
            if let Err(err) = data.add_component(ident, world, &self.sender) {
                log::error!("add component failed:{}", err);
            }
        })
    }

    fn setup(&mut self, world: &mut World) {
        T::setup(world);
    }
}
