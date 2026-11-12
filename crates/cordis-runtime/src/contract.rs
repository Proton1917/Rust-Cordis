//! DOC-11 §8、ADR-0017：独立验证会话的断言、阶段转换与状态继承。

use crate::{
    config::{Result, parameter},
    control::Controller,
    storage,
};
use cordis_supervisor::manager::{Arguments, CallContext, LocalReply, LocalService, Manager};
use cordis_types::{ContractAssertion, ContractReport};
use cordis_up::{Limits, Value, WireError, rpc::HandlerFuture};
use parking_lot::Mutex;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Notify;

pub struct Plan {
    pub suite: String,
    pub subject: String,
    pub report: PathBuf,
    pub timeout: Duration,
}

#[derive(Clone)]
pub(crate) struct Session(Arc<Inner>);

struct Inner {
    plan: Plan,
    state: Mutex<State>,
    changed: Notify,
}

#[derive(Default)]
struct State {
    report: ContractReport,
    requested_roundtrip: bool,
    requested_finish: bool,
    bytes: usize,
    observing: bool,
    watched: Vec<cordis::Fiber>,
    failure: Option<WireError>,
}

enum Decision {
    Wait,
    Roundtrip,
    Finish,
}

impl Session {
    /// ADR-0017：会话只持有指定实例和报告状态，生命周期动作由 Controller 驱动。
    pub(crate) fn new(plan: Plan) -> Result<Self> {
        crate::config::instance_name(&plan.suite)?;
        crate::config::instance_name(&plan.subject)?;
        if plan.suite == plan.subject {
            return Err(parameter("套件和被测实例需要独立名称"));
        }
        Ok(Self(Arc::new(Inner {
            plan,
            state: Mutex::new(State::default()),
            changed: Notify::new(),
        })))
    }

    /// Q0、ADR-0017：服务由核心提供，授权限定为本次指定套件实例。
    pub(crate) fn install(&self, manager: &Manager) -> Result<()> {
        manager.provide_builtin(
            "cordis.contract@1.0",
            ["report", "finish", "stage", "request_state_roundtrip"]
                .map(str::to_owned)
                .to_vec(),
            Arc::new(self.clone()),
        )?;
        let changed = self.clone();
        manager
            .context()
            .on::<cordis::InternalStatus>(move |(fiber, _)| {
                let mut state = changed.0.state.lock();
                if state.observing
                    && state.watched.contains(fiber)
                    && fiber.state() != cordis::FiberState::Active
                {
                    state.failure.get_or_insert_with(|| {
                        WireError::new(4, format!("验证实例在检查阶段进入 {:?}", fiber.state()))
                    });
                }
                drop(state);
                changed.0.changed.notify_one();
                None
            })
            .map_err(|problem| WireError::new(1, problem.to_string()))?;
        Ok(())
    }

    /// ADR-0017、CP-4：启动和全部验证阶段共用截止时间，超时后监管器继续拥有已提交动作。
    pub(crate) async fn run(&self, controller: &mut Controller) -> Result<()> {
        let result = tokio::time::timeout(self.0.plan.timeout, async {
            controller.start().await?;
            controller.contract_settle(&self.0.plan.subject).await?;
            controller.contract_settle(&self.0.plan.suite).await?;
            self.observe(controller)?;
            self.run_phases(controller).await
        })
        .await
        .map_err(|_| WireError::new(6, "契约验证超过截止时间"));
        self.0.state.lock().observing = false;
        result?
    }

    /// ADR-0017：套件先交回当前 apply，再进行状态迁移；完成通知只结束当前验证。
    async fn run_phases(&self, controller: &mut Controller) -> Result<()> {
        loop {
            self.check_instances(&controller.manager)?;
            match self.decision() {
                Decision::Wait => self.0.changed.notified().await,
                Decision::Finish => {
                    let mut state = self.0.state.lock();
                    state.report.finished = true;
                    state.report.passed = state.report.error.is_none()
                        && !state.report.assertions.is_empty()
                        && state
                            .report
                            .assertions
                            .iter()
                            .all(|assertion| assertion.passed);
                    state.observing = false;
                    return if state.report.passed {
                        Ok(())
                    } else {
                        Err(WireError::new(1, "契约断言失败"))
                    };
                }
                Decision::Roundtrip => {
                    self.0.state.lock().observing = false;
                    controller
                        .contract_roundtrip(&self.0.plan.suite, &self.0.plan.subject, || {
                            let mut state = self.0.state.lock();
                            state.report.stage = 1;
                            state.requested_roundtrip = false;
                        })
                        .await?;
                    self.observe(controller)?;
                }
            }
        }
    }

    /// ADR-0017、CP-6：检查阶段观察真实纤程，阶段迁移期间由监管事务判断成败。
    fn observe(&self, controller: &Controller) -> Result<()> {
        let watched = [&self.0.plan.subject, &self.0.plan.suite]
            .into_iter()
            .map(|name| controller.manager.instance_fiber(name))
            .collect::<Result<Vec<_>>>()?;
        let mut state = self.0.state.lock();
        state.watched = watched;
        state.observing = true;
        Ok(())
    }

    /// CP-6：运行故障以真实实例状态结束验证，依赖等待状态保留错误原因。
    fn check_instances(&self, manager: &Manager) -> Result<()> {
        if let Some(problem) = &self.0.state.lock().failure {
            return Err(problem.clone());
        }
        let snapshot = manager.snapshot();
        for name in [&self.0.plan.subject, &self.0.plan.suite] {
            let instance = snapshot
                .instances
                .iter()
                .find(|instance| &instance.name == name)
                .ok_or_else(|| parameter(format!("验证实例 {name} 缺席")))?;
            if instance.state != "active" {
                return Err(WireError::new(
                    4,
                    format!(
                        "验证实例 {name} 状态 {}：{}",
                        instance.state,
                        instance
                            .error
                            .as_ref()
                            .map(|error| error.message.as_str())
                            .unwrap_or("等待声明依赖或生命周期结束")
                    ),
                ));
            }
        }
        Ok(())
    }

    /// ADR-0017：阶段请求保持互斥，等待由服务调用或生命周期事件唤醒。
    fn decision(&self) -> Decision {
        let state = self.0.state.lock();
        if state.requested_finish {
            Decision::Finish
        } else if state.requested_roundtrip {
            Decision::Roundtrip
        } else {
            Decision::Wait
        }
    }

    /// DOC-11 §8.2：最终报告包括核心清理错误，成功判定依真实断言和套件完成通知。
    pub(crate) fn save(&self, outcome: &Result<()>) -> Result<()> {
        let mut state = self.0.state.lock();
        if let Err(problem) = outcome {
            state.report.passed = false;
            state.report.error = Some(problem.to_string());
        }
        let data = serde_json::to_vec_pretty(&state.report).map_err(parameter)?;
        storage::write(&self.0.plan.report, &data)
    }
}

impl LocalService for Session {
    /// DOC-11 §8、CP-2/CP-8：单次报告在资源边界计量，阶段请求立即返回由核心事务接续。
    fn invoke<'a>(
        &'a self,
        context: &'a CallContext,
        method: &'a str,
        args: &'a Arguments,
    ) -> HandlerFuture<'a, Result<LocalReply>> {
        Box::pin(async move {
            if context.instance != self.0.plan.suite {
                return Err(WireError::new(9, "契约报告服务限本次指定套件使用"));
            }
            let mut state = self.0.state.lock();
            if state.report.finished || state.requested_finish {
                return Err(WireError::new(4, "套件已交回最终结果"));
            }
            let output = if method == "report" {
                let (name, passed, message): (String, bool, String) =
                    Value::Array(args.values.clone())
                        .deserialized()
                        .map_err(parameter)?;
                self.record(&mut state, name, passed, message)?;
                Value::Null
            } else {
                if !args.values.is_empty() {
                    return Err(parameter("该方法接受空参数数组"));
                }
                match method {
                    "stage" => Value::Integer(state.report.stage.into()),
                    "finish" if !state.requested_roundtrip => {
                        state.requested_finish = true;
                        Value::Null
                    }
                    "finish" => return Err(parameter("状态往返完成后再交回最终结果")),
                    "request_state_roundtrip"
                        if state.report.stage == 0 && !state.requested_roundtrip =>
                    {
                        state.requested_roundtrip = true;
                        Value::Null
                    }
                    "request_state_roundtrip" => {
                        return Err(parameter("状态往返在阶段 0 请求一次"));
                    }
                    _ => return Err(WireError::new(3, "契约服务方法缺席")),
                }
            };
            drop(state);
            self.0.changed.notify_one();
            Ok(LocalReply::new(output))
        })
    }
}

impl Session {
    /// CP-2、CP-8：断言条数与累计文本字节在接收边界受协议预算限制。
    fn record(&self, state: &mut State, name: String, passed: bool, message: String) -> Result<()> {
        let name = format!("{}.{}", self.0.plan.suite, name);
        let bytes = state
            .bytes
            .saturating_add(name.len())
            .saturating_add(message.len())
            .saturating_add(64);
        if state.report.assertions.len() >= Limits::DEFAULT.max_collection
            || bytes > Limits::DEFAULT.max_frame / 6
        {
            state.failure = Some(WireError::new(8, "契约报告超过容量上限"));
            self.0.changed.notify_one();
            return Err(WireError::new(8, "契约报告超过容量上限"));
        }
        state.bytes = bytes;
        state.report.assertions.push(ContractAssertion {
            name,
            passed,
            message,
        });
        Ok(())
    }
}
