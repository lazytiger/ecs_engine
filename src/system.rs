use crate::{
    component::Closing,
    network::{RequestData, ResponseSender},
    sync::Changeset,
    Input, NetToken, RequestIdent,
};
use crossbeam::channel::Receiver;
use specs::{
    Component, Entities, Join, LazyUpdate, Read, ReadStorage, RunNow, System, World, WorldExt,
    WriteStorage,
};
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

pub struct InputSystem<I, O> {
    receiver: Receiver<RequestData<I>>,
    sender: ResponseSender<O>,
}

impl<I, O> InputSystem<I, O> {
    pub fn new(receiver: Receiver<RequestData<I>>, sender: ResponseSender<O>) -> Self {
        Self { receiver, sender }
    }
}

impl<'a, I, O> RunNow<'a> for InputSystem<I, O>
where
    I: Input<Output = O> + Send + Sync + 'static,
    O: Clone,
{
    fn run_now(&mut self, world: &'a World) {
        //TODO how to control input frequency.
        self.receiver.try_iter().for_each(|(ident, data)| {
            log::debug!("new request found");
            if let Some(data) = data {
                if let Err(err) = data.add_component(ident, world, self.sender.clone()) {
                    log::error!("add component failed:{}", err);
                }
            } else {
                match ident {
                    RequestIdent::Entity(entity) => {
                        if let Some(token) = world.read_component::<NetToken>().get(entity) {
                            self.sender.send_close(token.token(), false);
                        } else {
                            log::error!("entity:{:?} has no NetToken component", entity);
                        }
                    }
                    RequestIdent::Token(token) => {
                        self.sender.send_close(token, false);
                    }
                    _ => unreachable!("close shouldn't decode failed"),
                }
            }
        })
    }

    fn setup(&mut self, world: &mut World) {
        I::setup(world);
    }
}

pub struct CloseSystem<T> {
    _phantom: PhantomData<T>,
}

impl<T> CloseSystem<T> {
    pub fn new() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }
}

impl<'a, T> System<'a> for CloseSystem<T>
where
    T: Send + Sync + 'static,
{
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, Closing>,
        ReadStorage<'a, NetToken>,
        Read<'a, LazyUpdate>,
    );

    fn run(&mut self, (entities, closing, tokens, lazy_update): Self::SystemData) {
        let (entities, tokens): (Vec<_>, Vec<_>) = (&entities, &tokens, &closing)
            .join()
            .map(|(entity, token, _)| (entity, token.token()))
            .unzip();
        if entities.is_empty() {
            return;
        }

        lazy_update.exec_mut(move |world| {
            if let Err(err) = world.delete_entities(entities.as_slice()) {
                log::error!("delete entities failed:{}", err);
            }
            log::debug!("{} entities deleted", entities.len());
            world
                .read_resource::<ResponseSender<T>>()
                .broadcast_close(tokens);
        });
    }

    fn setup(&mut self, world: &mut World) {
        world.register::<Closing>();
    }
}
