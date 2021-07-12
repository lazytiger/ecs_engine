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
      
    
## 实体类型
* 我们可以从概念上将游戏中的实体分类，比如玩家，NPC，公会，场景等等
* 这些实体之间可能从结构关系，比如一个玩家可以属于一个公会，或者一个玩家在一个场景中等
* 这些实体之间的关系一般来说是一种层级结构Hierarchy，可以直接使用specs-hierarchy来解决


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
  
* 关于日志模块，由于rust对于动态链接库的策略是在ffi边界上完全拷贝，所以我们在exe里初始化了日志之后dll里的日志并没有初始化，
  所以这就需要我们在dll加载的时候主动调用一下日志初始化模块，具体参考dlog。
  
* DynamicManager作为resource为所有的system提供动态链接库支持
* Library是一个封装，用于代理一个lib库
* DynamicSystem是一个基类，所有希望拥有动态链接库支持的System里都应该有一个成员变量是这个类型


## 组件的创建
* 初始直接全部创建，最简单，但是可能占用内存会稍高一些

* 需要时再创建
这种情况下，上面的函数应该需要有个返回值，如果有返回值则进行创建，或者直接用LazyUpdate来进行，这种方式的缺点在于组件只能在下一帧才能被访问到

## systemn属性
system属性用于生成各种模板代码，主要功能如下：
* System对象，包括动态链接支持以及状态字段
* 实现setup代码，包括component的注册以及动态库初始化，最后把自己加入scheduler里
* 实现System接口，具体包括
    * 定义用于收集已经处理过的entity的vector，如果有input的话
    * 定义于用收集output结果的vector，如果有output的话
    * 循环整个定义的component，并调用实际处理函数，如果有output的话，匹配时取反
    * 根据返回值收集新的component
    * 清除这个input storage里已经处理过的component，然后再查看是否还有未匹配的input，如果有则打日志报错
    * 插入收集到的所有新的component并插入
    * 如果WriteComponent确实被触发，则设置这个Component为dirty，如果确认一个类是否是changeset?
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
#[dynamic(false)]
fn test(user:&UserInfo){}
```
代表这是一个静态实现，不要忽略test函数，将它编译成代码中，并且在System具体实现中调用它。

#export属性
如果panic在动态链接库里并且未被catch而在调用中catch会导致调用者abort，因此设计了export这个属性来完成以下工作
* 自动生成转成extern函数，并加上no_mangle的标签
* 自动加上catch_unwind防止panic
* 添加类型检查代码以备类型检查

# 其他关键模块
## 网络层
基于mio库来实现一个完全的单线程模型，此模型只做网络分发，不做任何其他编解码的工作，这样一来单线程完全可以胜任全部的工作。
网络层与ecs核心层之间通过channel来通信，ecs层的消息可以通过mio提供的Waker来通知mio有新的数据需要发送，而新的请求则完全靠
mio的Poll就可以了。
* 请求协议
  
    | 包体长度 | 命令id | 包体 |
    | --- | --- | --- |
    | 4 bytes(n)| 4 bytes | (n-4) bytes |
* 响应协议
  
    | 包体长度 | 实体id | 命令id | 包体 | 
    | --- | --- | --- | --- |
    | 4 bytes(n) | 4 bytes| 4 bytes | (n-8) bytes |

  
## 数据层
rust有一个优秀的数据ORM库，diesel，它实现的功能跟我们目前用go实现的差不多的功能，可以自动比对数据库结构，自动生成更新语句，自动映射等。
TBD

## 数据集与组件
* 一个组件即是一个数据集，担任与客户端的同步最小单元
* 因为数据集里可能是个数组，比如物品，比如技能等等，所以需要考虑如何同步数组
* 比如数组内元素的增删改，因此需要有标识，主要是如何同步删除信息
* pb3实现了对map的支持，因此我们利用这个特性可以比较容易的实现数组同步
    * 首先要求所有的数组都用map来实现，因为我们要利用key来实现同步
    * 另外要求所有数组的value也必须是一个message类型
    * 所有的message都自动添加一个mask字段用于标识哪些字段进行了修改
    * 编码时，添加了如下代码：
    ```rust
    fn write_to_with_cached_sizes(&self, os: &mut ::protobuf::CodedOutputStream<'_>) -> ::protobuf::ProtobufResult<()> {
        if self.is_dirty_field(1) {
            if self.x != 0. {
                os.write_float(1, self.x)?;
            }
        }
        if self.is_dirty_field(2) {
            if self.y != 0. {
                os.write_float(2, self.y)?;
            }
        }
        if self.is_dirty_field(3) {
            ::protobuf::rt::write_map_with_cached_sizes::<::protobuf::types::ProtobufTypeUint32, ::protobuf::types::ProtobufTypeMessage<Test>>(3, &self.tests, os)?;
        }
        if self.mask != 0 {
            os.write_uint64(4, self.mask)?;
        }
        os.write_unknown_fields(self.get_unknown_fields())?;
        ::std::result::Result::Ok(())
    }
    ```
    * 解码时，比较复杂，大致流程如下：
    ```rust
    fn merge_from(&mut self, is: &mut ::protobuf::CodedInputStream<'_>) -> ::protobuf::ProtobufResult<()> {
        let mut mask = 0u64;
        self.mask = 0;
        self.tests.iter_mut().for_each(|(_, v)|v.mask = 1);
        while !is.eof()? {
            let (field_number, wire_type) = is.read_tag_unpack()?;
            match field_number {
                1 => {
                    if wire_type != ::protobuf::wire_format::WireTypeFixed32 {
                        return ::std::result::Result::Err(::protobuf::rt::unexpected_wire_type(wire_type));
                    }
                    let tmp = is.read_float()?;
                    self.x = tmp;
                    mask |= 1 << 1;
                },
                2 => {
                    if wire_type != ::protobuf::wire_format::WireTypeFixed32 {
                        return ::std::result::Result::Err(::protobuf::rt::unexpected_wire_type(wire_type));
                    }
                    let tmp = is.read_float()?;
                    self.y = tmp;
                    mask |= 1 << 2;
                },
                3 => {
                    ::protobuf::rt::read_map_into::<::protobuf::types::ProtobufTypeUint32, ::protobuf::types::ProtobufTypeMessage<Test>>(wire_type, is, &mut self.tests)?;
                    mask |= 1 << 3;
                },
                4 => {
                    if wire_type != ::protobuf::wire_format::WireTypeVarint {
                        return ::std::result::Result::Err(::protobuf::rt::unexpected_wire_type(wire_type));
                    }
                    let tmp = is.read_uint64()?;
                    self.mask = tmp;
                    mask |= 1 << 4;
                },
                _ => {
                    ::protobuf::rt::read_unknown_or_skip_group(field_number, wire_type, is, self.mut_unknown_fields())?;
                },
            };
        }
        self.mask &= ! mask;
        while self.mask != 0 {
            let field_number = self.mask.trailing_zeros();
            match field_number {
                1 => {
                    self.clear_x();
                    self.clear_dirty_field(1);
                },
                2 => {
                    self.clear_y();
                    self.clear_dirty_field(2);
                },
                3 => {
                    self.clear_tests();
                    self.clear_dirty_field(3);
                },
                _ => {
                    return Err(::protobuf::ProtobufError::WireError(::protobuf::error::WireError::Other));
                },
            };
        }
        let keys:Vec<_> = self.tests.iter().filter_map(|(k, v)|if v.mask == 1 { Some(k.clone()) } else { None }).collect();
        keys.iter().for_each(|k|{self.tests.remove(k);});
        ::std::result::Result::Ok(())
    }
    ```
    * 如果需要完整的全部数据时，应该clone一份数据出来，然后再调用mask_all方法，然后再编码
    * 客户端在使用时，需要分辨哪些数据是新加的，哪些数据是删除的，以便针对资源层作为相应的逻辑，这一点可以使用mask来进行标记，大致可能需要如下状态,
      由于protobuf里field number从1开始，所以最低位是无用的，再加上最高位，我们取两位作为标记位，则可以得到如下4种状态
        * 删除标记(包括先增后删，为什么不需要保留增加标记？），可以用0b01来进行标识
        * 增加标记，可以用0b10来进行标记
        * 先删后增(为什么需要两个标记同时保留？），应该同时具有，0b11
        * 单纯修改，0b00

## 乱序与覆盖
按照目前的实现来说，虽然从数据的角度来看是安全并且高效的在执行，但是从玩家的角度来看，存在乱序以及请求覆盖的风险。
一般来说，请求覆盖是符合预期的，但是乱序不一定是符合预期的，关于这一点在处理上有两种可能性
* 请求设计的时候尽可能避免乱序可能带来的风险
* 在框架层添加限制，同一帧内只处理玩家一个请求

# 典型应用场景实现
希望大家可以fork本工程出来，然后实现一些常见功能并补充到下面
## 背包
## 工会
## 大世界移动

TBD
* ~~同一个component不能同时出现在input和output里，加上这个检查~~
* ~~有可能input有没匹配上的，需要加日志~~
* 离线用户数据如何处理？
* 数据集，包括标脏以及同步
* 数据库，包括持久化以及拉取
* 读取请求数据从RunNow移到System里去，利用SystemData生成
* 增加统计类System支持
* ~~重命名component为dataset~~
* input改为使用drain