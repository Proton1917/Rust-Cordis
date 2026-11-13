//! DOC-15 §6.1、DOC-16 §6：服务参数和结果的资源编号重写。

use crate::{Arguments, Batch, CallContext, LocalService, Result, Router, decode, error, value};
use cordis_up::{
    Value, payload,
    rpc::{Connection, Outgoing, Payload, RemoteObject},
};
use std::{collections::HashMap, sync::Arc};

impl<R: Send + Sync + 'static> Router<R> {
    /// DOC-16 §6：同一载荷中的重复资源共用一个出口，失败由 Batch 回收。
    pub fn transfer(
        &self,
        data: Value,
        incoming: &Payload,
        recipient: &Connection,
        route: impl Fn(RemoteObject) -> Arc<R>,
    ) -> Result<(Value, Batch<R>)> {
        let mut batch = self.batch(recipient.id());
        let mut translated = HashMap::new();
        let data = rewrite(data, &mut |id| {
            if let Some(mapped) = translated.get(&id) {
                return Ok(*mapped);
            }
            let object = incoming.import(id)?;
            let owner = object.peer().id();
            let mapped = self.export(recipient, route(object), Some(owner), &mut batch)?;
            translated.insert(id, mapped);
            Ok(mapped)
        })?;
        Ok((data, batch))
    }

    /// DOC-15 §6.1：纪元持有由调用方保证，路由器转交实际远程调用和结果。
    pub async fn invoke_remote(
        &self,
        caller: &Connection,
        object: &RemoteObject,
        request: payload::Invoke,
        incoming: &Payload,
        route: impl Fn(RemoteObject) -> Arc<R>,
    ) -> Result<Outgoing> {
        let (args, batch) =
            self.transfer(Value::Array(request.args), incoming, object.peer(), &route)?;
        let Value::Array(args) = args else {
            unreachable!("argument array preserved")
        };
        let outgoing = value(&payload::Invoke {
            object: object.id(),
            method: request.method,
            args,
            refs: batch.ids.clone(),
        })?;
        let result = object
            .peer()
            .request("invoke", batch.outgoing(outgoing), self.timeout)
            .await?;
        let response: payload::InvokeResult = decode(result.value())?;
        self.reply(response.value, &result, caller, route)
    }

    /// ADR-0017：内置函数在目录锁外执行，回复写出后再执行约定动作。
    pub async fn invoke_local(
        &self,
        caller: &Connection,
        service: &dyn LocalService,
        origin: &CallContext,
        request: payload::Invoke,
        incoming: &Payload,
        route: impl Fn(RemoteObject) -> Arc<R>,
    ) -> Result<Outgoing> {
        let objects = request
            .refs
            .iter()
            .map(|id| incoming.import(*id).map(|object| (*id, object)))
            .collect::<Result<_>>()?;
        let arguments = Arguments {
            values: request.args,
            objects,
        };
        let result = service.invoke(origin, &request.method, &arguments).await?;
        let outgoing = self.reply(result.value, incoming, caller, route)?;
        Ok(match result.after_write {
            Some(action) => outgoing.after_write(action),
            None => outgoing,
        })
    }

    fn reply(
        &self,
        data: Value,
        incoming: &Payload,
        recipient: &Connection,
        route: impl Fn(RemoteObject) -> Arc<R>,
    ) -> Result<Outgoing> {
        let (value, batch) = self.transfer(data, incoming, recipient, route)?;
        let outgoing = crate::value(&payload::InvokeResult {
            value,
            refs: batch.ids.clone(),
        })?;
        Ok(batch.outgoing(outgoing))
    }
}

/// DOC-16 §6：仅接口资源标签改写编号，其余标量和复合值保留原表示。
fn rewrite(data: Value, resource: &mut impl FnMut(u64) -> Result<u64>) -> Result<Value> {
    Ok(match data {
        Value::Tag(payload::OBJECT_TAG, id) => {
            let id = id
                .as_integer()
                .and_then(|id| u64::try_from(id).ok())
                .filter(|id| *id != 0)
                .ok_or_else(|| error(2, "资源标签需要正u64编号"))?;
            payload::ObjectRef(resource(id)?).into()
        }
        Value::Tag(tag, value) => Value::Tag(tag, Box::new(rewrite(*value, resource)?)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| rewrite(value, resource))
                .collect::<Result<_>>()?,
        ),
        Value::Map(values) => Value::Map(
            values
                .into_iter()
                .map(|(key, value)| Ok((rewrite(key, resource)?, rewrite(value, resource)?)))
                .collect::<Result<_>>()?,
        ),
        value => value,
    })
}
