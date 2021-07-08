#![allow(dead_code)]
use crate::{Output, ResponseSender};
use mio::Token;
use specs::{
    BitSet, Component, DenseVecStorage, Entity, FlaggedStorage, HashMapStorage, Join, NullStorage,
    ReadStorage, VecStorage,
};
use specs_hierarchy::Parent;
use std::{
    cmp::Ordering,
    ops::{Deref, DerefMut},
};

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
    id: u32,
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
    pub fn new(id: u32, token: Token, sender: ResponseSender<T>) -> Self {
        Self { id, token, sender }
    }

    pub fn send_data(&self, data: impl Into<T>) {
        self.sender.send_data(self.id, self.token, data);
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
    /// 场景id
    fn id(&self) -> u32;
    /// 场景坐标的最小xy值
    fn get_min_xy(&self) -> (f32, f32);
    /// 获取场景的分块尺寸，即可以分为行列数
    fn get_size(&self) -> (i32, i32);
    /// 场景分隔的正方形边长
    fn grid_size(&self) -> f32;
    /// 根据位置信息计算格子索引
    /// index = y * column + x
    fn grid_index(&self, x: f32, y: f32) -> Option<usize> {
        let (min_x, min_y) = self.get_min_xy();
        if x < min_x || y < min_y {
            return None;
        }
        let x = ((x - min_x) * 100.0) as i32;
        let y = ((y - min_y) * 100.0) as i32;
        let grid_size = (self.grid_size() * 100.0) as i32;
        let x = x / grid_size;
        let y = y / grid_size;
        let (row, column) = self.get_size();
        if x >= column || y >= row {
            return None;
        }
        Some((y * column + x) as usize)
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
    /// 根据旧的索引以及新索引来得到三个数据，分别代表删除，未变，新增
    fn diff(&self, old: usize, new: usize) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
        let old = self.around(old);
        let new = self.around(new);
        let mut only_old = Vec::new();
        let mut only_new = Vec::new();
        let mut share = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < old.len() && j < new.len() {
            match old[i].cmp(&new[j]) {
                Ordering::Less => {
                    only_old.push(old[i]);
                    i += 1;
                }
                Ordering::Equal => {
                    share.push(old[i]);
                    i += 1;
                    j += 1;
                }
                Ordering::Greater => {
                    only_new.push(new[j]);
                    j += 1;
                }
            }
        }
        if i < old.len() {
            only_old.extend_from_slice(&old.as_slice()[i..]);
        }
        if j < old.len() {
            only_new.extend_from_slice(&new.as_slice()[j..]);
        }

        (only_old, share, only_new)
    }
}
pub type TeamMember = Member<0>;
pub type SceneMember = Member<1>;
#[derive(Default)]
pub struct NewSceneMember(pub Option<BitSet>);

impl Component for NewSceneMember {
    type Storage = HashMapStorage<Self>;
}
