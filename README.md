# 基于ECS的服务器设计

## 设计目标
* 适用性 - 能够适应目前已知的所有游戏类型
* 性能 - 能够提供有竞争力的服务器性能
* 在线更新 - 适应游戏开发的快速迭代
* 扩展性 - 能够

## 技术选型
* ECS
    * 介绍
        * 
    * 优点
        * 游戏行业原生框架，适用性很高
        * 基于SOA，对于缓存友好，执行效率高
        * 数据与逻辑相分离，为代码动态更新提供基础
        * 天然具有多线程工作能力
    * 缺点
        * 与传统开发模式有思维方式上的转变，有学习成本
        * 设计不当的情况下可能使得整个调度退回成单线程模式
* Rust
    * 优点
        * 执行效率高，从benchmarkgames上的数据来看，已经超过c++
        * 内存安全，不会因为业务逻辑代码的失误而使程序宕机
        * 线程安全，可以放心的使用多线程技术而不用担心竞争等问题
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
    所以，这个方案也在一定程度上降低了rust的使用成本
      
    * ECS的代码与数据分离的设计，使得我们可以将具体的system实现放到一个个独立的动态lib库里实现，这样一来每个业务代码相互独立，单个工程编译简单，
    并且可以直接编译成动态链接库，从而实现动态更新
      
    
## 需求抽象
我们从客户端发出请求的角度来对所有请求类型进行总结，会发现以下三类
* 单人目标请求  
这类请求一般都只涉及到当前用户自己或者其他某个人的数据读取以及修改，这是客户端对服务器请求中最常见，同时也是数量最多的请求，大概占比能在9成以上
  
* 双人目标请求  
这类请求一般会涉及到当前用户自己以及另外一个用户，比如卡牌中常见的战斗请求

* 多人目标请求  
这类请求一般在MMO类游戏中比较常见，比如场景中移动，战斗，可能会影响到场景中全部的人
  
## 需求实现
* 单人目标请求  
这类请求是最容易用ECS来实现的，直接取到需要操作的component来进行数据操作就可以了,我们只需要实现如下类似就可以了
  ```rust
  #[system]
  fn single_target(input:&SingleInput, user:&UserInfo, bag:&BagInfo){}
  ```
  
* 双人目标请求
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
  
* 多人目标请求
这类请求，一般来说需要进行特殊分析，目前能够想到的一种方式是这样的, 比如单人需要操作公会对象
  ```rust
  #[system(multi)]
  fn multi_target(user:&UserInfo, #[multi(users)] guild:&GuildInfo){}
  ```
  其他更复杂的形式已经不是业务逻辑的范畴了，所以我们不在这里进行讨论，可以直接使用ecs底层代码来进行实现
