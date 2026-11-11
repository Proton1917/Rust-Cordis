use cordis::{ComponentDef, Context, Inject};
use cordis_testkit::TestExecutor;

fn main() -> cordis::Result {
    // 宿主提供任务派发与时钟；这里用 testkit 的确定性执行器。
    let host = TestExecutor::new();
    let root = Context::new(host.clone());

    // 可回退效应：效应体登记逆变换，释放时按相反顺序执行。
    let effect = root.effect("greeting", |fx| {
        println!("effect on");
        fx.defer(|| {
            println!("effect off");
            Ok(())
        })
    })?;
    host.finish(effect.dispose())?;

    // 组件：激活时贡献效应；重载或停用时由内核完整回退。
    let server = ComponentDef::new("server", Inject::new(), |ctx, port: u16| async move {
        println!("listen {port}");
        ctx.effect("listener", move |fx| {
            fx.defer(move || {
                println!("close {port}");
                Ok(())
            })
        })?;
        Ok(())
    });
    let fiber = root.mount_with(&server, 8080u16)?;
    host.finish(fiber.settled())?;
    host.finish(fiber.update(9090u16))?;
    host.finish(fiber.dispose())?;
    host.finish(root.fiber().dispose())?;
    Ok(())
}
