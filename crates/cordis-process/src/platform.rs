//! CP-7、ADR-0027：跨平台进程组所有权与回收原语。
//! Unix 使用独立进程组与 SIGCHLD/waitid/killpg；Windows 使用 Job Object 与进程树终止。

use cordis_up::WireError;
use std::{
    path::Path,
    process::{ExitStatus, Stdio},
};
use tokio::process::{Child, Command};

pub(crate) type Result<T> = std::result::Result<T, WireError>;

fn error(code: u32, message: impl Into<String>) -> WireError {
    WireError::new(code, message)
}

fn io_error(problem: std::io::Error) -> WireError {
    error(1, problem.to_string())
}

#[cfg(unix)]
mod imp {
    use super::*;
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    use tokio::signal::unix::{Signal as ChildSignals, SignalKind, signal};

    pub(crate) struct ChildGuard {
        pid: u32,
        signals: ChildSignals,
    }

    pub(crate) fn spawn(
        command: &[String],
        env: Option<&[(String, String)]>,
        cwd: &Path,
        stdin: Stdio,
    ) -> Result<(Child, ChildGuard)> {
        let Some(program) = command.first() else {
            return Err(error(2, "进程命令为空"));
        };
        let signals = signal(SignalKind::child()).map_err(io_error)?;
        let mut builder = Command::new(program);
        builder
            .args(&command[1..])
            .current_dir(cwd)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        if let Some(vars) = env {
            for (key, value) in vars {
                builder.env(key, value);
            }
        }
        let child = builder.spawn().map_err(io_error)?;
        let pid = child.id().expect("new child has PID");
        Ok((child, ChildGuard { pid, signals }))
    }

    impl ChildGuard {
        pub(crate) fn pid(&self) -> u32 {
            self.pid
        }

        /// ADR-0027：只观察本进程拥有的Child，NOWAIT将PID保留至组信号结束。
        pub(crate) fn exited(&self) -> Result<bool> {
            let pid =
                rustix::process::Pid::from_raw(self.pid as i32).expect("child PID is positive");
            waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG,
            )
            .map(|state| state.is_some())
            .map_err(|problem| error(1, problem.to_string()))
        }

        pub(crate) async fn wait_signal(&mut self, _child: &mut Child) -> Result<()> {
            self.signals
                .recv()
                .await
                .map(|_| ())
                .ok_or_else(|| error(4, "子进程退出信号流已关闭"))
        }

        /// CP-7、ADR-0027：调用方持有待回收Child；有效组权限错误交回调用方。
        pub(crate) async fn terminate_and_reap(&mut self, child: &mut Child) -> Result<ExitStatus> {
            let group = Pid::from_raw(self.pid as i32);
            let signal_error = match killpg(group, Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => None,
                Err(Errno::EPERM) if self.exited()? => Some(Errno::EPERM),
                Err(problem) => return Err(error(1, format!("终止插件进程组失败：{problem}"))),
            };
            let status = child.wait().await.map_err(io_error)?;
            // ADR-0027：回收后只查询组是否已空，终止信号已在PID受保有期间发出。
            if let Some(problem) = signal_error
                && killpg(group, None) != Err(Errno::ESRCH)
            {
                return Err(error(1, format!("终止插件进程组失败：{problem}")));
            }
            Ok(status)
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::mem::{size_of, zeroed};
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

    struct JobHandle(HANDLE);

    // SAFETY: Win32 Job Object 句柄跨线程安全使用并由唯一所有者在 Drop 时关闭。
    unsafe impl Send for JobHandle {}
    // SAFETY: 句柄只经由同步或原子路径持有，不在无保护下共享可变内存。
    unsafe impl Sync for JobHandle {}

    impl JobHandle {
        fn create() -> Result<Self> {
            // SAFETY: 创建匿名 Job Object，空名称与默认安全属性合法。
            let handle = unsafe { CreateJobObjectW(null_mut(), null_mut()) };
            if handle.is_null() {
                return Err(io_error(std::io::Error::last_os_error()));
            }
            let job = Self(handle);
            // SAFETY: 零初始化 Win32 结构体并仅设置 KillOnJobClose 标志。
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: job.0 为有效句柄，info 尺寸与信息类别匹配。
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(io_error(std::io::Error::last_os_error()));
            }
            Ok(job)
        }

        fn assign(&self, process: HANDLE) -> Result<()> {
            // SAFETY: self.0 与 process 均为当前有效的内核对象句柄。
            let ok = unsafe { AssignProcessToJobObject(self.0, process) };
            if ok == 0 {
                return Err(io_error(std::io::Error::last_os_error()));
            }
            Ok(())
        }

        fn terminate(&self) -> Result<()> {
            // SAFETY: self.0 为有效 Job Object 句柄，退出码 1 终止整棵进程树。
            let ok = unsafe { TerminateJobObject(self.0, 1) };
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                // 进程树已全部退出时忽略访问/完成态。
                if err.raw_os_error() != Some(5) {
                    return Err(error(1, format!("终止插件进程组失败：{err}")));
                }
            }
            Ok(())
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            // SAFETY: self.0 由 JobHandle 唯一拥有，仅关闭一次。
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub(crate) struct ChildGuard {
        pid: u32,
        job: JobHandle,
        status: Option<ExitStatus>,
    }

    pub(crate) fn spawn(
        command: &[String],
        env: Option<&[(String, String)]>,
        cwd: &Path,
        stdin: Stdio,
    ) -> Result<(Child, ChildGuard)> {
        let Some(program) = command.first() else {
            return Err(error(2, "进程命令为空"));
        };
        let job = JobHandle::create()?;
        let mut builder = Command::new(program);
        builder
            .args(&command[1..])
            .current_dir(cwd)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .creation_flags(CREATE_NEW_PROCESS_GROUP);
        if let Some(vars) = env {
            for (key, value) in vars {
                builder.env(key, value);
            }
        }
        let mut child = builder.spawn().map_err(io_error)?;
        let pid = child.id().expect("new child has PID");
        if let Some(handle) = child.raw_handle()
            && let Err(problem) = job.assign(handle)
        {
            let _ = child.start_kill();
            return Err(problem);
        }
        Ok((
            child,
            ChildGuard {
                pid,
                job,
                status: None,
            },
        ))
    }

    impl ChildGuard {
        pub(crate) fn pid(&self) -> u32 {
            self.pid
        }

        pub(crate) fn exited(&self) -> Result<bool> {
            Ok(self.status.is_some())
        }

        pub(crate) async fn wait_signal(&mut self, child: &mut Child) -> Result<()> {
            if self.status.is_some() {
                return Ok(());
            }
            let status = child.wait().await.map_err(io_error)?;
            self.status = Some(status);
            Ok(())
        }

        pub(crate) async fn terminate_and_reap(&mut self, child: &mut Child) -> Result<ExitStatus> {
            self.job.terminate()?;
            if let Some(status) = self.status {
                return Ok(status);
            }
            let status = child.wait().await.map_err(io_error)?;
            self.status = Some(status);
            Ok(status)
        }
    }
}

pub(crate) use imp::{ChildGuard, spawn};
