use crate::{
    AbiBytes, AbiFuture, AbiOption, AbiState, AbiStr, AbiUnit, Buf, Environment, Error, Object,
    RemoteFuture, Result,
};

#[derive(Clone, Debug)]
pub struct State {
    pub schema: String,
    pub version: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy)]
pub struct HostClient {
    env: Environment,
}

impl HostClient {
    pub const fn new(env: Environment) -> Self {
        Self { env }
    }
    pub const fn environment(self) -> Environment {
        self.env
    }
    pub fn log(self, ctx: u64, level: u32, message: &str) -> Result<()> {
        // SAFETY: DOC-12 §9，借用message覆盖调用期，错误槽有效。
        unsafe {
            self.env
                .call_unit(|err| (self.env.raw_host().log)(ctx, level, AbiStr::new(message), err))
        }
    }
    pub fn now(self, ctx: u64) -> Result<u64> {
        // SAFETY: DOC-12 §9，now成功写出u64。
        unsafe {
            self.env
                .call(|out, err| (self.env.raw_host().now)(ctx, out, err))
        }
    }
    pub fn sleep(self, ctx: u64, nanos: u64) -> Result<RemoteFuture<AbiUnit>> {
        // SAFETY: DOC-12 §8、§9，sleep交出单位Future。
        let raw = unsafe {
            self.env
                .call(|out, err| (self.env.raw_host().sleep)(ctx, nanos, out, err))
        }?;
        Ok(self.future(raw))
    }
    pub fn spawn(self, ctx: u64, task: RemoteFuture<AbiUnit>) -> Result<()> {
        // SAFETY: DOC-12 §5、§9，进入宿主时转交任务及既有租约。
        unsafe {
            self.env
                .call_unit(|err| (self.env.raw_host().spawn)(ctx, task.into_raw(), err))
        }
    }
    pub fn effect(self, ctx: u64, label: &str, body: Object) -> Result<u64> {
        // SAFETY: DOC-12 §5、§9，body在进入调用时转交，输出是effect句柄。
        unsafe {
            self.env.call(|out, err| {
                (self.env.raw_host().effect)(ctx, AbiStr::new(label), body.into_raw(), out, err)
            })
        }
    }
    pub fn effect_async(self, ctx: u64, label: &str, body: Object) -> Result<u64> {
        // SAFETY: DOC-12 §5、§9，异步body及其租约转交宿主。
        unsafe {
            self.env.call(|out, err| {
                (self.env.raw_host().effect_async)(
                    ctx,
                    AbiStr::new(label),
                    body.into_raw(),
                    out,
                    err,
                )
            })
        }
    }
    pub fn defer(self, scope: u64, cleanup: Object) -> Result<()> {
        // SAFETY: DOC-12 §5、§9，cleanup在进入调用时转交。
        unsafe {
            self.env
                .call_unit(|err| (self.env.raw_host().defer)(scope, cleanup.into_raw(), err))
        }
    }
    pub fn defer_async(self, scope: u64, cleanup: Object) -> Result<()> {
        // SAFETY: DOC-12 §5、§9，异步cleanup在进入调用时转交。
        unsafe {
            self.env
                .call_unit(|err| (self.env.raw_host().defer_async)(scope, cleanup.into_raw(), err))
        }
    }
    pub fn is_current(self, ctx: u64) -> Result<bool> {
        // SAFETY: DOC-12 §9，is_current成功写出ABI布尔。
        let value = unsafe {
            self.env
                .call(|out, err| (self.env.raw_host().is_current)(ctx, out, err))
        }?;
        match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::new(2, "invalid ABI boolean")),
        }
    }
    pub fn provide(self, ctx: u64, interface: &str, value: Object) -> Result<()> {
        // SAFETY: DOC-12 §5、§9，接口对象及租约转交宿主。
        unsafe {
            self.env.call_unit(|err| {
                (self.env.raw_host().provide)(ctx, AbiStr::new(interface), value.into_raw(), err)
            })
        }
    }
    pub fn get(self, ctx: u64, interface: &str) -> Result<Object> {
        // SAFETY: DOC-12 §9，get交出已注入服务的拥有引用。
        let raw = unsafe {
            self.env
                .call(|out, err| (self.env.raw_host().get)(ctx, AbiStr::new(interface), out, err))
        }?;
        // SAFETY: DOC-12 §5、§11，接管宿主交出的对象及既有租约。
        Ok(unsafe { Object::from_raw(raw, self.env) })
    }
    pub fn on(self, ctx: u64, event: &str, listener: Object) -> Result<()> {
        // SAFETY: DOC-12 §5、§9，监听器对象进入宿主后由其管理。
        unsafe {
            self.env.call_unit(|err| {
                (self.env.raw_host().on)(ctx, AbiStr::new(event), listener.into_raw(), err)
            })
        }
    }
    pub fn emit(self, ctx: u64, event: &str, payload: &[u8]) -> Result<()> {
        // SAFETY: DOC-12 §5、§9，名称与载荷借用覆盖同步调用。
        unsafe {
            self.env.call_unit(|err| {
                (self.env.raw_host().emit)(ctx, AbiStr::new(event), AbiBytes::new(payload), err)
            })
        }
    }
    pub fn emit_async(
        self,
        ctx: u64,
        event: &str,
        payload: &[u8],
    ) -> Result<RemoteFuture<AbiUnit>> {
        // SAFETY: DOC-12 §8、§9，宿主在返回Future前复制载荷。
        let raw = unsafe {
            self.env.call(|out, err| {
                (self.env.raw_host().emit_async)(
                    ctx,
                    AbiStr::new(event),
                    AbiBytes::new(payload),
                    out,
                    err,
                )
            })
        }?;
        Ok(self.future(raw))
    }
    pub fn export_state(
        self,
        ctx: u64,
        schema: &str,
        version: u32,
        exporter: Object,
    ) -> Result<()> {
        // SAFETY: DOC-12 §5、§9，导出器对象在进入宿主时转交。
        unsafe {
            self.env.call_unit(|err| {
                (self.env.raw_host().export_state)(
                    ctx,
                    AbiStr::new(schema),
                    version,
                    exporter.into_raw(),
                    err,
                )
            })
        }
    }
    pub fn inherited_state(self, ctx: u64) -> Result<Option<State>> {
        // SAFETY: DOC-12 §9，宿主交出具有有效标志与拥有字段的可选状态。
        let raw: AbiOption<AbiState> = unsafe {
            self.env
                .call(|out, err| (self.env.raw_host().inherited_state)(ctx, out, err))
        }?;
        // SAFETY: DOC-12 §5，宿主结果遵守AbiOption的结构契约。
        let Some(raw) = (unsafe { raw.take() }) else {
            return Ok(None);
        };
        // SAFETY: DOC-12 §5，先接管两个拥有缓冲再解码模式名。
        let schema = unsafe { Buf::from_raw(raw.schema, self.env) };
        // SAFETY: DOC-12 §5，数据字段具有独立所有权与租约。
        let data = unsafe { Buf::from_raw(raw.data, self.env) };
        Ok(Some(State {
            schema: schema.into_string()?,
            version: raw.version,
            data: data.into_vec()?,
        }))
    }
    pub fn config(self, ctx: u64) -> Result<Vec<u8>> {
        // SAFETY: DOC-12 §9，config输出拥有CBOR缓冲。
        let raw = unsafe {
            self.env
                .call(|out, err| (self.env.raw_host().config)(ctx, out, err))
        }?;
        // SAFETY: DOC-12 §5，宿主转交缓冲及其租约。
        unsafe { Buf::from_raw(raw, self.env) }.into_vec()
    }
    pub fn spawn_thread(self, ctx: u64, name: &str, body: Object) -> Result<()> {
        // SAFETY: DOC-12 §5、§9，线程体对象转交宿主并随效应join。
        unsafe {
            self.env.call_unit(|err| {
                (self.env.raw_host().spawn_thread)(ctx, AbiStr::new(name), body.into_raw(), err)
            })
        }
    }
    pub fn lease_acquire(self, library: u64) -> Result<()> {
        self.env.acquire(library)
    }
    pub fn lease_release(self, library: u64) -> Result<()> {
        self.env.release(library)
    }
    pub fn effect_settled(self, effect: u64) -> Result<RemoteFuture<AbiUnit>> {
        // SAFETY: DOC-12 §9，settled交出单位结果的Future。
        let raw = unsafe {
            self.env
                .call(|out, err| (self.env.raw_host().effect_settled)(effect, out, err))
        }?;
        Ok(self.future(raw))
    }
    pub fn effect_dispose(self, effect: u64) -> Result<RemoteFuture<AbiUnit>> {
        // SAFETY: DOC-12 §9，dispose交出等待清理的单位Future。
        let raw = unsafe {
            self.env
                .call(|out, err| (self.env.raw_host().effect_dispose)(effect, out, err))
        }?;
        Ok(self.future(raw))
    }
    pub fn object_export(self, object: Object) -> Result<u64> {
        // SAFETY: DOC-12 §9，进入宿主时转交对象引用，成功写出宿主分配的资源编号。
        unsafe {
            self.env
                .call(|out, err| (self.env.raw_host().object_export)(object.into_raw(), out, err))
        }
    }
    pub fn object_import(self, id: u64) -> Result<Object> {
        // SAFETY: DOC-12 §9，宿主克隆资源表中的对象，成功交出独立拥有引用。
        let raw = unsafe {
            self.env
                .call(|out, err| (self.env.raw_host().object_import)(id, out, err))
        }?;
        // SAFETY: DOC-12 §5、§11，接管import产生的对象及租约。
        Ok(unsafe { Object::from_raw(raw, self.env) })
    }
    pub fn object_release(self, id: u64) -> Result<()> {
        // SAFETY: DOC-12 §9，宿主移出资源表引用并在锁外释放。
        unsafe {
            self.env
                .call_unit(|err| (self.env.raw_host().object_release)(id, err))
        }
    }
    pub fn import_resource(self, value: &crate::CborValue) -> Result<Object> {
        if let crate::CborValue::Tag(crate::RESOURCE_TAG, value) = value {
            if let crate::CborValue::Integer(id) = value.as_ref() {
                return self.object_import(
                    u64::try_from(*id).map_err(|error| Error::new(2, error.to_string()))?,
                );
            }
        }
        Err(Error::new(
            2,
            "resource requires tag 60000 and a u64 identifier",
        ))
    }
    fn future(self, raw: AbiFuture) -> RemoteFuture<AbiUnit> {
        // SAFETY: DOC-12 §8、§9，调用本辅助函数的Host方法均产生单位结果。
        unsafe { RemoteFuture::from_raw(raw, self.env) }
    }
}
