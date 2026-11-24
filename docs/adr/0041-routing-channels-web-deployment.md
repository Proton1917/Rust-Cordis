# ADR-0041 服务路由、通道与页面部署

cordis-routing 处理对象编号、转交及调用。cordis-channels 管理转发授权与通道寿命，cordis-web-deployment 协调页面资源、实例与替换。

独立页面连接丢失时，由内核撤回其资源；全栈更新时网页模块和后端各由执行端替换，运行时协调共同结果。

## 框架装配的依赖与所有权

网页资源 Owner 是声明依赖的内核组件。HTTP 登记使用 Owner 已提交的 `cordis.web@1` 绑定，保留该绑定实际选择的隔离域和远端对象。登记、激活和撤回属于同一个提供方；提供方替换时，内核先等待旧登记清理，再让 Owner 使用新绑定加载。

全栈 Owner 额外声明类型化服务 `InstanceLifecycle`。监管器为每个实例分配该服务的私有隔离域，服务名固定为 `cordis.instance`。域身份复用现有 `RealmLease`：提供方和等待中的 Owner 共同持有租约，最后一个视图释放时归还登记。实例域与插件可选择的命名域采用独立键类别，插件命名域不参与实例生命周期绑定。

服务值 `InstanceGeneration` 弱持对应的远端加载代。Owner 从自己的已提交绑定取得该值，再从这一代的描述符与上下文直接取得后端组件和父上下文。组件名称只在实际装配网页时复制。每个远端加载代与网页 Owner 捕获一次共享纪元，使用 `Context::for_epoch` 派生视图。内核在同一把 `State` 锁中检查该视图的全部纪元约束并登记效应，使子挂载、服务和监听器登记取得一致的准入点。换代已完成的旧视图继续指向旧代，登记以失活错误结束。挂载已创建的临时子记录在准入失败后由现有释放流程回收。

同一纤程上的扩展、隔离与拦截视图继承纪元约束；子组件拥有新的纤程作用域，寿命由父挂载效应管理，子代可以捕获自己的纪元。普通 `Context` 保留原有存活契约。纪元约束限制新增效应，清理仍可读取已提交服务并完成旧登记撤回。`Context::is_current` 包含视图约束，`Fiber::is_current_epoch` 读取纤程自身的一致状态。监管器在远端配置校验前利用视图检查提前返回 code 4；跨异步边界的最终准入归内核效应登记。

生命周期依赖继续使用普通服务绑定、隔离、声明注入及效应清理顺序。内核服务名保留表只登记一个固定类型键；每代的私有视图与身份值随其所有者释放。网页激活直接读取两个依赖，省去全实例目录快照和按名字再次选择后端的路径。UP/1、SDK 和页面组件的业务接口保持现行版本。

对应回归位于 `cordis-web-deployment/tests/bindings.rs`、`cordis-supervisor/tests/manager.rs` 的 `core_owner_tracks_backend_presence_and_releases_page`，以及 `manager/views.rs` 的生命周期域租约测试。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0041-routing-channels-web-deployment.md)
