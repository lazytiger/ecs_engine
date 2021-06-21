use crate::ResponseSender;
use mio::Token;
use specs::{Component, DenseVecStorage, HashMapStorage, NullStorage, VecStorage};
use std::ops::{Deref, DerefMut};

macro_rules! component {
    ($storage:ident, $name:ident) => {
        pub struct $name<T> {
            data: T,
        }

        impl<T> Component for $name<T>
        where
            T: 'static + Sync + Send,
        {
            type Storage = $storage<Self>;
        }
        impl<T> Deref for $name<T> {
            type Target = T;

            fn deref(&self) -> &Self::Target {
                &self.data
            }
        }

        impl<T> DerefMut for $name<T> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.data
            }
        }

        impl<T> $name<T> {
            pub fn new(data: T) -> Self {
                Self { data }
            }
        }
    };
}

component!(HashMapStorage, HashComponent);
component!(VecStorage, VecComponent);
component!(DenseVecStorage, DenseVecComponent);

pub type NetToken = VecComponent<Token>;

impl NetToken {
    pub fn token(&self) -> Token {
        self.data
    }
}

#[derive(Default)]
pub struct Closing;

impl Component for Closing {
    type Storage = NullStorage<Self>;
}

pub struct SelfSender {
    token: Token,
    sender: ResponseSender,
}

impl Component for SelfSender {
    type Storage = VecStorage<Self>;
}

impl SelfSender {
    pub fn new(token: Token, sender: ResponseSender) -> Self {
        Self { token, sender }
    }

    pub fn send_data(&self, data: Vec<u8>) {
        self.sender.send_data(self.token, data);
    }

    pub fn send_close(&self, confirm: bool) {
        self.sender.send_close(self.token, confirm);
    }
}
