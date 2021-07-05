#![allow(dead_code)]
use crate::{Output, ResponseSender};
use mio::Token;
use specs::{
    BitSet, Component, DenseVecStorage, Entity, FlaggedStorage, HashMapStorage, Join, NullStorage,
    ReadStorage, VecStorage,
};
use specs_hierarchy::Parent;
use std::ops::{Deref, DerefMut};

macro_rules! component {
    ($storage:ident, $name:ident) => {
        #[derive(Debug, Default)]
        pub struct $name<T: Default> {
            data: T,
        }

        impl<T: Default> Component for $name<T>
        where
            T: 'static + Sync + Send,
        {
            type Storage = $storage<Self>;
        }
        impl<T: Default> Deref for $name<T> {
            type Target = T;

            fn deref(&self) -> &Self::Target {
                &self.data
            }
        }

        impl<T: Default> DerefMut for $name<T> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.data
            }
        }

        impl<T: Default> $name<T> {
            pub fn new(data: T) -> Self {
                Self { data }
            }
        }
    };
}

component!(HashMapStorage, HashComponent);
component!(VecStorage, VecComponent);
component!(DenseVecStorage, DenseVecComponent);

pub type NetToken = VecComponent<usize>;

impl NetToken {
    pub fn token(&self) -> Token {
        Token(self.data)
    }

    pub fn tokens<'a>(storage: &'a ReadStorage<'a, NetToken>, set: &BitSet) -> Vec<Token> {
        (storage, set)
            .join()
            .map(|(token, _)| token.token())
            .collect()
    }
}

#[derive(Default)]
pub struct Closing;

impl Component for Closing {
    type Storage = NullStorage<Self>;
}

/// 单用于发送数据给自己
pub struct SelfSender<T> {
    token: Token,
    sender: ResponseSender<T>,
}

impl<T> Component for SelfSender<T>
where
    T: Sync + Send + 'static,
{
    type Storage = VecStorage<Self>;
}

impl<T> SelfSender<T>
where
    T: Output,
{
    pub fn new(token: Token, sender: ResponseSender<T>) -> Self {
        Self { token, sender }
    }

    pub fn send_data(&self, data: impl Into<T>) {
        self.sender.send_data(self.token, data);
    }

    pub fn send_close(&self, confirm: bool) {
        self.sender.send_close(self.token, confirm);
    }
}

pub struct Member<const T: usize> {
    entity: Entity,
}

impl<const T: usize> Member<T> {
    pub fn new(entity: Entity) -> Self {
        Self { entity }
    }
}

impl<const T: usize> Component for Member<T> {
    type Storage = FlaggedStorage<Self, VecStorage<Self>>;
}

impl<const T: usize> Parent for Member<T> {
    fn parent_entity(&self) -> Entity {
        self.entity
    }
}
/// 玩家的位置信息
pub trait Position {
    /// x轴坐标
    fn x(&self) -> f32;
    /// y轴坐标
    fn y(&self) -> f32;
}

/// 场景尺寸信息
pub trait SceneData: Clone {
    /// 场景坐标的最小xy值
    fn get_min_xy(&self) -> (f32, f32);
    /// 获取场景的分块尺寸，即可以分为行列数
    fn get_size(&self) -> (i32, i32);
    /// 场景分隔的正方形边长
    fn grid_size(&self) -> f32;
    /// 根据位置信息计算格子索引
    /// index = y * column + x
    fn grid_index(&self, p: &impl Position) -> usize {
        let x = p.x();
        let y = p.y();
        let (min_x, min_y) = self.get_min_xy();
        let x = ((x - min_x) * 100.0) as i32;
        let y = ((y - min_y) * 100.0) as i32;
        let grid_size = (self.grid_size() * 100.0) as i32;
        let x = x / grid_size;
        let y = y / grid_size;
        let (_, column) = self.get_size();
        (y * column + x) as usize
    }
    /// 获取周围格子的索引，包括当前格子
    fn around(&self, index: usize) -> Vec<usize> {
        let mut data = Vec::new();
        let index = index as i32;
        let (row, column) = self.get_size();
        let x = index % column;
        let y = index / column;
        for i in [-1, 0, 1] {
            let xx = x + i;
            if xx < 0 || xx >= column {
                continue;
            }
            for j in [-1, 0, 1] {
                let yy = y + j;
                if yy < 0 || yy >= row {
                    continue;
                }
                data.push((yy * column + xx) as usize)
            }
        }
        data
    }
}
pub type TeamMember = Member<0>;
pub type SceneMember = Member<1>;
