//! DOC-15 §6.1、DOC-16 §6：服务参数和结果的资源编号重写。

use crate::{Arguments, Batch, CallContext, LocalService, Result, Router, error};
use cordis_up::{
    Value, payload,
    rpc::{Connection, Outgoing, Payload, RemoteObject},
};
use std::{collections::HashMap, sync::Arc};

impl<R: crate::RouteLifetime + Send + Sync + 'static> Router<R> {
    /// DOC-16 §6：同一载荷中的重复资源共用一个出口，失败由 Batch 回收。
    pub fn transfer(
        &self,
        mut data: Value,
        incoming: &Payload,
        recipient: &Connection,
        route: impl Fn(RemoteObject) -> Arc<R>,
    ) -> Result<(Value, Batch<R>)> {
        let mut batch = self.batch(recipient.id());
        let mut translated = HashMap::new();
        rewrite(&mut data, &mut |id| {
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

    /// 当 `data` 直接取自 `incoming` 且解码未见 `OBJECT_TAG` 时，跳过整棵值树的资源重写扫描。
    fn transfer_incoming(
        &self,
        data: Value,
        incoming: &Payload,
        recipient: &Connection,
        route: impl Fn(RemoteObject) -> Arc<R>,
    ) -> Result<(Value, Batch<R>)> {
        if !incoming.has_resource_tags() {
            return Ok((data, self.batch(recipient.id())));
        }
        self.transfer(data, incoming, recipient, route)
    }

    /// DOC-15 §6.1：纪元持有由调用方保证，路由器转交实际远程调用和结果。
    pub async fn invoke_remote(
        &self,
        caller: &Connection,
        object: &RemoteObject,
        request: payload::Invoke,
        incoming: &Payload,
        route: impl Fn(RemoteObject) -> Arc<R>,
        response_route: impl Fn(RemoteObject) -> Arc<R>,
    ) -> Result<Outgoing> {
        let (args, batch) =
            self.transfer_incoming(Value::Array(request.args), incoming, object.peer(), &route)?;
        let Value::Array(args) = args else {
            unreachable!("argument array preserved")
        };
        let outgoing = payload::Invoke {
            object: object.id(),
            method: request.method,
            args,
            refs: batch.ids.clone(),
        }
        .into_value();
        let mut result = object
            .peer()
            .request("invoke", batch.outgoing(outgoing), self.timeout)
            .await?;
        let response = payload::InvokeResult::from_value_owned(result.take_value())
            .map_err(|problem| error(2, problem.to_string()))?;
        self.reply_incoming(response.value, &result, caller, response_route)
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

    fn reply_incoming(
        &self,
        data: Value,
        incoming: &Payload,
        recipient: &Connection,
        route: impl Fn(RemoteObject) -> Arc<R>,
    ) -> Result<Outgoing> {
        let (value, batch) = self.transfer_incoming(data, incoming, recipient, route)?;
        let outgoing = payload::InvokeResult {
            value,
            refs: batch.ids.clone(),
        }
        .into_value();
        Ok(batch.outgoing(outgoing))
    }

    fn reply(
        &self,
        data: Value,
        incoming: &Payload,
        recipient: &Connection,
        route: impl Fn(RemoteObject) -> Arc<R>,
    ) -> Result<Outgoing> {
        let (value, batch) = self.transfer(data, incoming, recipient, route)?;
        let outgoing = payload::InvokeResult {
            value,
            refs: batch.ids.clone(),
        }
        .into_value();
        Ok(batch.outgoing(outgoing))
    }
}

/// DOC-16 §6：仅接口资源标签改写编号，其余标量和复合值保留原表示。
fn rewrite(data: &mut Value, resource: &mut impl FnMut(u64) -> Result<u64>) -> Result<()> {
    match data {
        Value::Tag(payload::OBJECT_TAG, value) => {
            let id = value
                .as_integer()
                .and_then(|id| u64::try_from(id).ok())
                .filter(|id| *id != 0)
                .ok_or_else(|| error(2, "资源标签需要正u64编号"))?;
            **value = Value::Integer(resource(id)?.into());
        }
        Value::Tag(_, value) => rewrite(value, resource)?,
        Value::Array(values) => {
            for value in values {
                rewrite(value, resource)?;
            }
        }
        Value::Map(values) => {
            for (key, value) in values {
                rewrite(key, resource)?;
                rewrite(value, resource)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_up::ObjectRef;

    #[derive(Debug, PartialEq, Eq)]
    struct Allocation {
        kind: &'static str,
        address: usize,
        length: usize,
        capacity: usize,
    }

    fn allocations(value: &Value, output: &mut Vec<Allocation>) {
        let (kind, address, length, capacity) = match value {
            Value::Array(values) => (
                "array",
                values.as_ptr() as usize,
                values.len(),
                values.capacity(),
            ),
            Value::Map(fields) => (
                "map",
                fields.as_ptr() as usize,
                fields.len(),
                fields.capacity(),
            ),
            Value::Tag(_, value) => ("box", value.as_ref() as *const Value as usize, 1, 1),
            Value::Bytes(bytes) => (
                "bytes",
                bytes.as_ptr() as usize,
                bytes.len(),
                bytes.capacity(),
            ),
            Value::Text(text) => ("text", text.as_ptr() as usize, text.len(), text.capacity()),
            _ => return,
        };
        output.push(Allocation {
            kind,
            address,
            length,
            capacity,
        });
        match value {
            Value::Array(values) => {
                for value in values {
                    allocations(value, output);
                }
            }
            Value::Map(fields) => {
                for (key, value) in fields {
                    allocations(key, output);
                    allocations(value, output);
                }
            }
            Value::Tag(_, value) => allocations(value, output),
            _ => {}
        }
    }

    #[test]
    fn rewrite_preserves_nested_allocations_and_resource_order() {
        let mut bytes = Vec::with_capacity(128);
        bytes.extend([7, 8, 9]);
        let mut text = String::with_capacity(128);
        text.push_str("kept");
        let mut nested = Vec::with_capacity(8);
        nested.extend([ObjectRef(3).into(), Value::Bytes(bytes)]);
        let mut fields = Vec::with_capacity(8);
        fields.extend([
            (
                ObjectRef(2).into(),
                Value::Tag(99, Box::new(Value::Array(nested))),
            ),
            (Value::Text(text), ObjectRef(4).into()),
        ]);
        let mut values = Vec::with_capacity(8);
        values.extend([
            ObjectRef(1).into(),
            Value::Map(fields),
            Value::Tag(77, Box::new(ObjectRef(5).into())),
            Value::Null,
        ]);
        let mut value = Value::Array(values);
        let mut before = Vec::new();
        allocations(&value, &mut before);
        let mut visited = Vec::new();

        rewrite(&mut value, &mut |id| {
            visited.push(id);
            Ok(id + 100)
        })
        .unwrap();

        assert_eq!(visited, [1, 2, 3, 4, 5]);
        assert_eq!(
            value,
            Value::Array(vec![
                ObjectRef(101).into(),
                Value::Map(vec![
                    (
                        ObjectRef(102).into(),
                        Value::Tag(
                            99,
                            Box::new(Value::Array(vec![
                                ObjectRef(103).into(),
                                Value::Bytes(vec![7, 8, 9]),
                            ])),
                        ),
                    ),
                    (Value::Text("kept".into()), ObjectRef(104).into()),
                ]),
                Value::Tag(77, Box::new(ObjectRef(105).into())),
                Value::Null,
            ])
        );
        let mut after = Vec::new();
        allocations(&value, &mut after);
        assert_eq!(after, before);
    }

    #[test]
    fn rewrite_rejects_invalid_resource_ids_before_mapping() {
        for invalid in [
            Value::from(0_u64),
            Value::from(-1_i64),
            Value::Float(1.0),
            Value::Text("1".into()),
            Value::Null,
        ] {
            let mut value = Value::Tag(payload::OBJECT_TAG, Box::new(invalid));
            let original = value.clone();
            let problem = rewrite(&mut value, &mut |_| {
                panic!("invalid resource reached mapping callback")
            })
            .unwrap_err();
            assert_eq!(problem.code, 2);
            assert!(problem.message.contains("u64"));
            assert_eq!(value, original);
        }
    }

    #[test]
    fn rewrite_propagates_mapping_failure_and_stops_traversal() {
        let mut value = Value::Array(vec![
            ObjectRef(1).into(),
            ObjectRef(2).into(),
            ObjectRef(3).into(),
        ]);
        let mut visited = Vec::new();
        let problem = rewrite(&mut value, &mut |id| {
            visited.push(id);
            if id == 2 {
                return Err(error(4, "mapping refused"));
            }
            Ok(id + 100)
        })
        .unwrap_err();
        assert_eq!(visited, [1, 2]);
        assert_eq!(problem.code, 4);
        assert_eq!(problem.message, "mapping refused");
    }
}
