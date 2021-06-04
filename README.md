# 基于ECS的服务器设计

## 设计目标
* 适用性 - 能够适应目前已知的所有游戏类型
* 性能 - 能够提供有竞争力的服务器性能
* 在线更新 - 适应游戏开发的快速迭代
* 扩展性 - 具备适应变化的潜力
    * 为什么一般同一个程序内很难具有两种不同的线程模型
        * 线程模型与数据结构息息相关
        * 传统程序数据与业务逻辑强绑定
        * 因为业务逻辑与需求相关不能修改，而逻辑与数据强绑定，因此数据也不能修改，于是线程模型也不能修改

## 技术选型
* ECS
    * 介绍 TBD 需要一个完整的示例
        * Entity 实体，实际就是一个标识，只是用于串联所有的Component，一般用一个正整数实现
        * Component 组件，实际就是一组数据，这组数据一般来说会被同时访问或者修改，一个实体可以挂载多个组件
        * System 系统，用于进行组件间进行交互，聚合一系列的组合并操作读写操作
        * World 世界，统合整个ECS的最上层模块，实体，组件，系统都位于一个世界之中，世界内的组件都是互相可见的
        * Resource 资源，整个世界内被共享的数据，同种类型数据在一个世界内只有一份，这与组件不同。典型应用是配置文件。
        * Storage 存储，一类Component被一个Storage进行持有并存储，一般来说会有不同类型的Storage实现，同时Storage也是作为一种Resource存在的
        * Scheduler 调度器，用于调度整个世界中系统的运行，根据指定的依赖关系以及对组件的访问需求进行自动协调，并发执行
    * 优点
        * 游戏行业原生框架，适用性很高
        * 基于SOA(Structure of Array，对比Array Of Structure)，对于缓存友好，执行效率高
          ```rust
          struct SOA {
              names:Vec<String>,
              ages:Vec<u8>,   
          }
          
          
          struct AOS {
              name:String,
              age:u8,
          }
          static data:Vec<AOS> = Vec::new();
          ```
        * 数据与逻辑相分离，天然具有可塑性，可适应不同的线程模型，同时为代码动态更新提供基础
        * 天然具有多线程工作能力
    * 缺点
        * 与传统开发模式有思维方式上的转变，有学习成本
        * 设计不当的情况下可能使得整个调度退回成单线程模式
* Rust
    * 优点
        * 执行效率高，从benchmarkgames上的数据来看，已经超过c++
        * 内存安全，不会因为业务逻辑代码（safe code)的失误而使程序宕机，相当于可以try/catch panic的C++
        * 线程安全，可以放心的使用多线程技术而不用担心竞争等问题
        * 框架代码可以使用unsafe代码，尽可能的提高框架易用性，逻辑代码限定只能使用safe代码，让编译器帮忙检查所有潜在问题
        ```rust
        #![deny(unsafe_code)]
        ```
        * trait特性可以在struct定义之外再新加封装，只要所有的struct属性都是public的
        ```rust
        pub trait AddMoney {
            fn add_money(&mut self, money:i32);
        }
     
        impl AddMoney for UserInfo {
            fn add_money(&mut self, money:i32) {
                self.money += money;
            }
        }
        ```
        上面示例中UserInfo就是一个Component，由于逻辑与数据相分离，我们不会在Component的定义中添加add_money方法，而只会在其他的实现system
        中添加新的方法。注意，一些公用方法除外，那些方法还是需要留在数据定义的地方。
    * 缺点
        * 学习成本高，学习曲线陡
        * 编译速度慢，影响开发迭代速度
    
* Rust的ECS实现们
    * specs
        * 优点
            * 成熟度比较高，各方面比较平衡
            * 接口设计合理，扩展性比较好
        * 缺点
            * 接口对新人不友好
            * 模板代码过多
        * 示例
        ```rust
        #[derive(Default)]
        struct UserTestSystem {
            lib: DynamicSystem<fn(&UserInfo, &BagInfo)>,
        }

        impl UserTestSystem {
            pub fn setup(
                mut self,
                world: &mut World,
                builder: &mut DispatcherBuilder,
                dm: &DynamicManager,
            ) {
                world.register::<UserInfo>();
                world.register::<BagInfo>();
                self.lib.init("".into(), "".into(), dm);
                builder.add(self, "user_test", &[]);
            }
        }

        impl<'a> System<'a> for UserTestSystem {
            type SystemData = (
                ReadStorage<'a, UserInfo>,
                ReadStorage<'a, BagInfo>,
                Read<'a, DynamicManager>,
            );

            fn run(&mut self, (user, bag, dm): Self::SystemData) {
                if let Some(symbol) = self.lib.get_symbol(&dm) {
                    for (user, bag) in (&user, &bag).join() {
                        (*symbol)(user, bag);
                    }
                } else {
                    todo!()
                }
            }
        }
        ```
    * legion
        * 优点
            * 接口友好
            * 模板代码少
        * 缺点
            * 成熟度低，适用于客户端使用场景的ecs（本身创意也来自unity的jobs）
            * 各方面不平衡，当component数目超过100之后，性能会有急剧下降
            * 添加新的Component算法复杂度不是O(1)
        * 示例
        ```rust
        #[system]
        fn user_system(user:&UserInfo, bag:&BagInfo) {}
        ```
      
* 方案
    * ECS框架方面，经过各方面测试，最终决定使用specs，specs的两个缺点实际上可以参考legion的实现来自己通过proc_macro来进行扩展
    同时还可以根据我们的实际需求来进行调整, 这样一来可以使得ECS的思维习惯方面的缺点降到最低
      
    * ECS设计导致的调度问题，只需要在component层面进行数据分解操作就可以了，对于整体影响比较小，另外还可以通过system来监控执行时间确认整体的设计是否合理
    
    * 采用上面的方案之后，每个业务只需要根据不同的请求来实现对应的函数即可，对于Rust而言，如果不涉及到持有引用，那我们几乎不会遇到复杂的生命周期问题
    所以，这个方案也在一定程度上降低了rust的使用成本，同时我们也要求逻辑代码中不许使用unsafe代码来加强整个代码的安全性
      
    * ECS的代码与数据分离的设计，使得我们可以将具体的system实现放到一个个独立的动态lib库里实现，这样一来每个业务代码相互独立，单个工程编译简单，
    并且可以直接编译成动态链接库，从而实现动态更新
      
    
## 需求抽象
我们从客户端发出请求的角度来对所有请求类型进行总结，会发现以下三类
* 玩家自身请求  
这类请求一般都只涉及到当前用户自己或者其他某个人的数据读取以及修改，这是客户端对服务器请求中最常见，同时也是数量最多的请求，大概占比能在9成以上
  
* 玩家与另外一个玩家请求  
这类请求一般会涉及到当前用户自己以及另外一个用户，比如卡牌中常见的战斗请求

* 玩家与另外一类实体请求请求  
这类请求一般会涉及到另外一类实体，比如公会，比如场景
  
* 玩家与另外一组目标
这类请求主要是在战斗中使用，比如索敌
  
## 需求实现
* 玩家自身请求  
这类请求是最容易用ECS来实现的，直接取到需要操作的component来进行数据操作就可以了,我们只需要实现如下类似就可以了
  ```rust
  #[system]
  fn single_target(input:&SingleInput, user:&UserInfo, bag:&BagInfo){}
  ```
  
* 玩家与另外一个玩家请求
这类请求也相对容易，只需要取到两个人相关的component来作为参数即可，与单人目标类似
  ```rust
  #[system]
  fn double_target(input:&DoubleInput, (source_user, target_user):(&UserInfo, &UserInfo), (source_bag, target_bag):(&BagInfo, &BagInfo)){}
  ```
  当然，上面的形式太啰嗦，实际上声明的时候，我们可以直接写成如下
  ```rust
  #[system(double)]
  fn single_target(#[double(player)] input:&DoubleInput, user:&UserInfo, bag:&BagInfo){} 
  ```
  最终宏系统会自动推导成上面的形式
  
* 玩家与另外一类实体请求
这类请求，一般来说需要进行特殊分析，目前能够想到的一种方式是这样的, 比如单人需要操作公会对象
  ```rust
  #[system(multi)]
  fn multi_target(user:&UserInfo, #[multi(users)] guild:&GuildInfo){}
  ```
    还有一类是MMO相关场景操作，比如
    ```rust
    #[system(multi)]
    fn multi_target(user:&UserInfo, pos:&mut Position, scene:&mut Scene){}
    ```
    这种类型其实也可以自动生成模板代码，利用par_join让所有的场景在单独线程并行执行，大致如下：
    ```rust
    impl<'a> System<'a> for EnterSceneSystem {
        type SystemData = (
            ReadStorage<'a, DiffType<Fake<1>, 1>>,
            WriteStorage<'a, DiffType<Fake<2>, 2>>,
            WriteStorage<'a, Scene>,
        );
    
        fn run(&mut self, (f1, mut f2, mut scene): Self::SystemData) {
            (&scene, ).par_join().for_each(|(scene, )| {
                for (f1, f2, _) in (&f1, &f2, &scene.players).join() {
                    unsafe {
                        #[allow(mutable_transmutes)]
                            test_scene(f1, std::mem::transmute(f2), std::mem::transmute(scene));
                    }
                }
            });
        }
    }
    ```
  
* 玩家与另外一组实体
  这种形式已经不是业务逻辑的范畴了，所以我们不在这里进行讨论，可以直接使用ecs底层代码来进行实现

## 动态链接库管理
* Rust的动态链接库可以通过将crate-type设置成cdylib来生成，需要注意的是，这种情况下，引用的其他外部库都是静态链接的形式被
  写入了当前库，除非是纯C类型的函数指针并且声明了extern。所以对于纯粹的rust类型来说，各个动态链接库project之间也是可以互相引用的，
  但是需要注意编译的时候可能出现版本不一样的情况，这是一个需要考虑的版本管理问题。目前已知的情况是即使代码未发生变化，但是如果完全重新编译
  生成的binary的checksum也会不一致。一个简单粗暴的办法是打包机全次都编译全部的依赖库。依靠版本号
* DynamicManager作为resource为所有的system提供动态链接库支持，具体代码如下
```rust
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
```
* Library是一个封装，用于代码一个lib库，具体如下
```rust
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
```
* DynamicSystem是一个基类，所有希望拥有动态链接库支持的System里都应该有一个成员变量是这个类型，具体如下
```rust
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
```

## 数据同步
* Mutable是一个wrapper，它用于将所有的具体类进行封装，当需要一个mut引用时，Mutable会自动记录该类型T对应的Storage已经被修改，同时将当前对象
  clone一份作为以后修改对比
```rust
pub struct Mutable<T, const N: usize> {
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

pub const MAX_COMPONENTS: usize = 1024;

lazy_static::lazy_static! {
    pub static ref MODS:Vec<AtomicBool> = {
        let mut mods = Vec::with_capacity(MAX_COMPONENTS);
        for i in 0..MAX_COMPONENTS {
           mods.push(AtomicBool::new(false));
        }
        mods
    };
}
```  
* CommitChangeSystem是一个用于检查所有Component是否经过修改的模板System，我们在系统启动的时候自动加上这些检查
```rust
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
        for (data,) in (&data,).join() {
            todo!()
        }
        Mutable::<T, N>::reset();
    }
}
```

## 组件的创建
* 初始直接全部创建，最简单，但是可能占用内存会稍高一些

* 需要时再创建
这种情况下，上面的函数应该需要有个返回值，如果有返回值则进行创建，或者直接用LazyUpdate来进行，这种方式的缺点在于组件只能在下一帧才能被访问到

## systemn属性
system属性用于生成各种模板代码，主要功能如下：
* System对象，包括动态链接支持以及状态字段
* 实现setup代码，包括component的注册以及动态库初始化，最后把自己加入scheduler里
* 实现System接口，具体包括
    * 定义用于收集返回值以及清除input的数组
    * 循环整个定义的component，并调用实际处理函数
    * 收集返回值以及当前的entity
    * 插入新创建的component以及清除input组件
### system属性补充  
关于各种目标的示例在上面已经讲过了，下面再补充一些其他的未涉及到的属性
* resource
```rust
#[system]
fn test(#[resource] counter:&usize, user:&UserInfo){}
```
这种属性会出现在参数变量前，代表这个参数是一个Resource而不是Component，要求这种类型都必须实现了Default接口
* state
```rust
#[system]
fn test(#[state] counter:&usize, user:&UserInfo){}
```
这种属性会出现在参数变量前，代表这个参数是当前System的成员变量，它作为一个状态提供给使用者
* dynamic
```rust
#[system]
#[dynamic(lib = "native", func = "test")]
fn test1(user:&UserInfo){}
```
这是参数最全的一种方式，表明test1函数实际通过动态链接库来实现，system属性生成代码时会忽略掉这个函数目前的具体实现，也即test1将不会存在于编译完的
代码中。而具体实现在叫native的动态链接库中，并且symbol的名字叫test

```rust
#[system]
#[dynamic(lib = "xxx")]
fn test2(user:&UserInfo){}
```
这是上面形式的一种省略形式，表示func默认就是test2

```rust
#[system]
#[dynamic]
fn user_test(user:&UserInfo){}
```
这是一种更省略的方式，代表lib=user, func = test，注意这种方式下，如果名字不带下划线，如test，则lib = test, func = test

```rust
#[system]
fn test(user:&UserInfo){}
```
因为我们鼓励所有的system都是动态链接的，所以dynamic属性是默认的，如上代表lib = test, func = test
* static
```rust
#[system]
#[static]
fn test(user:&UserInfo){}
```
代表这是一个静态实现，不要忽略test函数，将它编译成代码中，并且在System具体实现中调用它。

#export属性
如果panic在动态链接库里并且未被catch而在调用中catch会导致调用者abort，因此设计了export这个属性来完成以下工作
* 自动生成转成extern函数，并加上no_mangle的标签
* 自动将写引用转成读引用，并调用catch_unwind，同时在调用中用transmute再转回可写引用
* 添加类型检查代码以备类型检查

# 其他关键模块
## 网络层
基于mio库来实现一个完全的单线程模型，此模型只做网络分发，不做任何其他编解码的工作，这样一来单线程完全可以胜任全部的工作。
网络层与ecs核心层之间通过channel来通信，ecs层的消息可以通过mio提供的Waker来通知mio有新的数据需要发送，而新的请求则完全靠
mio的Poll就可以了。
TBD

## 数据层
rust有一个优秀的数据ORM库，diesel，它实现的功能跟我们目前用go实现的差不多的功能，可以自动比对数据库结构，自动生成更新语句，自动映射等。
TBD

# 典型应用场景实现
希望大家可以fork本工程出来，然后实现一些常见功能并补充到下面
## 背包
## 工会
## 大世界移动

TBD
* ~~同一个component不能同时出现在input和output里，加上这个检查~~
* 有可能input有没匹配上的，需要加日志